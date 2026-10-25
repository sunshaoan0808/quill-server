//! text2.rs — RP 文本规范化 + SFX 切分（吸收自 pnotisdev/rp text/messageSegments.ts + text/sfx.ts，MIT）。
//!
//! 纪律（抄rp）：
//! - `normalize_rp_markup` 幂等：HTML→星号/剥除，`**`/`***` 坍成单个 `*`。
//! - `split_message_segments` 是渲染唯一真相源：text/action/quote/sfx 四段；
//!   SFX 只在 text/action 里找，绝不进 `"quote"`。
//! - SFX 词表宁紧勿松：纯拟声才收，喊叫/强调动词不收。
//! - 全纯函数，零迁移。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Text,
    Action,
    Quote,
    Sfx,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageSegment {
    pub kind: SegmentKind,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SfxConfig {
    pub disabled: bool,
    pub extra_words: Vec<String>,
}

/// 内置拟声词（抄rp BUILTIN_SFX_WORDS；全大写比对）。
pub const BUILTIN_SFX_WORDS: &[&str] = &[
    "BOOM", "KABOOM", "KABLAM", "KAPOW", "BANG", "BAM", "POW", "WHAM", "WHUMP", "WHUD",
    "THUD", "THUMP", "THOOM", "THOK", "CRASH", "SMASH", "SLAM", "CRACK", "CRACKLE", "SNAP",
    "CRUNCH", "POP", "KNOCK", "RAP", "TAP", "THWACK", "WHACK", "SMACK", "SLAP", "CLANG",
    "CLANK", "CLATTER", "CLICK", "CLACK", "TICK", "TOCK", "DING", "DONG", "RING", "BUZZ",
    "BZZT", "ZAP", "WHOOSH", "WOOSH", "SWOOSH", "SWISH", "FWOOSH", "FWIP", "WHIP", "SPLASH",
    "SPLAT", "PLOP", "PLONK", "PLINK", "PLUNK", "DRIP", "SIZZLE", "HISS", "FIZZ", "WHIRR",
    "RUMBLE", "ROAR", "GROWL", "GRR", "CREAK", "SQUEAK", "SCREECH", "VROOM", "ZOOM", "HONK",
    "BEEP", "TOOT", "GULP", "MUNCH", "SLURP", "CHOMP", "NOM", "TWANG", "BOING", "SPROING",
    "TCH", "HMPH", "THWIP", "SHING", "KATHUNK", "KACHUNK", "KATHUMP", "RATATAT", "RATTAT",
    "DOKI", "DOKIDOKI", "ZAWA", "GOGOGO", "BADUMP", "PATTER",
];

/// RP 文本漂移归一化：HTML→星号/剥除，`**`/`***` 坍成单个 `*`。幂等。
pub fn normalize_rp_markup(text: &str) -> String {
    let mut out = text.to_string();
    if out.contains('<') {
        if let Ok(re) = regex::Regex::new(r"(?i)<(i|em)\b[^>]*>([\s\S]*?)</\1>") {
            out = re.replace_all(&out, "*$2*").to_string();
        }
        if let Ok(re) = regex::Regex::new(r"(?i)<(b|strong)\b[^>]*>([\s\S]*?)</\1>") {
            out = re.replace_all(&out, "**$2**").to_string();
        }
        if let Ok(re) = regex::Regex::new(r"(?i)<br\s*/?>") {
            out = re.replace_all(&out, "\n").to_string();
        }
        if let Ok(re) = regex::Regex::new(r"</?[a-zA-Z][a-zA-Z0-9]*\b[^>]*>") {
            out = re.replace_all(&out, "").to_string();
        }
    }
    if let Ok(re) = regex::Regex::new(r"\*\*\s*\*\*") {
        out = re.replace_all(&out, "").to_string();
    }
    if let Ok(re) = regex::Regex::new(r"\*{2,}([^\*\n]+?)\*{2,}") {
        out = re.replace_all(&out, "*$1*").to_string();
    }
    if let Ok(re) = regex::Regex::new(r"\*{2,}") {
        out = re.replace_all(&out, "*").to_string();
    }
    out
}

/// 自由输入的 extra 拟声词（逗号/换行/空格分隔）→ 干净词表。
pub fn parse_sfx_word_list(raw: &str) -> Vec<String> {
    raw.split(|c: char| c.is_whitespace() || c == ',')
        .map(|w| w.chars().filter(|c| c.is_ascii_alphabetic() || *c == '-').collect::<String>())
        .map(|w| w.trim().to_string())
        .filter(|w| w.replace('-', "").len() >= 2)
        .collect()
}

fn builtin_set() -> std::collections::HashSet<String> {
    BUILTIN_SFX_WORDS.iter().map(|s| s.to_string()).collect()
}

fn sfx_word_set(extra: &[String]) -> std::collections::HashSet<String> {
    let mut set = builtin_set();
    for w in extra {
        let clean: String = w.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_uppercase();
        if clean.len() >= 2 {
            set.insert(clean);
        }
    }
    set
}

/// 大写 + 拉长音坍缩（"boooom"→"boom"/"bom"），拉长拼写也命中。
fn normalize_sfx_variants(word: &str) -> Vec<String> {
    let upper = word.to_uppercase();
    let mut out = vec![upper.clone()];
    let mut collapsed2 = String::new();
    let mut prev: Option<char> = None;
    let mut run = 0usize;
    for c in upper.chars() {
        if Some(c) == prev {
            run += 1;
        } else {
            run = 1;
            prev = Some(c);
        }
        if run <= 2 {
            collapsed2.push(c);
        }
    }
    let mut collapsed1 = String::new();
    let mut prev1: Option<char> = None;
    for c in upper.chars() {
        if Some(c) != prev1 {
            collapsed1.push(c);
            prev1 = Some(c);
        }
    }
    if collapsed2 != upper {
        out.push(collapsed2);
    }
    if collapsed1 != upper && !out.contains(&collapsed1) {
        out.push(collapsed1);
    }
    out
}

fn strip_token_edges(token: &str) -> String {
    let s = token.trim_start_matches(|c: char| !c.is_ascii_alphabetic());
    let s = s.trim_end_matches(|c: char| !c.is_ascii_alphabetic());
    s.to_string()
}

/// 单个空白分隔 token（可带连字符与尾部 !?~.）是不是拟声。
fn is_sfx_token(token: &str, word_set: &std::collections::HashSet<String>) -> bool {
    let core = strip_token_edges(token);
    if core.len() < 2 {
        return false;
    }
    let parts: Vec<&str> = core.split(|c| c == '-' || c == '–').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return false;
    }
    let in_set = |w: &str| normalize_sfx_variants(w).iter().any(|n| word_set.contains(n));
    if parts.iter().all(|p| in_set(p)) {
        return true;
    }
    // "ka-thunk" 形：分片单独不是音，连起来是。
    in_set(&parts.join(""))
}

