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

/// R8 性能优化（2026-10-07）：匹配层改为预变换变体 ——
/// 旧实现每个 key 都把整个扫描缓冲 lowercase 一次 + whole-word 路径每 key
/// 现场编译正则；现在缓冲按 depth 缓存一份小写副本、词边界用字符级判定、
/// regex 键编译结果按趟缓存。行为与旧实现逐分支等价（regex 键仍对原文匹配）。

pub(crate) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `(?:^|\W)(needle)(?:$|\W)` 的字符级等价实现（needle 为字面量，非正则）。
pub(crate) fn whole_word_contains(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut start = 0usize;
    while let Some(rel) = hay[start..].find(needle) {
        let abs = start + rel;
        let before_ok =
            abs == 0 || !hay[..abs].chars().next_back().map(is_word_char).unwrap_or(false);
        let after = abs + needle.len();
        let after_ok =
            after >= hay.len() || !hay[after..].chars().next().map(is_word_char).unwrap_or(false);
        if before_ok && after_ok {
            return true;
        }
        start = abs + needle.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        if start >= hay.len() {
            break;
        }
    }
    false
}

/// 预变换匹配：`raw` 原文、`lower` 其小写副本（调用方按 depth 缓存），
/// regex 键对原文匹配（与旧实现一致），plain 键按大小写语义选择文本。
pub(crate) fn match_keys_pre(
    raw: &str,
    lower: &str,
    key: &str,
    entry: &WiEntry,
    settings: &WiSettings,
    regex_cache: &mut HashMap<String, Option<regex::Regex>>,
) -> bool {
    let needle = key.trim();
    if needle.is_empty() {
        return false;
    }
    let re_opt: Option<&regex::Regex> = if let Some(cached) = regex_cache.get(needle) {
        cached.as_ref()
    } else {
        let parsed = parse_regex_from_string(needle);
        regex_cache.insert(needle.to_string(), parsed);
        regex_cache.get(needle).and_then(|c| c.as_ref())
    };
    if let Some(re) = re_opt {
        return re.is_match(raw);
    }
    let case_sensitive = entry.case_sensitive.unwrap_or(settings.case_sensitive);
    let match_whole = entry.match_whole_words.unwrap_or(settings.match_whole_words);
    if case_sensitive {
        if match_whole {
            whole_word_contains(raw, needle)
        } else {
            raw.contains(needle)
        }
    } else {
        let neo = needle.to_lowercase();
        if match_whole {
            let words: Vec<&str> = neo.split_whitespace().collect();
            if words.len() > 1 {
                lower.contains(&neo)
            } else {
                whole_word_contains(lower, &neo)
            }
        } else {
            lower.contains(&neo)
        }
    }
}

pub(crate) fn primary_match_pre(
    entry: &WiEntry,
    raw: &str,
    lower: &str,
    settings: &WiSettings,
    regex_cache: &mut HashMap<String, Option<regex::Regex>>,
) -> Option<String> {
    for key in &entry.keys {
        if match_keys_pre(raw, lower, key, entry, settings, regex_cache) {
            return Some(key.clone());
        }
    }
    None
}

pub(crate) fn secondary_match_pre(
    entry: &WiEntry,
    raw: &str,
    lower: &str,
    settings: &WiSettings,
    regex_cache: &mut HashMap<String, Option<regex::Regex>>,
) -> bool {
    if !entry.selective || entry.keysecondary.is_empty() {
        return true;
    }
    let mut any = false;
    let mut all = true;
    for key in &entry.keysecondary {
        let m = match_keys_pre(raw, lower, key, entry, settings, regex_cache);
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

pub(crate) fn score_entry_pre(
    entry: &WiEntry,
    raw: &str,
    lower: &str,
    settings: &WiSettings,
    regex_cache: &mut HashMap<String, Option<regex::Regex>>,
) -> i32 {
    let mut score = 0;
    for k in &entry.keys {
        if match_keys_pre(raw, lower, k, entry, settings, regex_cache) {
            score += 1;
        }
    }
    for k in &entry.keysecondary {
        if match_keys_pre(raw, lower, k, entry, settings, regex_cache) {
            score += 1;
        }
    }
    score
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
