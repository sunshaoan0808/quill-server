//! crawler/parts/p002/html —— HTML 抽取/清洗助手（anti-bot/initial_state/标题/章节项/strip_tags）。（R6-3① 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn check_anti_bot(html: &str) -> Result<(), String> {
    if html.len() < 5000 || !html.contains("window.__INITIAL_STATE__") {
        // API-only pages may be shorter; only hard-fail if captcha markers present.
        if let Some(title) = extract_html_title(html) {
            let t = title.to_lowercase();
            if t.contains("验证") || t.contains("captcha") || (t.contains("安全") && t.contains("验证")) {
                return Err("触发番茄小说反爬验证拦截（滑块/验证页）。".into());
            }
        }
        if html.len() < 800 && !html.contains("chapterData") {
            return Err("触发番茄小说反爬验证拦截，页面内容异常。".into());
        }
    }
    if let Some(title) = extract_html_title(html) {
        let t = title.to_lowercase();
        if t.contains("验证") || t.contains("captcha") || (t.contains("安全") && t.contains("验证")) {
            return Err(
                "触发番茄小说反爬验证拦截，请在浏览器中访问该链接进行验证后重试。".into(),
            );
        }
    }
    Ok(())
}

pub(crate) fn extract_from_initial_state(html: &str) -> Option<(String, String)> {
    let start_marker = "window.__INITIAL_STATE__ =";
    let start_marker2 = "window.__INITIAL_STATE__=";
    let idx = html.find(start_marker).or_else(|| html.find(start_marker2))?;
    let brace_rel = html[idx..].find('{')?;
    let json_str = &html[idx + brace_rel..];
    // Replace bare `undefined` tokens so serde can parse a prefix.
    let cleaned = replace_undefined(json_str);
    // Incremental parse first value
    let mut stream = serde_json::Deserializer::from_str(&cleaned).into_iter::<Value>();
    let data = stream.next()?.ok()?;
    let content = data
        .pointer("/reader/chapterData/content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let title = data
        .pointer("/reader/chapterData/title")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    Some((title, content))
}

pub(crate) fn replace_undefined(s: &str) -> String {
    // Cheap substitute for r":\s*undefined\b"
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            out.push(':');
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                out.push(bytes[i] as char);
                i += 1;
            }
            if i + 9 <= bytes.len() && &bytes[i..i + 9] == b"undefined" {
                let end_ok = i + 9 == bytes.len()
                    || !(bytes[i + 9].is_ascii_alphanumeric() || bytes[i + 9] == b'_');
                if end_ok {
                    out.push_str("null");
                    i += 9;
                    continue;
                }
            }
            continue;
        }
        let c = s[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

pub(crate) fn extract_html_title(html: &str) -> Option<String> {
    let start = html.find("<title>")? + 7;
    let end_rel = html[start..].find("</title>")?;
    Some(html[start..start + end_rel].trim().to_string())
}

pub(crate) fn extract_h1_title(html: &str) -> Option<String> {
    // Prefer <h1>...</h1>
    if let Some(start) = html.find("<h1") {
        if let Some(gt) = html[start..].find('>') {
            let content_start = start + gt + 1;
            if let Some(end_rel) = html[content_start..].find("</h1>") {
                let raw = &html[content_start..content_start + end_rel];
                let t = strip_tags(raw).trim().to_string();
                if !t.is_empty() {
                    return Some(t);
                }
            }
        }
    }
    extract_html_title(html)
}

pub(crate) fn extract_reader_ids(html: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let needle = "/reader/";
    let mut search = html;
    while let Some(idx) = search.find(needle) {
        let after = &search[idx + needle.len()..];
        let id: String = after
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !id.is_empty() && !ids.contains(&id) {
            ids.push(id);
        }
        search = &after[after
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .count()
            .min(after.len())..];
        if search.is_empty() {
            break;
        }
        // advance at least 1 to avoid infinite loop
        if search == after {
            search = &after[1.min(after.len())..];
        }
    }
    ids
}

pub(crate) fn extract_chapter_items(html: &str) -> Vec<(String, String)> {
    // Look for anchors with /reader/ and nearby text.
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(a_idx) = rest.find("<a") {
        let chunk = &rest[a_idx..];
        let Some(tag_end) = chunk.find('>') else {
            rest = &rest[a_idx + 2..];
            continue;
        };
        let open = &chunk[..=tag_end];
        let after_open = &chunk[tag_end + 1..];
        let Some(close_rel) = after_open.find("</a>") else {
            rest = &rest[a_idx + 2..];
            continue;
        };
        let inner = &after_open[..close_rel];
        rest = &after_open[close_rel + 4..];

        if !open.contains("/reader/") {
            continue;
        }
        // href
        let href = attr_value(open, "href").unwrap_or_default();
        let id = capture_after(&href, "/reader/").unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let title = strip_tags(inner).trim().to_string();
        if title.contains("最近更新") || title.contains("开始阅读") {
            continue;
        }
        if title.is_empty() {
            continue;
        }
        if !out.iter().any(|(_, cid)| cid == &id) {
            out.push((title, id));
        }
    }
    out
}

pub(crate) fn attr_value(tag: &str, name: &str) -> Option<String> {
    let patterns = [
        format!("{name}=\""),
        format!("{name}='"),
    ];
    for p in patterns {
        if let Some(idx) = tag.find(&p) {
            let start = idx + p.len();
            let quote = p.chars().last()?;
            let end_rel = tag[start..].find(quote)?;
            return Some(tag[start..start + end_rel].to_string());
        }
    }
    None
}

pub(crate) fn capture_after(s: &str, marker: &str) -> Option<String> {
    let idx = s.find(marker)?;
    let after = &s[idx + marker.len()..];
    let id: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
}

pub(crate) fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

pub(crate) fn sanitize_filename(filename: &str) -> String {
    let mut safe = filename.to_string();
    for c in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
        safe = safe.replace(c, "_");
    }
    let t = safe.trim();
    if t.is_empty() {
        "untitled".into()
    } else {
        t.chars().take(80).collect()
    }
}

pub(crate) fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
}

pub(crate) fn clean_html_content(content: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    // Prefer paragraph-ish split on <p> / <div>
    let mut paragraphs = Vec::new();
    let lower = content.to_ascii_lowercase();
    if lower.contains("<p") || lower.contains("<div") {
        let mut rest = content;
        while let Some(idx) = rest.find('<') {
            let after = &rest[idx..];
            let tag_name = after
                .chars()
                .skip(1)
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .to_ascii_lowercase();
            if tag_name == "p" || tag_name == "div" {
                if let Some(gt) = after.find('>') {
                    let body_start = idx + gt + 1;
                    let close = format!("</{tag_name}");
                    if let Some(end_rel) = rest[body_start..].to_ascii_lowercase().find(&close) {
                        let raw = &rest[body_start..body_start + end_rel];
                        let txt = strip_tags(raw).trim().to_string();
                        if !txt.is_empty() {
                            paragraphs.push(txt);
                        }
                        rest = &rest[body_start + end_rel + close.len()..];
                        continue;
                    }
                }
            }
            // advance
            rest = &rest[idx + 1..];
            if rest.is_empty() {
                break;
            }
        }
    }
    if paragraphs.is_empty() {
        let stripped = strip_tags(content);
        for line in stripped.split('\n') {
            let t = line.trim();
            if !t.is_empty() {
                paragraphs.push(t.to_string());
            }
        }
    }
    paragraphs.join("\n\n")
}