/// 整句去首尾空白后是不是拟声 burst（1~6 个拟声词，别无其他）。
fn is_sfx_run(clause: &str, word_set: &std::collections::HashSet<String>) -> bool {
    let tokens: Vec<&str> = clause.split(|c| c == ' ' || c == '\t').filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() || tokens.len() > 6 {
        return false;
    }
    tokens.iter().all(|t| is_sfx_token(t, word_set))
}

fn is_clause_sep(c: char) -> bool {
    // 抄rp CLAUSE_SPLIT_RE /([\n.…—–]+)/：!/? 不算边界（burst 自带尾缀，is_sfx_token 剥）。
    matches!(c, '\n' | '.' | '…' | '—' | '–')
}

fn split_sfx(seg: &MessageSegment, word_set: &std::collections::HashSet<String>) -> Vec<MessageSegment> {
    if seg.kind != SegmentKind::Text && seg.kind != SegmentKind::Action {
        return vec![seg.clone()];
    }
    // 按子句边界切（保留分隔符），逐片判定。
    let mut pieces: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in seg.content.chars() {
        if is_clause_sep(c) {
            if !cur.is_empty() {
                pieces.push(cur.clone());
                cur.clear();
            }
            pieces.push(c.to_string());
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        pieces.push(cur);
    }
    let mut out: Vec<MessageSegment> = Vec::new();
    for piece in pieces {
        let trimmed = piece.trim();
        let is_sep = piece.chars().all(is_clause_sep) && !piece.is_empty();
        if !is_sep && trimmed.len() >= 2 && is_sfx_run(trimmed, word_set) {
            let start = piece.find(trimmed).unwrap_or(0);
            let lead = &piece[..start];
            let trail = &piece[start + trimmed.len()..];
            if !lead.is_empty() {
                out.push(MessageSegment { kind: seg.kind, content: lead.to_string() });
            }
            out.push(MessageSegment { kind: SegmentKind::Sfx, content: trimmed.to_string() });
            if !trail.is_empty() {
                out.push(MessageSegment { kind: seg.kind, content: trail.to_string() });
            }
        } else {
            out.push(MessageSegment { kind: seg.kind, content: piece });
        }
    }
    out
}

fn coalesce(segments: Vec<MessageSegment>) -> Vec<MessageSegment> {
    let mut out: Vec<MessageSegment> = Vec::new();
    for seg in segments {
        if let Some(last) = out.last_mut() {
            if last.kind == seg.kind && seg.kind != SegmentKind::Sfx {
                last.content.push_str(&seg.content);
                continue;
            }
        }
        out.push(seg);
    }
    out
}

fn split_base(normalized: &str) -> Vec<MessageSegment> {
    let mut base: Vec<MessageSegment> = Vec::new();
    // 用 byte 下标走，逻辑用 chars。
    let bytes = normalized.as_bytes();
    let nb = bytes.len();
    let mut bi = 0usize;
    let mut bcursor = 0usize;
    while bi < nb {
        let c = normalized[bi..].chars().next().unwrap();
        if c == '*' {
            // 开头 1~3 个星
            let mut open = 0usize;
            while bi + open < nb && normalized[bi + open..].starts_with('*') && open < 3 {
                open += 1;
            }
            // 找同行内的闭合
            let rest = &normalized[bi + open..];
            let line_end = rest.find('\n').map(|p| bi + open + p).unwrap_or(nb);
            let mut found: Option<(usize, usize)> = None;
            let mut k = bi + open;
            while k < line_end {
                if normalized[k..].starts_with('*') {
                    let mut close = 0usize;
                    while k + close < line_end && normalized[k + close..].starts_with('*') && close < 3 {
                        close += 1;
                    }
                    let inner = &normalized[bi + open..k];
                    if !inner.is_empty() && !inner.contains('*') && !inner.contains('\n') {
                        found = Some((k, close));
                        break;
                    }
                    k += close.max(1);
                } else {
                    k += normalized[k..].chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
                }
            }
            if let Some((k, close)) = found {
                if bi > bcursor {
                    base.push(MessageSegment { kind: SegmentKind::Text, content: normalized[bcursor..bi].to_string() });
                }
                let inner = normalized[bi + open..k].to_string();
                base.push(MessageSegment { kind: SegmentKind::Action, content: inner });
                bi = k + close;
                bcursor = bi;
                continue;
            }
            bi += c.len_utf8();
        } else if c == '"' {
            let rest = &normalized[bi + 1..];
            let line_end = rest.find('\n').map(|p| bi + 1 + p).unwrap_or(nb);
            if let Some(rel) = normalized[bi + 1..line_end].find('"') {
                let end = bi + 1 + rel + 1;
                if bi > bcursor {
                    base.push(MessageSegment { kind: SegmentKind::Text, content: normalized[bcursor..bi].to_string() });
                }
                base.push(MessageSegment { kind: SegmentKind::Quote, content: normalized[bi..end].to_string() });
                bi = end;
                bcursor = bi;
                continue;
            }
            bi += 1;
        } else {
            bi += c.len_utf8();
        }
    }
    if bcursor < nb {
        base.push(MessageSegment { kind: SegmentKind::Text, content: normalized[bcursor..].to_string() });
    }
    base
}

/// RP 消息切四段（text/action/quote/sfx），渲染唯一真相源。
pub fn split_message_segments(text: &str, sfx: Option<&SfxConfig>) -> Vec<MessageSegment> {
    let normalized = normalize_rp_markup(text);
    let base = split_base(&normalized);
    match sfx {
        None => coalesce(base),
        Some(cfg) => {
            if cfg.disabled {
                return coalesce(base);
            }
            let word_set = sfx_word_set(&cfg.extra_words);
            coalesce(base.into_iter().flat_map(|seg| split_sfx(&seg, &word_set)).collect())
        }
    }
}

/// 不定冠词（抄rp text/article.ts）：展示启发式，宁缺勿错。
/// 有限定词/数字开头/复数结尾→None；silent h→an；辅音音vowel→a。
pub fn indefinite_article_for(phrase: &str) -> Option<&'static str> {
    let first = phrase.split_whitespace().next()?;
    if first.is_empty() {
        return None;
    }
    let bare: String =
        first.chars().filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '-').collect();
    if bare.is_empty() {
        return None;
    }
    let low = bare.to_lowercase();
    for d in ["a", "an", "the", "some", "one", "two", "three", "four", "five", "six", "my", "your", "his", "her", "their", "our", "this", "that", "these", "those"] {
        if low == d {
            return None;
        }
    }
    if bare.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        return None;
    }
    let words: Vec<&str> = phrase.split_whitespace().collect();
    if let Some(last_raw) = words.last() {
        let last: String =
            last_raw.chars().filter(|c| c.is_alphanumeric() || *c == '\'' || *c == '-').collect();
        let ll = last.to_lowercase();
        if ll.ends_with('s')
            && !(ll.ends_with("ss") || ll.ends_with("us") || ll.ends_with("is") || ll.ends_with("as") || ll.ends_with("os"))
        {
            return None;
        }
    }
    if low.starts_with("hour") || low.starts_with("honest") || low.starts_with("honor") || low.starts_with("honour") || low.starts_with("heir") {
        return Some("an");
    }
    for p in ["uni", "use", "user", "usual", "euro", "eu", "ubiquit", "one", "once"] {
        if low.starts_with(p) {
            // rp CONSONANT_INITIAL_VOWEL 是前缀启发式；uni后跟元音+n除外（union类）——此处从简按前缀判a。
            return Some("a");
        }
    }
    if "aeiou".contains(low.chars().next().unwrap_or(' ')) {
        return Some("an");
    }
    Some("a")
}

