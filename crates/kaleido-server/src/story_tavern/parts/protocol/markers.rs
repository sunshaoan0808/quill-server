//! parts/protocol/markers — [2026-10-06 二期①] 散装标记解析器（自 p005 逐字节迁出）：
//! 【选项】【面板】【角色清单】【节目单】【状态更新】【检定】【节点推进】+ 去重助手。
//! 与 receipt.rs（【回执】）同属叙事文本协议层；p005 保留守卫与 prompt 侧职责。
#![allow(unused_imports)]

use super::super::*;
pub(crate) fn extract_advance_marker(full_text: &str) -> Option<(usize, String)> {
    // 格式1: 【节点推进:n2】
    const INNER: &str = "【节点推进:";
    if let Some(pos) = full_text.rfind(INNER) {
        let after = &full_text[pos + INNER.len()..];
        let node_id: String = after
            .split(|c: char| c == '】' || c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_string();
        if !node_id.is_empty() {
            return Some((pos, node_id));
        }
    }
    // 格式2: 【节点推进】 n2
    const OUTER: &str = "【节点推进】";
    if let Some(pos) = full_text.rfind(OUTER) {
        let after = full_text[pos + OUTER.len()..].trim();
        let node_id = after.split(char::is_whitespace).next().unwrap_or("").to_string();
        if !node_id.is_empty() {
            return Some((pos, node_id));
        }
    }
    None
}

pub(crate) fn parse_option_list(after: &str) -> Vec<String> {
    let after = after.trim();
    if after.is_empty() {
        return Vec::new();
    }
    // [P11 2026-08-15] 入口级 JSON 泄漏检测：LLM 偶发把检定 JSON 对象裸写在正文，
    // 其键名（characterId/templateId/stateChanges/...）会被下方分支捞成「选项」。
    // 选项特征是短中文动作短语，绝不含这些结构键名——检测到即整组丢弃。
    const JSON_KEY_HINTS: [&str; 12] = [
        "characterId", "templateId", "stateChanges", "criticalSuccess",
        "memory_summary", "addTraits", "removeTraits", "outcomes",
        "fieldId", "actorId", "intent", "challenge",
    ];
    {
        let mut quoted: Vec<&str> = Vec::new();
        let mut rest = after;
        while let Some(q) = rest.find('"') {
            rest = &rest[q + 1..];
            if let Some(q2) = rest.find('"') {
                let s = rest[..q2].trim();
                if !s.is_empty() {
                    quoted.push(s);
                }
                rest = &rest[q2 + 1..];
            } else {
                break;
            }
        }
        // 引号串数量多（≥4）且任一含结构键名 → JSON 对象泄漏
        if quoted.len() >= 4 && quoted.iter().any(|s| JSON_KEY_HINTS.iter().any(|k| s.contains(k))) {
            return Vec::new();
        }
    }
    // Prefer JSON array (possibly followed by junk / code fences)
    if let Some(start) = after.find('[') {
        if let Some(end) = after.rfind(']') {
            if end > start {
                let slice = &after[start..=end];
                if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(slice) {
                    let parsed: Vec<String> = arr
                        .into_iter()
                        .filter_map(|v| {
                            v.as_str()
                                .map(|s| s.trim().to_string())
                                .or_else(|| v.as_i64().map(|n| n.to_string()))
                        })
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !parsed.is_empty() {
                        return parsed;
                    }
                }
            }
        }
    }
    // Quoted strings anywhere
    {
        let mut out = Vec::new();
        let mut rest = after;
        while let Some(q) = rest.find('"') {
            rest = &rest[q + 1..];
            if let Some(q2) = rest.find('"') {
                let s = rest[..q2].trim();
                if !s.is_empty() {
                    out.push(s.replace("\\\"", "\""));
                }
                rest = &rest[q2 + 1..];
            } else {
                break;
            }
        }
        // [P11 2026-08-15] 防 JSON 对象污染：LLM 偶发把检定 JSON 对象（{"action":...}）
        // 裸写在正文，此分支会把所有键值对捞成「选项」（实踩 n=52）。
        // 选项特征 = 短（≤40 字）+ 不含 JSON 结构词（键名/嵌套值）。任一违反即判定为
        // JSON 泄漏，整组丢弃（宁可无选项，不给玩家一堆垃圾 chip）。
        const JSON_KEY_HINTS: [&str; 12] = [
            "characterId", "templateId", "stateChanges", "criticalSuccess",
            "memory_summary", "addTraits", "removeTraits", "outcomes",
            "fieldId", "actorId", "intent", "challenge",
        ];
        let looks_like_json_leak = out.len() >= 4
            && out
                .iter()
                .any(|s| JSON_KEY_HINTS.iter().any(|k| s.contains(k)));
        if !looks_like_json_leak && out.len() >= 2 {
            return out;
        }
    }
    let mut out = Vec::new();
    for line in after.lines() {
        let mut l = line.trim();
        if l.is_empty() {
            continue;
        }
        for prefix in ["- ", "• ", "* ", "– ", "· "] {
            if let Some(rest) = l.strip_prefix(prefix) {
                l = rest.trim();
                break;
            }
        }
        let stripped = l
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')', '、', ':', '：', ' ', '\t'])
            .trim_start_matches('）')
            .trim_start_matches(')');
        if stripped.len() < l.len() {
            l = stripped.trim();
        }
        if l.is_empty() || l == "[" || l == "]" || l.starts_with('【') {
            continue;
        }
        out.push(l.to_string());
    }
    out
}

