//! json_repair.rs — 截断 JSON 修复（backport 自 museai-server PR #108，源吸收 dramatica-flow `_repair_truncated_json`，MIT）。
//!
//! 纪律（抄源）：
//! - 先扫「字符串中途截断」：in_str 时截到最后一个完整 } / ]；
//! - 再按括号栈补全未闭合的 { } [ ]；
//! - 末尾悬空逗号 / 冒号先剥掉。
//! - 修不好时原样返回（调用方再决定抛错还是兜底）。
//! - 全纯函数，零依赖（std only）。

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repairs_truncated_array() {
        let t = r#"[{"a": 1}, {"b": 2"#;
        let fixed = repair_truncated_json(t);
        assert!(serde_json::from_str::<serde_json::Value>(&fixed).is_ok(), "修完应可解析: {fixed}");
    }

    #[test]
    fn strips_trailing_comma() {
        let fixed = repair_truncated_json(r#"{"a": 1,"#);
        assert!(serde_json::from_str::<serde_json::Value>(&fixed).is_ok(), "修完应可解析: {fixed}");
    }

    #[test]
    fn intact_json_untouched_semantics() {
        let t = r#"{"a": [1, 2]}"#;
        assert_eq!(repair_truncated_json(t), t);
    }
}