/// 带冠词短语（无冠词原样返回）。
pub fn with_indefinite_article(phrase: &str) -> String {
    let t = phrase.trim();
    match indefinite_article_for(t) {
        Some(a) => format!("{a} {t}"),
        None => t.to_string(),
    }
}

/// 单条消息的 SFX 策略：全局开关 + 说话人自己的拟声词（抄rp sfxConfigFor）。
/// role=="char" 才带说话人词；user 只有全局词；关了回 disabled。
pub fn sfx_config_for(
    role: &str,
    speaker_id: Option<&str>,
    enabled: bool,
    global_words: &[String],
    primary_id: Option<&str>,
    primary_words: &[String],
    participants: &[(String, Vec<String>)],
) -> Option<SfxConfig> {
    if !enabled {
        return Some(SfxConfig { disabled: true, extra_words: vec![] });
    }
    let mut words: Vec<String> = global_words.to_vec();
    if role == "char" {
        let own: Option<&[String]> = match speaker_id {
            Some(sid) => participants
                .iter()
                .find(|(id, _)| id == sid)
                .map(|(_, w)| w.as_slice())
                .or(Some(primary_words)),
            None => Some(primary_words),
        };
        let _ = primary_id;
        if let Some(w) = own {
            if !w.is_empty() {
                words.extend(w.iter().cloned());
            }
        }
    }
    if words.is_empty() {
        return None;
    }
    Some(SfxConfig { disabled: false, extra_words: words })
}