/// Split narrative vs player options. Always strips known markers from content.
/// Returns (clean_content, options).
pub(crate) fn split_options_from_narrative(full: &str) -> (String, Vec<String>) {
    let mut text = full.to_string();
    let mut options: Vec<String> = Vec::new();

    // 1) Canonical 【选项】
    const OPTS_MARKER: &str = "【选项】";
    if let Some(opts_start) = text.rfind(OPTS_MARKER) {
        let after = text[opts_start + OPTS_MARKER.len()..].trim();
        options = parse_option_list(after);
        text = text[..opts_start].trim_end().to_string();
    }

    // 1b) 【询问】停笔卡（吸收自梨园 ask_director）——同解析；允许无正文纯询问回合
    if options.is_empty() {
        const ASK_MARKER: &str = "【询问】";
        if let Some(opts_start) = text.rfind(ASK_MARKER) {
            let after = text[opts_start + ASK_MARKER.len()..].trim();
            options = parse_option_list(after);
            text = text[..opts_start].trim_end().to_string();
        }
    }

    // 2) <choices>...</choices>
    if options.is_empty() {
        let lower = text.to_ascii_lowercase();
        if let Some(start) = lower.rfind("<choices>") {
            if let Some(rel_end) = lower[start..].find("</choices>") {
                let inner_start = start + "<choices>".len();
                let inner_end = start + rel_end;
                let after_end = inner_end + "</choices>".len();
                let inner = text[inner_start..inner_end].trim();
                options = parse_option_list(inner);
                let mut kept = text[..start].to_string();
                if after_end < text.len() {
                    kept.push_str(text[after_end..].trim_start());
                }
                text = kept.trim_end().to_string();
            }
        }
    }

    // 3) Fallback: bare JSON array of 2–6 short strings near end (LLM forgot marker)
    if options.is_empty() {
        if let Some(start) = text.rfind('[') {
            if let Some(end) = text[start..].rfind(']') {
                let slice = &text[start..start + end + 1];
                let parsed = parse_option_list(slice);
                // Heuristic: look like choices (2..6 items, each short)
                if parsed.len() >= 2
                    && parsed.len() <= 6
                    && parsed.iter().all(|s| s.chars().count() <= 80)
                {
                    options = parsed;
                    text = text[..start].trim_end().to_string();
                }
            }
        }
    }

    (text, options)
}

