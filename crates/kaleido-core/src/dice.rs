//! 骰子引擎（吞噬 mnehmos.rpg.mcp `dice.ts` 口径，MIT）。
//!
//! - 表达式：`NdX` / `dX` / `NdX+M` / `NdXdl1|dh|kl|khN` / `NdX!`（爆炸）
//! - 可复现 RNG：种子字符串 → xorshift64（无 rand 依赖，纯核可单测）
//! - ST-27 检定调用方用 `seed_from(&str)` 从 turn+action 派生种子，
//!   同一回合同一行动必出同一结果（可审计、可复跑）。

/// 解析后的骰子表达式。
#[derive(Debug, Clone, PartialEq)]
pub struct DiceExpr {
    pub count: usize,
    pub sides: u32,
    pub drop_lowest: usize,
    pub drop_highest: usize,
    pub keep_lowest: usize,
    pub keep_highest: usize,
    pub modifier: i64,
    pub explode: bool,
}

/// 解析 `2d6+4` / `d20` / `4d6dl1` / `1d20!` 等。非法 → None。
pub fn parse_dice_expr(spec: &str) -> Option<DiceExpr> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    // [count] d sides [dl|dh|kl|kh N] [+-M] [!]
    let lower = s.to_ascii_lowercase();
    let dpos = lower.find('d')?;
    let count_str = s[..dpos].trim();
    let count = if count_str.is_empty() {
        1
    } else {
        count_str.parse::<usize>().ok()?
    };
    let mut rest = s[dpos + 1..].trim();
    if rest.is_empty() {
        return None;
    }
    // sides digits
    let mut i = 0;
    while i < rest.len() && rest.as_bytes()[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return None;
    }
    let sides: u32 = rest[..i].parse().ok()?;
    rest = rest[i..].trim();
    let mut ex = DiceExpr {
        count,
        sides,
        drop_lowest: 0,
        drop_highest: 0,
        keep_lowest: 0,
        keep_highest: 0,
        modifier: 0,
        explode: false,
    };
    // dl|dh|kl|kh N
    let rl = rest.to_ascii_lowercase();
    for key in ["dl", "dh", "kl", "kh"] {
        if rl.starts_with(key) {
            let after = rest[key.len()..].trim();
            let mut j = 0;
            while j < after.len() && after.as_bytes()[j].is_ascii_digit() {
                j += 1;
            }
            if j == 0 {
                return None;
            }
            let n: usize = after[..j].parse().ok()?;
            match key {
                "dl" => ex.drop_lowest = n,
                "dh" => ex.drop_highest = n,
                "kl" => ex.keep_lowest = n,
                _ => ex.keep_highest = n,
            }
            rest = after[j..].trim();
            break;
        }
    }
    // +-M
    if rest.starts_with('+') || rest.starts_with('-') {
        let neg = rest.starts_with('-');
        let after = rest[1..].trim();
        let mut j = 0;
        while j < after.len() && after.as_bytes()[j].is_ascii_digit() {
            j += 1;
        }
        if j == 0 {
            return None;
        }
        let m: i64 = after[..j].parse().ok()?;
        ex.modifier = if neg { -m } else { m };
        rest = after[j..].trim();
    }
    // !
    if rest == "!" {
        ex.explode = true;
        rest = "";
    }
    if !rest.is_empty() {
        return None;
    }
    if count < 1 || sides < 1 {
        return None;
    }
    Some(ex)
}

/// xorshift64*：种子字符串 → 确定性骰流（FNV-1a 播种）。
pub struct SeededRng {
    state: u64,
}

impl SeededRng {
    pub fn new(seed: &str) -> Self {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in seed.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        if h == 0 {
            h = 0x853c49e6748fea9b;
        }
        Self { state: h }
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545f4914f6cdd1d)
    }
    /// [1, sides]
    pub fn roll_one(&mut self, sides: u32) -> u32 {
        if sides <= 1 {
            return 1;
        }
        ((self.next_u64() >> 11) % sides as u64) as u32 + 1
    }
}

/// 从任意字符串派生种子（turn+action 等）。
pub fn seed_from(parts: &[&str]) -> String {
    parts.join("|")
}

/// 掷表达式。返回 (各骰明细, 总和含修正)。
pub fn roll_expr(ex: &DiceExpr, rng: &mut SeededRng) -> (Vec<u32>, i64) {
    let mut rolls = Vec::with_capacity(ex.count);
    for _ in 0..ex.count {
        let mut v = rng.roll_one(ex.sides);
        if ex.explode {
            // 爆炸：满面重掷累加（cap 100 防无限）
            let mut total = v;
            for _ in 0..100 {
                if v != ex.sides {
                    break;
                }
                v = rng.roll_one(ex.sides);
                total += v;
            }
            rolls.push(total);
        } else {
            rolls.push(v);
        }
    }
    // keep/drop
    let mut idx: Vec<usize> = (0..rolls.len()).collect();
    idx.sort_by_key(|&i| rolls[i]);
    let mut keep = vec![true; rolls.len()];
    if ex.keep_highest > 0 {
        let n = ex.keep_highest.min(rolls.len());
        for (k, &i) in idx.iter().rev().take(n).enumerate() {
            let _ = k;
            keep[i] = true;
        }
        // 先全关再开
        keep.fill(false);
        for &i in idx.iter().rev().take(n) {
            keep[i] = true;
        }
    } else if ex.keep_lowest > 0 {
        let n = ex.keep_lowest.min(rolls.len());
        keep.fill(false);
        for &i in idx.iter().take(n) {
            keep[i] = true;
        }
    } else {
        if ex.drop_lowest > 0 {
            let n = ex.drop_lowest.min(rolls.len());
            for &i in idx.iter().take(n) {
                keep[i] = false;
            }
        }
        if ex.drop_highest > 0 {
            let n = ex.drop_highest.min(rolls.len());
            for &i in idx.iter().rev().take(n) {
                keep[i] = false;
            }
        }
    }
    let kept: Vec<u32> = rolls
        .iter()
        .enumerate()
        .filter(|(i, _)| keep[*i])
        .map(|(_, &v)| v)
        .collect();
    let sum: i64 = kept.iter().map(|&v| v as i64).sum::<i64>() + ex.modifier;
    (kept, sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_basic() {
        assert_eq!(
            parse_dice_expr("2d6+4").unwrap(),
            DiceExpr { count: 2, sides: 6, drop_lowest: 0, drop_highest: 0, keep_lowest: 0, keep_highest: 0, modifier: 4, explode: false }
        );
        assert!(parse_dice_expr("d20").unwrap().count == 1);
        let e = parse_dice_expr("4d6dl1").unwrap();
        assert_eq!(e.drop_lowest, 1);
        let e = parse_dice_expr("1d20!").unwrap();
        assert!(e.explode);
        assert!(parse_dice_expr("abc").is_none());
        assert!(parse_dice_expr("").is_none());
    }
    #[test]
    fn seeded_deterministic() {
        let mut a = SeededRng::new("turn5|潜行");
        let mut b = SeededRng::new("turn5|潜行");
        assert_eq!(a.roll_one(20), b.roll_one(20));
        let mut c = SeededRng::new("turn6|潜行");
        // 不同种子大概率不同（不强求，xorshift 首值可能碰巧同）
        let _ = c.roll_one(20);
    }
    #[test]
    fn roll_keep_drop() {
        let mut rng = SeededRng::new("test");
        let ex = parse_dice_expr("4d6dl1").unwrap();
        let (kept, sum) = roll_expr(&ex, &mut rng);
        assert_eq!(kept.len(), 3);
        assert!(sum >= 3 && sum <= 18);
    }
}
