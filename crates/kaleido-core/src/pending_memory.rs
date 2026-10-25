//! # 悬而未决（pending）记忆三要素
//!
//! 参考 dianziji 实测的记忆摘要格式：只留**未兑现**的承诺/伏笔，每条带**轮次戳**。
//!
//! ```text
//! 【悬而未决】#1 第6轮 下周帮苏韵搬茶具
//! 【悬而未决】#2 第6轮 她说会主动来找你
//! ```
//!
//! 三要素：**分类法**（只留未兑现，不留事实）+ **轮次戳**（便于回溯何时提起）+ 宿主侧
//! `integrating` 异步标志（本模块只提供解析与归并，标志由调用方维护）。

use serde::{Deserialize, Serialize};

/// 一条悬而未决项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingItem {
    /// 稳定序号（同一份摘要内递增，用于跨轮对齐同一件事）。
    pub seq: u32,
    /// 提出该承诺/伏笔的轮次（`n` 戳）。
    pub turn: u64,
    /// 条目正文（去掉 `【悬而未决】#N 第M轮 ` 前缀后的内容）。
    pub text: String,
}

impl PendingItem {
    /// 渲染回规范的 `【悬而未决】#seq 第turn轮 text` 形式。
    pub fn render(&self) -> String {
        format!("【悬而未决】#{} 第{}轮 {}", self.seq, self.turn, self.text)
    }
}

// 原先这里有个只含 `integrating: bool` 的 `PendingStatus`——它被
// `PendingStore::PendingSnapshot` 取代（后者还带 items/summary/updatedAt，
// 且 `begin_merge` 真的会把 integrating 置起来）。留着它就是纯摆设，故删除。

/// 解析一段摘要里的所有 `【悬而未决】` 行。
///
/// 容错：
/// - 序号可缺（`【悬而未决】 第6轮 …`），按出现顺序补号
/// - 轮次戳可缺，整条按 `fallback_turn` 计
/// - 行内其他位置的标记不误伤（要求行首，允许前导空白与列表符号）
pub fn parse_pending(text: &str, fallback_turn: u64) -> Vec<PendingItem> {
    const TAG: &str = "【悬而未决】";
    let mut out: Vec<PendingItem> = Vec::new();
    for line in text.lines() {
        let l = line.trim_start().trim_start_matches(['-', '*', '+']).trim_start();
        let Some(rest) = l.strip_prefix(TAG) else {
            continue;
        };
        let mut body = rest.trim_start();

        // 可选 `#seq`
        let mut seq: Option<u32> = None;
        if let Some(stripped) = body.strip_prefix('#') {
            let digits: String = stripped.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() {
                seq = digits.parse::<u32>().ok();
                body = stripped[digits.len()..].trim_start();
            }
        }

        // 可选 `第N轮`
        let mut turn = fallback_turn;
        if let Some(idx) = body.find('第') {
            let after = &body[idx + '第'.len_utf8()..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !digits.is_empty() && after[digits.len()..].trim_start().starts_with('轮') {
                if let Ok(t) = digits.parse::<u64>() {
                    turn = t;
                    // 吃掉 `第N轮` 与其后一个空白
                    let rest_after = &after[digits.len()..];
                    let trimmed = rest_after.trim_start();
                    let skip = rest_after.len() - trimmed.len();
                    body = &rest_after[skip + '轮'.len_utf8()..];
                }
            }
        }

        let text = body.trim().to_string();
        if text.is_empty() {
            continue;
        }
        out.push(PendingItem {
            seq: seq.unwrap_or(0),
            turn,
            text,
        });
    }
    // 缺号的按出现顺序补号（保持原序号则不动）
    let mut next = 1u32;
    for it in out.iter_mut() {
        if it.seq == 0 {
            it.seq = next;
        }
        next = it.seq + 1;
    }
    out
}

/// 把一批悬而未决项渲染成规范摘要文本（空则空串）。
pub fn render_pending(items: &[PendingItem]) -> String {
    items.iter().map(|i| i.render()).collect::<Vec<_>>().join("\n")
}

/// 跨轮归并：保留上一轮未兑现项 + 本轮新增项，按 text 去重；本轮轮次写进新项。
///
/// 已兑现的项（宿主判定后不再传入）自然消失——事实化交给调用方，这里只做并集去重。
pub fn merge_pending(prev: &[PendingItem], next: &[PendingItem]) -> Vec<PendingItem> {
    let mut merged: Vec<PendingItem> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for it in prev.iter().chain(next.iter()) {
        let key = it.text.trim().to_string();
        if seen.iter().any(|s| *s == key) {
            continue;
        }
        seen.push(key);
        merged.push(it.clone());
    }
    // 重排序号，保持稳定
    for (i, it) in merged.iter_mut().enumerate() {
        it.seq = (i + 1) as u32;
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "【悬而未决】#1 第6轮 下周帮苏韵搬茶具\n【悬而未决】#2 第6轮 她说会主动来找你";

    #[test]
    fn parses_real_fixture() {
        let items = parse_pending(REAL, 0);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].seq, 1);
        assert_eq!(items[0].turn, 6);
        assert_eq!(items[0].text, "下周帮苏韵搬茶具");
        assert_eq!(items[1].seq, 2);
    }

    #[test]
    fn roundtrip_render_parse() {
        let items = parse_pending(REAL, 0);
        assert_eq!(render_pending(&items), REAL);
    }

    #[test]
    fn tolerates_missing_seq_and_turn() {
        let t = "【悬而未决】# 帮她搬家\n【悬而未决】第3轮 她会回来";
        let items = parse_pending(t, 99);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].seq, 1);
        assert_eq!(items[0].turn, 99);
        assert_eq!(items[1].seq, 2);
        assert_eq!(items[1].turn, 3);
    }

    #[test]
    fn skips_empty_and_non_tag_lines() {
        let t = "普通事实行\n【悬而未决】#1 第2轮 \n正文里的【悬而未决】不算\n【悬而未决】#2 第2轮 真项";
        let items = parse_pending(t, 0);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "真项");
    }

    #[test]
    fn list_markers_are_tolerated() {
        let t = "- 【悬而未决】#1 第5轮 候补项";
        let items = parse_pending(t, 0);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].text, "候补项");
    }

    #[test]
    fn merge_dedups_and_renumbers() {
        let prev = parse_pending(REAL, 0);
        let next = vec![PendingItem {
            seq: 9,
            turn: 7,
            text: "下周帮苏韵搬茶具".into(),
        }];
        let m = merge_pending(&prev, &next);
        // 重复项被去掉，只剩 2 条原项
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].seq, 1);
        assert_eq!(m[1].seq, 2);
    }

    #[test]
    fn merge_keeps_old_turn_for_unchanged() {
        let prev = parse_pending(REAL, 0);
        let next = vec![PendingItem {
            seq: 3,
            turn: 7,
            text: "新的一项".into(),
        }];
        let m = merge_pending(&prev, &next);
        assert_eq!(m.len(), 3);
        // 老项仍带第6轮，新项是第7轮
        assert_eq!(m[0].turn, 6);
        assert_eq!(m[2].turn, 7);
        assert_eq!(m[2].seq, 3);
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(parse_pending("", 0).is_empty());
        assert!(parse_pending("没有标记的文本", 3).is_empty());
        assert_eq!(render_pending(&[]), "");
    }
}