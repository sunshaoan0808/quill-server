//! slop2.rs — AI腔检测 + 输出清洗（吸收自 pnotisdev/rp text/slop.ts，MIT）。
//!
//! 纪律（抄rp）：
//! - SLOP_PATTERNS 只做 steering，不原地改写；clean 只删“绝不可能是角色说话”的东西。
//! - find_slop 跨多轮计数；repeated 只报最长形；avoidance 有话才说（clean时返回None）。
//! - 全纯函数，零迁移。

/// 一条可识别的AI腔。
pub struct SlopPattern {
    pub id: &'static str,
    pub label: &'static str,
    pub re: &'static str,
}

/// 语料（抄rp SLOP_PATTERNS；Rust regex 用 (?i) 前缀；`['’]` 保留原文写法）。
pub const SLOP_PATTERNS: &[SlopPattern] = &[
    SlopPattern { id: "couldnt-help", label: "couldn't help but", re: r"(?i)\bcould ?n['’]?t help but\b" },
    SlopPattern { id: "mix-of", label: "\"a mix of X and Y\" for an expression", re: r"(?i)\ba mix(ture)? of [a-z\s]{3,30} and [a-z\s]{3,30}" },
    SlopPattern { id: "despite-herself", label: "\"despite herself/himself/themselves\"", re: r"(?i)\bdespite (her|him|them)self\b" },
    SlopPattern { id: "ghost-of-smile", label: "\"a ghost of a smile\"", re: r"(?i)\b(a|the) ghost of (a|her|his|their) (smile|grin|smirk)\b" },
    SlopPattern { id: "ghost-of-touch", label: "\"the ghost of a touch\"", re: r"(?i)\b(a|the) ghost of (a|her|his|their) (touch|breath|laugh)\b" },
    SlopPattern { id: "barely-whisper", label: "\"voice barely above a whisper\"", re: r"(?i)\b(barely|scarcely|no louder than) (a|above a) whisper\b" },
    SlopPattern { id: "shiver-down", label: "\"sent a shiver down her spine\"", re: r"(?i)\b(sent|sending) (a|an) (shiver|shudder|jolt|spark|thrill) (down|through|up)\b" },
    SlopPattern { id: "heart-hammering", label: "\"heart hammering/pounding in her chest\"", re: r"(?i)\b(heart|pulse) (hammer|pound|thunder|thud|race)(ing|ed|s)? (in|against|inside) (her|his|their|its) (chest|ribs|throat)\b" },
    SlopPattern { id: "breath-didnt-know", label: "\"a breath she didn't know she was holding\"", re: r"(?i)\b(a |the )?breath (she|he|they) did ?n['’]?t (even )?(know|realise|realize) (she|he|they) (was|were) holding\b" },
    SlopPattern { id: "air-thick-with", label: "\"the air was thick with\"", re: r"(?i)\b(the )?air (was|felt|hung|grew|turned) (thick|heavy|charged|electric)\b" },
    SlopPattern { id: "silence-stretched", label: "\"the silence stretched\"", re: r"(?i)\b(the )?silence (stretch|linger|hang|hung|drag)(ed|ing|s)?\b" },
    SlopPattern { id: "unreadable", label: "\"an unreadable expression\"", re: r"(?i)\b(an?|her|his|their) (unreadable|inscrutable|indecipherable) (expression|look|gaze|face)\b" },
    SlopPattern { id: "something-flickered", label: "\"something flickered in her eyes\"", re: r"(?i)\bsomething (flicker|flash|shift|dance|glint)(ed|ing|s)? (in|across|behind|through)\b" },
    SlopPattern { id: "eyes-darkened", label: "\"her eyes darkened\"", re: r"(?i)\b(her|his|their) (eyes|gaze) (darken|soften|harden)(ed|s|ing)?\b" },
    SlopPattern { id: "smile-didnt-reach", label: "\"a smile that didn't reach her eyes\"", re: r"(?i)\b(smile|grin) that did ?n['’]?t (quite )?reach (her|his|their) eyes\b" },
    SlopPattern { id: "dangerously-low", label: "\"voice dangerously low\"", re: r"(?i)\bvoice (dropp?ing|dropped|going|went|low(er)?)(,)? (dangerous(ly)?|deceptive(ly)?|impossibly) (low|soft|quiet)\b" },
    SlopPattern { id: "beat-passed", label: "\"a beat passed\"", re: r"(?i)\b(a|another) beat (passed|of silence|went by)\b" },
    SlopPattern { id: "first-time-in", label: "\"for the first time in a long time\"", re: r"(?i)\bfor the first time in (a )?(long time|years|forever)\b" },
    SlopPattern { id: "little-did", label: "\"little did she know\"", re: r"(?i)\blittle did (she|he|they|you|[A-Za-z]+) (know|suspect|realise|realize)\b" },
    SlopPattern { id: "unbeknownst", label: "\"unbeknownst to\"", re: r"(?i)\bunbeknownst to\b" },
    SlopPattern { id: "in-that-moment", label: "\"in that moment, she knew\"", re: r"(?i)\bin that moment,? (she|he|they|you) (knew|understood|realised|realized)\b" },
    SlopPattern { id: "not-just-but", label: "\"it's not just X, it's Y\" phrasing", re: r"(?i)\bnot (just|only|merely) [^.,;!?]{2,40}[,;]? (but|it['’]s|they['’]re|she['’]s|he['’]s) " },
    SlopPattern { id: "more-than-just", label: "\"more than just\"", re: r"(?i)\bmore than (just|merely|simply) (a|an|the)?\b" },
    SlopPattern { id: "seemed-to", label: "\"seemed to\" / \"appeared to\" hedging in narration", re: r"(?i)\b(seemed|appeared) to (be|have|know|understand|sense|want|need)\b" },
    SlopPattern { id: "orbs", label: "\"orbs\" for eyes", re: r"(?i)\b(her|his|their|the) (emerald|sapphire|amber|violet|azure|obsidian|crimson|golden|dark|bright)? ?orbs\b" },
    SlopPattern { id: "ministrations", label: "\"ministrations\"", re: r"(?i)\bministrations\b" },
    SlopPattern { id: "pools-of", label: "\"pools of\" for eyes", re: r"(?i)\bpools of (liquid |molten |dark |warm )?[a-z]{3,12}\b" },
    SlopPattern { id: "electricity", label: "\"electricity shot through\"", re: r"(?i)\b(electricity|a current|fire|heat) (shot|surged|coursed|raced) (through|down|up)\b" },
    SlopPattern { id: "every-fibre", label: "\"every fibre of her being\"", re: r"(?i)\bevery fib(re|er) of (her|his|their|my) being\b" },
    SlopPattern { id: "tapestry", label: "\"a tapestry of\"", re: r"(?i)\ba tapestry of\b" },
    SlopPattern { id: "testament", label: "\"a testament to\"", re: r"(?i)\ba testament to\b" },
    SlopPattern { id: "symphony", label: "\"a symphony of\"", re: r"(?i)\ba symphony of\b" },
    SlopPattern { id: "dance-of", label: "\"a delicate dance of\"", re: r"(?i)\b(a|the) (delicate|intricate|careful) dance of\b" },
    SlopPattern { id: "palpable", label: "\"palpable\"", re: r"(?i)\bpalpable\b" },
    SlopPattern { id: "only-time-will-tell", label: "\"only time will tell\"", re: r"(?i)\bonly time (will|would) tell\b" },
    SlopPattern { id: "rest-is-history", label: "\"and the rest is history\"", re: r"(?i)\bthe rest (is|was) history\b" },
    SlopPattern { id: "what-happens-next", label: "\"whatever happens next\" scene-closing filler", re: r"(?i)\bwhat(ever)? (happens|comes|came) next\b" },
    SlopPattern { id: "one-thing-certain", label: "\"one thing was certain\"", re: r"(?i)\bone thing (was|is) (certain|for sure|clear)\b" },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlopHit {
    pub id: String,
    pub label: String,
    pub count: usize,
}

/// 单文本命中（带计数）。
pub fn find_slop(text: &str) -> Vec<SlopHit> {
    find_slop_across(&[text])
}

/// 跨多文本计数（output 按 count 倒序）。
pub fn find_slop_across(texts: &[&str]) -> Vec<SlopHit> {
    let mut hits: Vec<SlopHit> = vec![];
    for p in SLOP_PATTERNS {
        let mut count = 0usize;
        let Ok(re) = regex::Regex::new(p.re) else { continue };
        for t in texts {
            if t.is_empty() { continue; }
            count += re.find_iter(t).count();
        }
        if count > 0 {
            hits.push(SlopHit { id: p.id.to_string(), label: p.label.to_string(), count });
        }
    }
    hits.sort_by(|a, b| b.count.cmp(&a.count));
    hits
}

/// 回声判定：只看上一条；短句直接放过，1-2词大写标签可剥（抄rp isVerbatimEcho）。
pub fn is_verbatim_echo(text: &str, prior: Option<&str>) -> bool {
    let prior = prior.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let Some(p) = prior else { return false };
    let t = text.trim();
    if t == p { return true; }
    // 剥掉 "Kai: " 形前缀再比一次。
    let stripped = strip_leading_speaker_label(t);
    stripped == p
}

fn strip_leading_speaker_label(t: &str) -> String {
    let s = t.trim_start();
    // 1-2 个大写开头词 + 冒号。
    let Ok(re) = regex::Regex::new(r"^[A-Z][A-Za-z'-]*(?:\s[A-Z][A-Za-z'-]*)?\s*:\s*") else {
        return t.trim().to_string();
    };
    re.replace(s, "").to_string()
}

/// 复述下限：双方都要≥40字才比（抄rp DUPLICATE_MIN_LENGTH）。
pub const DUPLICATE_MIN_LENGTH: usize = 40;

fn normalize_for_dupe(text: &str) -> String {
    text.trim().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 双向子串复述（抄rp isDuplicateOfRecentText）。
pub fn is_duplicate_of_recent(candidate: &str, recent: &[&str]) -> bool {
    let c = normalize_for_dupe(candidate);
    if c.len() < DUPLICATE_MIN_LENGTH { return false; }
    for r in recent {
        if r.is_empty() { continue; }
        let o = normalize_for_dupe(r);
        if o.len() < DUPLICATE_MIN_LENGTH { continue; }
        if c.contains(&o) || o.contains(&c) { return true; }
    }
    false
}

fn odd_count(text: &str, mark: char) -> bool {
    text.chars().filter(|c| *c == mark).count() % 2 == 1
}

/// 收尾是否干净：终结标点 + `*`/`"` 成对（抄rp endsCleanly）。
pub fn ends_cleanly(text: &str) -> bool {
    let t = text.trim_end();
    if t.is_empty() { return true; }
    if odd_count(t, '*') || odd_count(t, '"') { return false; }
    let tail_ok = t.ends_with(['.', '!', '?', '…']) 
        || t.ends_with(['"', '\'', '’', '”', '*', ')', ']']);
    // 必须有终结标点，或以引号/星号收尾（rp 语义）。
    if tail_ok {
        // 粗判：结尾含标点或引号星号即算干净（`*"’]$` 分支）。
        let has_term = t.contains(['.', '!', '?', '…']);
        return has_term || t.ends_with(['*', '"', '”', '’']);
    }
    false
}

/// 补被 stop 截断的半个 markup（抄rp balanceTrailingMarkup）。
pub fn balance_trailing_markup(text: &str) -> String {
    let mut out = text.trim_end().to_string();
    for mark in ['"', '*'] {
        if !odd_count(&out, mark) { continue; }
        let m = format!("{mark}");
        if out.ends_with(&m) || out.ends_with(&format!("{m} ")) {
            // 孤零结尾：删掉。
            out = out.trim_end_matches(|c: char| c == mark || c.is_whitespace()).to_string();
        } else {
            out.push(mark);
        }
    }
    out
}

/// 截到最后完整句；丢太多(>35%)就原样返回（抄rp trimToLastSentence）。
pub fn trim_to_last_sentence(text: &str, max_loss_ratio: f64) -> String {
    let trimmed = text.trim_end();
    if trimmed.is_empty() { return text.to_string(); }
    if ends_cleanly(trimmed) { return trimmed.to_string(); }
    // 贪婪到最后一个终结标点（含尾随引号/括号/星号）。
    let Ok(re) = regex::Regex::new(r#"(?s)[\s\S]*[.!?…]["'’”*)\]]*"#) else {
        return text.to_string();
    };
    let Some(m) = re.find(trimmed) else { return text.to_string(); };
    let cut = m.as_str().trim_end();
    if cut.is_empty() { return text.to_string(); }
    if (trimmed.len() - cut.len()) as f64 / trimmed.len() as f64 > max_loss_ratio {
        return text.to_string();
    }
    cut.to_string()
}

/// 确定性清洗（抄rp cleanModelOutput 顺序：turn标记→meta行→标题→空行→孤星）。
/// 入口只做与模型无关的后三件；说话人前缀/turn截断由调用方按需先做（text2 侧已有）。
pub fn clean_model_output_basic(text: &str) -> String {
    if text.is_empty() { return text.to_string(); }
    let mut out = text.to_string();
    // markdown 标题：RP 正文没有标题。
    if let Ok(re) = regex::Regex::new(r"(?m)^[ \t]*#{1,6}[ \t]+") {
        out = re.replace_all(&out, "").to_string();
    }
    if let Ok(re) = regex::Regex::new(r"\n{3,}") {
        out = re.replace_all(&out, "\n\n").to_string();
    }
    let stars = out.chars().filter(|c| *c == '*').count();
    if stars % 2 == 1 {
        if let Ok(re) = regex::Regex::new(r"\*\s*$") {
            if re.is_match(&out) {
                out = re.replace(&out, "").to_string();
            }
        }
    }
    // 花引号拉直，保证后段 quote 匹配只看一种字符。
    out = out.replace('‘', "'").replace('’', "'").replace('“', "\"").replace('”', "\"");
    out.trim().to_string()
}

/// 回看轮数（抄rp SLOP_SCAN_TURNS）。
pub const SLOP_SCAN_TURNS: usize = 6;

/// steering 行文：点名模型刚用过的腔 + 复述过的整句；干净返回None（抄rp buildSlopAvoidanceNote）。
pub fn build_slop_avoidance_note(
    recent_char_turns: &[&str],
    max_phrases: usize,
    max_repeats: usize,
) -> Option<String> {
    let texts: Vec<&str> = recent_char_turns
        .iter()
        .copied()
        .filter(|t| !t.trim().is_empty())
        .collect();
    let texts: Vec<&str> = texts[texts.len().saturating_sub(SLOP_SCAN_TURNS)..].to_vec();
    if texts.is_empty() { return None; }
    let slop = find_slop_across(&texts);
    let slop: Vec<&SlopHit> = slop.iter().take(max_phrases).collect();
    let repeats = find_repeated_phrases(&texts, 4, 8, 2, max_repeats);
    let mut lines: Vec<String> = vec![];
    if !slop.is_empty() {
        let labels: Vec<String> = slop.iter().map(|h| h.label.clone()).collect();
        lines.push(format!(
            "You have already leaned on these in this chat: {}. Do not use them again. Write the moment a different way.",
            labels.join("; ")
        ));
    }
    if !repeats.is_empty() {
        let qs: Vec<String> = repeats.iter().map(|(p, _)| format!("\"{p}\"")).collect();
        lines.push(format!(
            "You have also repeated these exact phrasings: {}. Say it differently or leave it out.",
            qs.join("; ")
        ));
    }
    if lines.is_empty() { None } else { Some(lines.join(" ")) }
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "be", "but", "by", "for", "from", "her", "his", "i", "if",
    "in", "is", "it", "its", "me", "my", "not", "of", "on", "or", "she", "so", "that",
    "the", "their", "them", "then", "they", "this", "to", "was", "were", "with", "you", "your",
];

fn normalise_phrase_words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphabetic() || c == ' ' { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// 跨轮复述短语：只报最长形，全停用词跳过（抄rp findRepeatedPhrases）。
/// 返回 (phrase, count)，按 count 倒序、同 count 长者先。
pub fn find_repeated_phrases(
    texts: &[&str],
    min_words: usize,
    max_words: usize,
    min_count: usize,
    limit: usize,
) -> Vec<(String, usize)> {
    use std::collections::{HashMap, HashSet};
    let mut counts: HashMap<String, usize> = HashMap::new();
    for text in texts {
        if text.is_empty() { continue; }
        let words = normalise_phrase_words(text);
        let mut seen: HashSet<String> = HashSet::new();
        for n in min_words..=max_words {
            if words.len() < n { continue; }
            for i in 0..=words.len() - n {
                let slice = &words[i..i + n];
                if slice.iter().all(|w| STOPWORDS.contains(&w.as_str())) { continue; }
                let phrase = slice.join(" ");
                if seen.contains(&phrase) { continue; }
                seen.insert(phrase.clone());
                *counts.entry(phrase).or_insert(0) += 1;
            }
        }
    }
    let mut repeated: Vec<(String, usize)> = counts
        .into_iter()
        .filter(|(_, c)| *c >= min_count)
        .collect();
    repeated.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.len().cmp(&a.0.len())));
    let mut out: Vec<(String, usize)> = vec![];
    for (phrase, count) in repeated {
        if out.iter().any(|(k, c)| *c == count && k.contains(&phrase)) { continue; }
        out.push((phrase, count));
        if out.len() >= limit { break; }
    }
    out
}

/// explicit套话旧文表（抄rp EXPLICIT_ANTI_PATTERN_ENTRIES；仅peak查，building不扰）。
pub const EXPLICIT_ANTI_PATTERN_ENTRIES: [&str; 4] = ["she couldn't help but moan","he couldn't help but groan","their bodies moved as one","a symphony of pleasure"];
/// 串台截断（抄rp truncateAtStrayTurnMarker；user:/char:串台行截断，保prompt不吞 чужой 回合）。
pub fn truncate_at_stray_turn_marker(text: &str) -> &str {
    for (i, line) in text.lines().enumerate() {
        let t = line.trim().to_lowercase();
        if i > 0 && (t.starts_with("user:") || t.starts_with("{{user}}:")) { return text.split_at(text.find(line).unwrap_or(text.len())).0; }
    }
    text
}
/// 近文复读（抄rp isDuplicateOfRecentText；长句归一化比对，短句不判）。
pub fn is_duplicate_of_recent_text(candidate: &str, recent: &[&str]) -> bool {
    let norm = |s: &str| s.trim().to_lowercase();
    let c = norm(candidate);
    if c.chars().count() < 12 { return false; }
    recent.iter().any(|r| norm(r) == c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slop_hits_obvious() {
        let hits = find_slop("She couldn't help but smile. The air was thick with tension.");
        assert!(hits.iter().any(|h| h.id == "couldnt-help"));
        assert!(hits.iter().any(|h| h.id == "air-thick-with"));
        assert!(find_slop("She said hello and left.").is_empty());
    }

    #[test]
    fn echo_and_dupe() {
        assert!(is_verbatim_echo("Kai: hello", Some("hello")));
        assert!(!is_verbatim_echo("Oh.", Some("Fine.")));
        let long = "the old lighthouse keeper told stories of the northern passage every winter night";
        assert!(is_duplicate_of_recent(long, &[long]));
        assert!(!is_duplicate_of_recent("Oh.", &[long]));
    }

    #[test]
    fn clean_tail() {
        assert!(ends_cleanly("She left."));
        assert!(!ends_cleanly("She left and"));
        assert_eq!(balance_trailing_markup("*She says it").contains('*'), true);
        assert!(build_slop_avoidance_note(&["plain hello there friend"], 4, 3).is_none());
    }

    #[test]
    fn repeats_maximal() {
        let t1 = "the old lighthouse keeper walked slowly toward the quiet harbor town";
        let t2 = "the old lighthouse keeper walked slowly toward the quiet harbor town again";
        let r = find_repeated_phrases(&[t1, t2], 4, 8, 2, 5);
        assert!(!r.is_empty());
    }
}