/// 通用slug（抄rp slugifyId；小写连字符40截断+去重后缀）。
pub fn slugify_id(label: &str, existing: &[String]) -> String {
    let mut base: String = label.trim().to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    while base.contains("--") { base = base.replace("--","-"); }
    base = base.trim_matches('-').to_string();
    if base.len() > 40 { base.truncate(40); }
    base = base.trim_matches('-').to_string();
    if base.is_empty() { base = "item".into(); }
    if !existing.iter().any(|e| e==&base) { return base; }
    let mut i = 2;
    loop { let c = format!("{base}-{i}"); if !existing.iter().any(|e| e==&c) { return c; } i+=1; }
}
/// 正则脚本批量跑（抄rp applyRegexScripts；坏pattern跳过，target过滤）。
pub fn apply_regex_scripts_simple(text: &str, scripts: &[(bool, String, String, String, String)]) -> String {
    let mut out = text.to_string();
    for (enabled, target, find, flags, replace) in scripts {
        if !enabled { continue; }
        if target!="both" && target!="display" && target!="prompt" { continue; }
        let pat = match regex::Regex::new(if flags.is_empty() { find.clone() } else { format!("(?{flags}){find}") }.as_str()) { Ok(r)=>r, Err(_)=>continue };
        let rep = replace.replace("\\n","\n").replace("\\t","\t");
        out = pat.replace_all(&out, rep.as_str()).into_owned();
    }
    out
}

