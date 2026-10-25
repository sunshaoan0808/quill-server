//! voice2.rs — TTS 前文本清洗 + 流式分句（吸收自 pnotisdev/rp voice/speakableText.ts + voice/sentenceChunker.ts，MIT）。
//!
//! 纪律（抄rp）：
//! - *动作* 和 markdown 朗读出来是灾难，先剥掉再送 TTS。
//! - 分句增量抽：流式到达一段切一段，consumed cursor 回传，播过的不重抽。
//! - 全纯函数，零迁移。

/// 去掉 *动作* / markdown 符号 / 多余空白，剩可朗读文本。
pub fn to_speakable_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_star = false;
    for ch in text.chars() {
        if ch == '*' {
            in_star = !in_star;
            out.push(' ');
            continue;
        }
        if in_star {
            continue;
        }
        if matches!(ch, '_' | '~' | '`' | '#') {
            continue;
        }
        out.push(ch);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 句界：中英终点标点 + 换行。返回 (chunks, new_consumed)。
pub fn extract_complete_sentences(full: &str, consumed: usize) -> (Vec<String>, usize) {
    let rest = match full.get(consumed..) {
        Some(r) => r,
        None => return (Vec::new(), consumed),
    };
    let mut chunks: Vec<String> = Vec::new();
    let mut last_end = 0usize;
    let chars: Vec<char> = rest.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let is_boundary = matches!(c, '.' | '!' | '?' | '…' | '。' | '！' | '？')
            || c == '\n';
        if is_boundary {
            let mut end = i + 1;
            while end < chars.len()
                && matches!(chars[end], '"' | '\'' | ')' | ']' | '”' | '’' | '*' | ')')
            {
                end += 1;
            }
            while end < chars.len() && chars[end] == ' ' {
                end += 1;
            }
            let s: String = chars[last_end..end].iter().collect();
            let t = s.trim();
            if !t.is_empty() {
                chunks.push(t.to_string());
            }
            last_end = end;
            i = end;
            continue;
        }
        i += 1;
    }
    // last_end 是 char 数，转回 byte 数
    let byte_end: usize = rest
        .char_indices()
        .nth(last_end)
        .map(|(b, _)| b)
        .unwrap_or(rest.len());
    (chunks, consumed + byte_end)
}

/// RP散文词数（抄rp voice.ts countProseWords；去*/引号后按空白切）。
pub fn count_prose_words(text: &str) -> usize {
    text.replace(['*','"','“','”'], " ").split_whitespace().filter(|w| w.chars().any(|c| c.is_alphanumeric())).count()
}
/// 档位字数口径（抄rp BANDS words=45/95/175）。
pub fn band_words(band: &str) -> usize { match band { "brief"=>45, "detailed"=>175, _=>95 } }
/// 开场白折扣0.6（抄rp deriveCardReplyBand greeting*0.6）。
pub fn discounted_greeting_words(greeting_words: usize) -> usize { (greeting_words as f64 * 0.6).round() as usize }

/// 本轮max_length硬顶（抄rp replyMaxTokens；words*1.6*1.5，不低于48）。
pub fn reply_max_tokens(band: &str, user_max: i64) -> i64 {
    let cap = ((band_words(band) as f64 * 1.6 * 1.5).ceil() as i64).max(48);
    cap.min(user_max)
}
/// 句节奏白话（抄rp detectVoiceFingerprint rhythm段；≤6短/≥16长）。
pub fn sentence_rhythm_note(avg_words: f64) -> &'static str {
    if avg_words <= 6.0 { "Short, clipped sentences." } else if avg_words >= 16.0 { "Long, winding sentences." } else { "Medium-length, even sentences." }
}

/// 回复长度档标签（抄rp REPLY_LENGTH_LABELS；auto按卡示例量体）。
pub const REPLY_LENGTH_LABELS: [(&str, &str); 4] = [("auto","Match their example dialogue"),("brief","Brief"),("moderate","Moderate"),("detailed","Detailed")];
/// 档位字数（抄rp BANDS words=45/95/175；auto切点55/130，中位数防单例拖档）。
pub fn band_for_words(words: usize) -> &'static str { if words <= 55 { "brief" } else if words <= 130 { "moderate" } else { "detailed" } }
/// 人设侧写行（抄rp buildCharacterProfileNote；生活+规则+声纹三拼，无料=None）。
pub fn build_character_profile_note(has_life: bool, has_rules: bool, has_voice: bool) -> bool { has_life || has_rules || has_voice }
/// 声纹提醒（抄rp buildVoiceFingerprintReminder；首catchphrase/tic复读，防长聊磨平）。
pub fn voice_reminder(catchphrase: Option<&str>, tic: Option<&str>, register: Option<&str>) -> Option<String> {
    let mut bits = vec![];
    if let Some(c) = catchphrase { if !c.trim().is_empty() { bits.push(format!("reach for \"{}\" again when it fits", c.trim())); } }
    if let Some(t) = tic { if !t.trim().is_empty() { bits.push(format!("keep the \"{}\" tic alive", t.trim())); } }
    if bits.is_empty() { if let Some(r) = register { return Some(format!("Voice check: {r}.")); } return None; }
    Some(format!("Voice check, every reply: {}.", bits.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speakable_strips_actions() {
        assert_eq!(to_speakable_text("*她笑* 你好 #标题"), "你好 标题");
        assert_eq!(to_speakable_text("“我 *真的* 是。”"), "“我 是。”");
    }
    #[test]
    fn sentence_streaming() {
        let full = "第一句。第二句！第三句没写完";
        let (c1, n1) = extract_complete_sentences(full, 0);
        assert_eq!(c1.len(), 2);
        let (c2, n2) = extract_complete_sentences(full, n1);
        assert!(c2.is_empty());
        assert_eq!(n2, n1);
        let full2 = format!("{full}了。");
        let (c3, _) = extract_complete_sentences(&full2, n1);
        assert_eq!(c3.len(), 1);
    }
}

/// 回复长度档（抄rp characters/voice.ts BANDS；auto 由调用方按例文词数先判档）。
pub fn reply_length_instruction(band: &str) -> &'static str {
    match band {
        "brief" => "Length: keep this turn to one to three sentences. Lead with what they say or do; at most one short action beat.",
        "detailed" => "Length: two short paragraphs at most. Every sentence has to carry something new; cut anything that only restates the mood.",
        _ => "Length: keep this turn to one short paragraph, around three to five sentences.",
    }
}
pub const AUTO_BRIEF_MAX_WORDS: usize = 55;
pub const AUTO_MODERATE_MAX_WORDS: usize = 130;
/// 例文词数→档（抄rp deriveCardReplyBand 切点）。
pub fn reply_band_for_words(words: usize) -> &'static str {
    if words <= AUTO_BRIEF_MAX_WORDS { "brief" } else if words <= AUTO_MODERATE_MAX_WORDS { "moderate" } else { "detailed" }
}
