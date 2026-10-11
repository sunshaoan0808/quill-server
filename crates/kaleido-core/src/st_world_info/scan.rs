//! st_world_info/scan —— 键匹配/评分/扫描缓冲（check_world_info 薄壳）。
#![allow(unused_imports)]

use super::*;

pub fn parse_regex_from_string(input: &str) -> Option<regex::Regex> {
    let input = input.trim();
    if !input.starts_with('/') {
        return None;
    }
    let last = input.rfind('/')?;
    if last == 0 {
        return None;
    }
    let pattern = &input[1..last];
    let flags = &input[last + 1..];
    // unescaped slash inside pattern → invalid for ST
    let mut escaped = false;
    for ch in pattern.chars() {
        if ch == '\\' {
            escaped = !escaped;
            continue;
        }
        if ch == '/' && !escaped {
            return None;
        }
        escaped = false;
    }
    let pattern = pattern.replace("\\/", "/");
    let mut builder = regex::RegexBuilder::new(&pattern);
    for f in flags.chars() {
        match f {
            'i' => {
                builder.case_insensitive(true);
            }
            'm' => {
                builder.multi_line(true);
            }
            's' => {
                builder.dot_matches_new_line(true);
            }
            'u' => {}
            'g' | 'y' => {} // JS-only; rust always can re-search
            _ => {}
        }
    }
    builder.build().ok()
}

pub(crate) fn transform_case(s: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        s.to_string()
    } else {
        s.to_lowercase()
    }
}

pub(crate) fn match_keys(haystack: &str, needle: &str, entry: &WiEntry, settings: &WiSettings) -> bool {
    let needle = needle.trim();
    if needle.is_empty() {
        return false;
    }
    if let Some(re) = parse_regex_from_string(needle) {
        return re.is_match(haystack);
    }
    let case_sensitive = entry.case_sensitive.unwrap_or(settings.case_sensitive);
    let match_whole = entry
        .match_whole_words
        .unwrap_or(settings.match_whole_words);
    let hay = transform_case(haystack, case_sensitive);
    let neo = transform_case(needle, case_sensitive);
    if match_whole {
        let words: Vec<&str> = neo.split_whitespace().collect();
        if words.len() > 1 {
            hay.contains(&neo)
        } else {
            // (?:^|\W)(needle)(?:$|\W)
            let re = regex::RegexBuilder::new(&format!(
                r"(?:^|\W)({})(?:$|\W)",
                regex::escape(&neo)
            ))
            .case_insensitive(false)
            .build();
            match re {
                Ok(r) => r.is_match(&hay),
                Err(_) => hay.contains(&neo),
            }
        }
    } else {
        hay.contains(&neo)
    }
}

pub(crate) fn estimate_tokens_mode(s: &str, mode: &str) -> i32 {
    let m = crate::TokenEstimateMode::parse(mode);
    crate::estimate_tokens(s, m)
}

/// Chat messages newest-first (ST convention for WI scan buffer).
pub fn chat_to_scan_buffer(messages: &[(String, String)]) -> Vec<String> {
    // messages as (role, content) oldest-first from API → reverse for depth buffer
    messages
        .iter()
        .rev()
        .map(|(role, content)| {
            if role == "user" || role == "assistant" || role == "system" {
                content.trim().to_string()
            } else {
                content.trim().to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .collect()
}

pub(crate) fn buffer_text(depth_buf: &[String], depth: i32, recurse: &[String]) -> String {
    let d = depth.max(0) as usize;
    let slice = if d == 0 {
        &depth_buf[..0]
    } else if d >= depth_buf.len() {
        &depth_buf[..]
    } else {
        &depth_buf[..d]
    };
    let mut parts: Vec<&str> = slice.iter().map(|s| s.as_str()).collect();
    for r in recurse {
        parts.push(r.as_str());
    }
    parts.join("\n")
}

pub(crate) fn primary_match(
    entry: &WiEntry,
    text: &str,
    settings: &WiSettings,
) -> Option<String> {
    for key in &entry.keys {
        if match_keys(text, key, entry, settings) {
            return Some(key.clone());
        }
    }
    None
}

pub(crate) fn secondary_match(entry: &WiEntry, text: &str, settings: &WiSettings) -> bool {
    if !entry.selective || entry.keysecondary.is_empty() {
        return true;
    }
    let mut any = false;
    let mut all = true;
    for key in &entry.keysecondary {
        let m = match_keys(text, key, entry, settings);
        if m {
            any = true;
        } else {
            all = false;
        }
        match entry.selective_logic {
            SelectiveLogic::AndAny if m => return true,
            SelectiveLogic::NotAll if !m => return true,
            _ => {}
        }
    }
    match entry.selective_logic {
        SelectiveLogic::NotAny => !any,
        SelectiveLogic::AndAll => all,
        SelectiveLogic::AndAny => any,
        SelectiveLogic::NotAll => !all,
    }
}

pub(crate) fn score_entry(entry: &WiEntry, text: &str, settings: &WiSettings) -> i32 {
    let mut score = 0;
    for k in &entry.keys {
        if match_keys(text, k, entry, settings) {
            score += 1;
        }
    }
    for k in &entry.keysecondary {
        if match_keys(text, k, entry, settings) {
            score += 1;
        }
    }
    score
}

/// Core ST-compatible scan.
pub fn check_world_info(
    entries: &[WiEntry],
    chat_newest_first: &[String],
    max_context_tokens: i32,
    settings: &WiSettings,
) -> WiScanResult {
    check_world_info_timed(
        entries,
        chat_newest_first,
        max_context_tokens,
        settings,
        None,
        false,
        None,
    )
}