/// 后端错误体（抄rp KoboldApiError；message+可选status）。
#[derive(Debug, Clone)]
pub struct BackendApiError { pub message: String, pub status: Option<i32> }
impl BackendApiError {
    pub fn new(message: &str, status: Option<i32>) -> Self { Self { message: message.to_string(), status } }
}
impl std::fmt::Display for BackendApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "BackendApiError({:?}): {}", self.status, self.message) }
}
/// 单chat串行锁（抄rp generationLock；begin/end/held，同步防同tick双领）。
pub struct GenLock { held: bool }
impl GenLock {
    pub fn new() -> Self { Self { held: false } }
    pub fn begin(&mut self) -> bool { if self.held { false } else { self.held = true; true } }
    pub fn end(&mut self) { self.held = false; }
    pub fn held(&self) -> bool { self.held }
}
/// token粗估（抄rp estimateTokens；~4字符/token，后端失联兜底）。
pub fn estimate_tokens_fallback(text: &str) -> usize { text.chars().count().div_ceil(4) }

/// regex脚本校验（抄rp isValidRegexScript；空find/坏正则=false，其余true）。
pub fn is_valid_regex_script(find: &str, flags: &str) -> bool {
    if find.is_empty() { return false; }
    if flags.chars().any(|c| !"gimsuy".contains(c)) { return false; }
    regex::Regex::new(find).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markup_idempotent() {
        let a = normalize_rp_markup("**hi** and <i>go</i><br>bye");
        let b = normalize_rp_markup(&a);
        assert_eq!(a, b);
        assert!(a.contains("*hi*") || a.contains("hi"));
    }
    #[test]
    fn segments_quote_not_sfx() {
        let segs = split_message_segments("*she runs* \"BOOM\" BOOM", None);
        assert!(segs.iter().any(|s| s.kind == SegmentKind::Action));
        assert!(segs.iter().any(|s| s.kind == SegmentKind::Quote));
    }
    #[test]
    fn sfx_burst() {
        // 抄rp语义：整片子句全是拟声才成 burst；"BOOM! then runs" 含实词不成。
        let cfg = SfxConfig { disabled: false, extra_words: vec![] };
        let segs = split_message_segments("she stops. BOOM! BOOM!", Some(&cfg));
        assert!(segs.iter().any(|s| s.kind == SegmentKind::Sfx));
        let inline = split_message_segments("she stops. BOOM! then runs", Some(&cfg));
        assert!(!inline.iter().any(|s| s.kind == SegmentKind::Sfx));
    }
    #[test]
    fn sfx_disabled() {
        let cfg = SfxConfig { disabled: true, extra_words: vec![] };
        let segs = split_message_segments("BOOM", Some(&cfg));
        assert!(!segs.iter().any(|s| s.kind == SegmentKind::Sfx));
    }
    #[test]
    fn word_list_parse() {
        let w = parse_sfx_word_list("bang,  crash\nx a-wooo");
        assert!(w.contains(&"bang".to_string()));
        assert!(!w.contains(&"x".to_string()));
    }
}

/// ST宏替换小件（抄rp characters/macros.ts substituteMacros；只收char/user/time/date/newline）。
pub fn substitute_macros(text: &str, char_name: &str, user_name: &str) -> String {
    if text.is_empty() { return text.to_string(); }
    let mut out = text.replace("{{char}}", char_name).replace("{{user}}", user_name);
    out = out.replace("{{time}}", "--:--").replace("{{date}}", "--").replace("{{newline}}", "\n");
    // 兼容带空格写法 {{ char }}
    out = out.replace("{{ char }}", char_name).replace("{{ user }}", user_name);
    out
}