/// 提取 agent 自建面板（【面板】JSON 块，吸收自梨园 panels.ts）。
/// Returns (clean_content, panels)。同名即更新；软上限 6。
pub(crate) fn split_panels_from_narrative(full: &str) -> (String, Vec<kaleido_core::TavernPanel>) {
    let mut text = full.to_string();
    let mut panels: Vec<kaleido_core::TavernPanel> = Vec::new();
    const PANEL_MARKER: &str = "【面板】";
    loop {
        let Some(mark) = text.find(PANEL_MARKER) else { break };
        let after = &text[mark + PANEL_MARKER.len()..];
        let Some(open) = after.find('{') else { break };
        let json_start = mark + PANEL_MARKER.len() + open;
        // 括号深度匹配（JSON 字符串内的花括号会误判——接受，SVG 一般不嵌花括号）
        let mut depth = 0i32;
        let mut close: Option<usize> = None;
        for (i, ch) in text[json_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(json_start + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = close else { break };
        let json_slice = &text[json_start..end];
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(json_slice) {
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
            let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("markdown").to_string();
            let content = v.get("content").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if !name.is_empty() && !content.is_empty() {
                panels.push(kaleido_core::TavernPanel::new(name, kind, content));
            }
        }
        text = format!("{}{}", &text[..mark], &text[end..]);
    }
    panels.truncate(6); // 软上限（吸收自梨园 PANEL_SOFT_LIMIT）
    (text.trim().to_string(), panels)
}

pub(crate) fn split_roster_from_narrative(full: &str) -> (String, Option<Vec<String>>) {
    const OPEN: &str = "<角色清单>";
    const CLOSE: &str = "</角色清单>";
    let mut text = full.to_string();
    let mut roster: Option<Vec<String>> = None;
    loop {
        let Some(open_pos) = text.find(OPEN) else { break };
        let Some(rel_close) = text[open_pos + OPEN.len()..].find(CLOSE) else { break };
        let inner_start = open_pos + OPEN.len();
        let inner_end = inner_start + rel_close;
        let inner = text[inner_start..inner_end].trim();
        let names: Vec<String> = inner
            .split(['、', '，', ',', '。', ' '])
            .map(|s| {
                s.trim()
                    .trim_matches(|c: char| c.is_whitespace() || matches!(c, '、' | '，' | ',' | '。'))
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .collect();
        if !names.is_empty() {
            roster = Some(names);
        }
        let end = inner_end + CLOSE.len();
        text = format!("{}{}", &text[..open_pos], &text[end..]);
    }
    (text.trim().to_string(), roster)
}

/// 提取消息流内嵌程序卡（【程序】…【/程序】配对块, 吸收自梨园 show_html）。
/// Returns (clean_content, Some(html))。只取第一块, 其余按纯文本剥掉配对标记。
pub(crate) fn split_program_from_narrative(full: &str) -> (String, Option<String>) {
    const OPEN: &str = "【程序】";
    const CLOSE: &str = "【/程序】";
    let mut text = full.to_string();
    let mut program: Option<String> = None;
    if let Some(start) = text.find(OPEN) {
        let body_start = start + OPEN.len();
        if let Some(rel) = text[body_start..].find(CLOSE) {
            let body_end = body_start + rel;
            let html = text[body_start..body_end].trim().to_string();
            if !html.is_empty() {
                program = Some(html);
                // 剥离整块(含标记)
                let after = body_end + CLOSE.len();
                text = format!("{}{}", &text[..start], &text[after..]);
            }
        }
    }
    (text.trim().to_string(), program)
}

/// 重复键状态对象拆分：LLM 偶发把多角色状态合并进单个对象
/// （如 {"characterId":"A","fields":{...},"characterId":"B","fields":{...}}），
/// serde 对重复字段报错导致整块解析失败残留正文。
/// 按顶层 "characterId" 键切段、逐段包回 {}，供上层逐段解析；非重复键形态返回空 Vec。
pub(crate) fn split_dupe_state_objects(s: &str) -> Vec<String> {
    let t = s.trim();
    if !t.starts_with('{') || !t.ends_with('}') {
        return Vec::new();
    }
    let inner = &t[1..t.len() - 1];
    let key = "\"characterId\"";
    // 定位每个顶层 "characterId" 键的起始偏移（跳过字符串内的假匹配）
    let mut marks: Vec<usize> = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut i = 0usize;
    while i < inner.len() {
        // 不变量：循环条件 i < inner.len() 保证切片非空，chars().next() 恒 Some
        let c = inner[i..]
            .chars()
            .next()
            .expect("i < inner.len() guaranteed by loop guard");
        let w = c.len_utf8();
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => {
                    if depth == 0 && inner[i..].starts_with(key) {
                        marks.push(i);
                    }
                    in_str = true;
                }
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        i += w;
    }
    if marks.len() < 2 {
        return Vec::new();
    }
    // 每段：段0 从对象头（含 characterId 前的杂键），后续段从该键起，到下一键前止，包回 {}
    let mut segs: Vec<String> = Vec::new();
    for (k, &m) in marks.iter().enumerate() {
        let start = if k == 0 { 0 } else { m };
        let end = if k + 1 < marks.len() { marks[k + 1] } else { inner.len() };
        // 段尾可能残留分隔逗号（段0 切到下一键时带上），去掉后再包 {}
        let seg = inner[start..end].trim().trim_end_matches(',');
        if !seg.is_empty() {
            segs.push(format!("{{{}}}", seg));
        }
    }
    segs
}

/// [fix §10 2026-08-16] 章节剧情摘要提炼兜底：LLM 偶发输出「复述任务+思考」而非纯文本
/// （deepseek-v4-flash-free 指令遵循差）。此处复用 B2 的清洗链（剥 think/围栏/指令行），
/// 并附加指令回声截断（C7 已修）。
pub(crate) fn split_state_updates_from_narrative(full: &str) -> (String, Vec<kaleido_core::ActorStateUpdate>) {
    let mut clean = String::new();
    let mut updates: Vec<kaleido_core::ActorStateUpdate> = Vec::new();
    const STATE_MARKER: &str = "【状态更新】";
    let mut rest = full;
    loop {
        let Some(mark) = rest.find(STATE_MARKER) else {
            clean.push_str(rest);
            break;
        };
        clean.push_str(&rest[..mark]);
        let after = &rest[mark + STATE_MARKER.len()..];
        let Some(open) = after.find('{') else {
            clean.push_str(&rest[mark..]);
            break;
        };
        let json_start = mark + STATE_MARKER.len() + open;
        // 括号深度匹配（JSON 字符串内的花括号会误判——接受，状态值一般不嵌花括号）
        let mut depth = 0i32;
        let mut close: Option<usize> = None;
        for (i, ch) in rest[json_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(json_start + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = close else {
            clean.push_str(&rest[mark..]);
            break;
        };
        match serde_json::from_str::<kaleido_core::ActorStateUpdate>(&rest[json_start..end]) {
            Ok(u) if !u.character_id.is_empty() => {
                // 成功解析：剥离整块并收集
                updates.push(u);
            }
            _ => {
                // 解析失败/character_id 为空：尝试重复键拆分（LLM 偶发把多角色状态
                // 合并进单对象致 serde 重复字段报错）；全部子段可解析才剥离整块，否则保留原文
                let dupes = split_dupe_state_objects(&rest[json_start..end]);
                let parsed: Vec<kaleido_core::ActorStateUpdate> = dupes
                    .iter()
                    .filter_map(|seg| {
                        serde_json::from_str::<kaleido_core::ActorStateUpdate>(seg)
                            .ok()
                            .filter(|u| !u.character_id.is_empty())
                    })
                    .collect();
                if !parsed.is_empty() && parsed.len() == dupes.len() {
                    updates.extend(parsed);
                } else {
                    // [fix §9 2026-08-16] 解析失败也剥离：带【状态更新】标记的畸形块
                    // 正文零残留优先（状态不应用 < 污染正文），warn 记录原文供排查。
                    tracing::warn!(
                        raw = %&rest[json_start..end].chars().take(200).collect::<String>(),
                        "st state block unparseable; stripped from narrative"
                    );
                }
            }
        }
        rest = &rest[end..];
        if updates.len() >= 20 {
            clean.push_str(rest);
            break;
        }
    }

    // [fix 2026-08-16] 无标记裸 JSON 状态块剥离：LLM 偶发在正文末尾直接追加
    // {"characterId":...} 对象而未带【状态更新】标记，标记匹配不到会残留进正文。
    // 扫描 clean 中非标记开头的 JSON 对象，可解析为 ActorStateUpdate 且 character_id
    // 非空则剥离（结构特征极特定，正文叙事几乎不可能自然出现，误伤风险低）。
    let mut bare_clean = String::new();
    let mut bare_rest = clean.as_str();
    loop {
        if bare_rest.trim().is_empty() {
            bare_clean.push_str(bare_rest);
            break;
        }
        let Some(open) = bare_rest.find('{') else {
            bare_clean.push_str(bare_rest);
            break;
        };
        let prefix = &bare_rest[..open];
        // 仅当 { 处于行首（串开头或前一个字符是换行）才视为独立状态块；
        // 否则整段（含 {）作为正文内容保留，避免误删正文内嵌花括号或保留的标记块。
        if !(prefix.is_empty() || prefix.ends_with('\n')) {
            bare_clean.push_str(&bare_rest[..open + 1]);
            bare_rest = &bare_rest[open + 1..];
            continue;
        }
        // 括号深度匹配 JSON 对象
        let mut depth = 0i32;
        let mut close: Option<usize> = None;
        for (i, ch) in bare_rest[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = close else {
            bare_clean.push_str(bare_rest);
            break;
        };
        // 行首 JSON：尝试解析为状态块
        if let Ok(u) = serde_json::from_str::<kaleido_core::ActorStateUpdate>(&bare_rest[open..end]) {
            if !u.character_id.is_empty() {
                // 独立状态块：保留行首正文 prefix，剥离 JSON 对象本身
                if !prefix.is_empty() {
                    bare_clean.push_str(prefix);
                }
                updates.push(u);
            } else {
                // character_id 为空：保留整块
                bare_clean.push_str(&bare_rest[..end]);
            }
        } else {
            // 解析失败：尝试重复键拆分（LLM 偶发把多角色状态合并进单对象）
            let dupes = split_dupe_state_objects(&bare_rest[open..end]);
            let parsed: Vec<kaleido_core::ActorStateUpdate> = dupes
                .iter()
                .filter_map(|seg| {
                    serde_json::from_str::<kaleido_core::ActorStateUpdate>(seg)
                        .ok()
                        .filter(|u| !u.character_id.is_empty())
                })
                .collect();
            if !parsed.is_empty() && parsed.len() == dupes.len() {
                if !prefix.is_empty() {
                    bare_clean.push_str(prefix);
                }
                updates.extend(parsed);
            } else {
                // [fix §9 2026-08-16] 行首独立 JSON 但解析失败：结构特征已极特定
                // （行首 { 对象），正文零残留优先——剥离 + warn（与带标记路径一致）。
                tracing::warn!(
                    raw = %bare_rest[open..end].chars().take(200).collect::<String>(),
                    "st bare state block unparseable; stripped from narrative"
                );
            }
        }
        bare_rest = &bare_rest[end..];
    }
    (bare_clean.trim().to_string(), updates)
}

/// ST-27: 提取【检定】JSON 块（括号深度匹配，同【状态更新】）。
/// 成功解析且 action 非空的块收集并剥离；解析失败/action 为空时跳过该块但保留原文。
/// 软上限 5 块。
pub(crate) fn split_check_from_narrative(full: &str) -> (String, Vec<kaleido_core::TurnCheckRequest>) {
    let mut clean = String::new();
    let mut checks: Vec<kaleido_core::TurnCheckRequest> = Vec::new();
    const CHECK_MARKER: &str = "【检定】";
    let mut rest = full;
    loop {
        let Some(mark) = rest.find(CHECK_MARKER) else {
            clean.push_str(rest);
            break;
        };
        clean.push_str(&rest[..mark]);
        let after = &rest[mark + CHECK_MARKER.len()..];
        let Some(open) = after.find('{') else {
            clean.push_str(&rest[mark..]);
            break;
        };
        let json_start = mark + CHECK_MARKER.len() + open;
        // 括号深度匹配（JSON 字符串内的花括号会误判——接受，检定值一般不嵌花括号）
        let mut depth = 0i32;
        let mut close: Option<usize> = None;
        for (i, ch) in rest[json_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(json_start + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = close else {
            clean.push_str(&rest[mark..]);
            break;
        };
        match serde_json::from_str::<kaleido_core::TurnCheckRequest>(&rest[json_start..end]) {
            Ok(c) if !c.action.trim().is_empty() => {
                // 成功解析：剥离整块并收集
                checks.push(c);
            }
            _ => {
                // 解析失败/action 为空：跳过该块（不中断、不报错），但保留原文
                clean.push_str(&rest[mark..end]);
            }
        }
        rest = &rest[end..];
        if checks.len() >= 5 {
            clean.push_str(rest);
            break;
        }
    }
    (clean.trim().to_string(), checks)
}