/// 宽松 JSON（抄rp jsonRepair.ts 三级：直解→掏括号→修补线；修不好抛原文）。
pub fn parse_lenient_json(raw: &str) -> Result<serde_json::Value, String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) { return Ok(v); }
    let s = extract_braces(raw).ok_or_else(|| "No JSON object found".to_string())?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) { return Ok(v); }
    let r = repair_pipeline(&s);
    serde_json::from_str::<serde_json::Value>(&r).map_err(|e| e.to_string())
}
fn extract_braces(text: &str) -> Option<String> {
    let s = text.find('{')?;
    match text.rfind('}') {
        Some(e) if e > s => Some(text[s..=e].to_string()),
        _ => Some(text[s..].to_string()),
    }
}
fn repair_pipeline(json: &str) -> String {
    let mut out = json.replace('“', "\"").replace('”', "\"");
    // 补缺冒号："key" [...] → "key": [...]
    if let Ok(re) = regex::Regex::new(r#"("[^"\\]*(?:\\.[^"\\]*)*")\s*([\[{])"#) {
        out = re.replace_all(&out, "$1: $2").to_string();
    }
    // 去尾逗号
    if let Ok(re) = regex::Regex::new(r",(\s*[}\]])") {
        out = re.replace_all(&out, "$1").to_string();
    }
    // 补缺失闭合
    let mut br = 0i32; let mut bk = 0i32; let mut instr = false; let mut esc = false;
    for ch in out.chars() {
        if esc { esc = false; continue; }
        if ch == '\\' { esc = true; continue; }
        if ch == '"' { instr = !instr; continue; }
        if instr { continue; }
        match ch { '{' => br += 1, '}' => br -= 1, '[' => bk += 1, ']' => bk -= 1, _ => {} }
    }
    if instr { out.push('"'); }
    for _ in 0..bk.max(0) { out.push(']'); }
    for _ in 0..br.max(0) { out.push('}'); }
    out
}
/// 主题色互转（抄rp color.ts tripletToHex/hexToTriplet；"R G B"↔#rrggbb）。
pub fn triplet_to_hex(triplet: &str) -> String {
    let mut n = [0u8; 3];
    for (i, p) in triplet.split_whitespace().take(3).enumerate() {
        n[i] = p.parse::<i32>().unwrap_or(0).clamp(0, 255) as u8;
    }
    format!("#{:02x}{:02x}{:02x}", n[0], n[1], n[2])
}
pub fn hex_to_triplet(hex: &str) -> String {
    let h = hex.trim_start_matches('#');
    let v = |a: usize, b: usize| u8::from_str_radix(h.get(a..b).unwrap_or("0"), 16).unwrap_or(0);
    format!("{} {} {}", v(0, 2), v(2, 4), v(4, 6))
}

// ---------------------------------------------------------------------------
// 截断 JSON 修复（吸收自 dramatica-flow core/llm/__init__.py _repair_truncated_json，MIT）。
// 纪律（抄源）：
// - 先扫「字符串中途截断」：in_str 时截到最后一个完整 } / ]；
// - 再按括号栈补全未闭合的 { } [ ]；
// - 末尾悬空逗号 / 冒号先剥掉。
// 与 parse_lenient_json 三级管线互补：本函数处理「JSON 整体被 max_tokens 截断」，
// parse_lenient_json 处理「带围栏/前缀的宽松输出」。
// ---------------------------------------------------------------------------

/// 尝试修复被 max_tokens 截断的 JSON 字符串：补全未闭合括号、剥尾逗号。
/// 修不好时原样返回（调用方再决定抛错还是兜底）。
pub fn repair_truncated_json(text: &str) -> String {
    let mut t = text.trim().to_string();
    // 1) 字符串中途截断 → 截到最后一个完整对象
    let bytes: Vec<char> = t.chars().collect();
    let mut in_str = false;
    let mut esc = false;
    let mut last_valid: Option<usize> = None;
    for (i, &c) in bytes.iter().enumerate() {
        if esc {
            esc = false;
            continue;
        }
        if c == '\\' && in_str {
            esc = true;
            continue;
        }
        if c == '"' && !esc {
            in_str = !in_str;
            continue;
        }
        if in_str {
            continue;
        }
        if c == '}' || c == ']' {
            last_valid = Some(i);
        }
    }
    if in_str {
        if let Some(idx) = last_valid {
            t = bytes[..=idx].iter().collect();
        } else {
            return t; // 无法修复
        }
    }

    // 2) 括号栈（含字符串/转义跳过）
    let mut stack: Vec<char> = Vec::new();
    let mut in_str2 = false;
    let mut esc2 = false;
    for c in t.chars() {
        if esc2 {
            esc2 = false;
            continue;
        }
        if c == '\\' && in_str2 {
            esc2 = true;
            continue;
        }
        if c == '"' {
            in_str2 = !in_str2;
            continue;
        }
        if in_str2 {
            continue;
        }
        match c {
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                if stack.last() == Some(&c) {
                    stack.pop();
                }
            }
            _ => {}
        }
    }

    // 3) 末尾悬空逗号 / 冒号
    t = t.trim_end().to_string();
    while t.ends_with(',') || t.ends_with(':') {
        t.pop();
        t = t.trim_end().to_string();
    }

    // 4) 补全未闭合
    while let Some(c) = stack.pop() {
        t.push(c);
    }
    t
}
