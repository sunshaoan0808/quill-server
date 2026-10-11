//! st_world_info/parse —— 示例对话/装饰器/条目解析 + 卡片世界书导入 + 宏替换。
#![allow(unused_imports)]

use super::*;

pub fn parse_mes_examples(examples_str: &str, user_name: &str, char_name: &str) -> Vec<(String, String)> {
    let s = examples_str.trim();
    if s.is_empty() || s == "<START>" {
        return Vec::new();
    }
    let normalized = if s.to_ascii_uppercase().contains("<START>") {
        s.to_string()
    } else {
        format!("<START>\n{s}")
    };
    let mut out = Vec::new();
    // split case-insensitive on <START>
    let lower = normalized.to_ascii_lowercase();
    let mut parts: Vec<&str> = Vec::new();
    let mut last = 0;
    let mut search = 0;
    while let Some(rel) = lower[search..].find("<start>") {
        let i = search + rel;
        if i > last {
            let chunk = normalized[last..i].trim();
            if !chunk.is_empty() {
                parts.push(&normalized[last..i]);
            }
        }
        search = i + 7;
        last = search;
    }
    if last < normalized.len() {
        parts.push(&normalized[last..]);
    }
    // first split piece before first START may be empty; ST uses slice(1)
    for block in parts {
        let block = block.trim();
        if block.is_empty() {
            continue;
        }
        out.extend(parse_example_dialogue_block(block, user_name, char_name));
    }
    out
}

pub(crate) fn parse_example_dialogue_block(block: &str, user_name: &str, char_name: &str) -> Vec<(String, String)> {
    let mut messages: Vec<(String, String)> = Vec::new();
    let user_l = user_name.to_lowercase();
    let char_l = char_name.to_lowercase();
    for line in block.lines() {
        let line = line.trim();
        if line.is_empty() || line.eq_ignore_ascii_case("<start>") {
            continue;
        }
        // patterns: {{user}}: / {{char}}: / User: / Char: / name:
        let (role, rest) = if let Some(r) = strip_example_prefix(line, &["{{user}}:", "{{User}}:", "<USER>:", "User:"]) {
            ("user", r)
        } else if let Some(r) = strip_example_prefix(line, &["{{char}}:", "{{Char}}:", "<BOT>:", "<CHAR>:", "Char:", "Assistant:"]) {
            ("assistant", r)
        } else if !user_l.is_empty() && line.to_lowercase().starts_with(&format!("{user_l}:")) {
            ("user", line.split_once(':').map(|(_, r)| r.trim()).unwrap_or(""))
        } else if !char_l.is_empty() && line.to_lowercase().starts_with(&format!("{char_l}:")) {
            ("assistant", line.split_once(':').map(|(_, r)| r.trim()).unwrap_or(""))
        } else if let Some(idx) = line.find(':') {
            // bare Name: content — if looks like assistant continuation, keep as system blob join
            let name = line[..idx].trim();
            let rest = line[idx + 1..].trim();
            if name.eq_ignore_ascii_case("system") {
                ("system", rest)
            } else {
                // append to previous assistant/user if any, else system
                if let Some(last) = messages.last_mut() {
                    if !last.1.is_empty() {
                        last.1.push('\n');
                    }
                    last.1.push_str(line);
                    continue;
                }
                ("system", line)
            }
        } else {
            if let Some(last) = messages.last_mut() {
                last.1.push('\n');
                last.1.push_str(line);
                continue;
            }
            ("system", line)
        };
        if rest.is_empty() && role == "system" {
            continue;
        }
        messages.push((role.to_string(), rest.to_string()));
    }
    messages
}

pub(crate) fn strip_example_prefix<'a>(line: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for p in prefixes {
        if line.len() >= p.len() && line[..p.len()].eq_ignore_ascii_case(p) {
            return Some(line[p.len()..].trim());
        }
    }
    None
}

pub fn parse_decorators(content: &str) -> (Vec<String>, String) {
    const KNOWN: &[&str] = &["@@activate", "@@dont_activate"];
    let is_known = |data: &str| -> bool {
        let d = if data.starts_with("@@@") {
            &data[1..]
        } else {
            data
        };
        KNOWN.iter().any(|k| d.starts_with(k))
    };
    if !content.starts_with("@@") {
        return (Vec::new(), content.to_string());
    }
    let lines: Vec<&str> = content.split('\n').collect();
    let mut decorators = Vec::new();
    let mut fallbacked = false;
    let mut body_start = lines.len();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("@@") {
            if line.starts_with("@@@") && !fallbacked {
                continue;
            }
            if is_known(line) {
                let d = if line.starts_with("@@@") {
                    line[1..].to_string()
                } else {
                    (*line).to_string()
                };
                // normalize to known prefix only (trim trailing junk optional)
                let norm = if d.starts_with("@@dont_activate") {
                    "@@dont_activate".to_string()
                } else if d.starts_with("@@activate") {
                    "@@activate".to_string()
                } else {
                    d
                };
                decorators.push(norm);
                fallbacked = false;
            } else {
                fallbacked = true;
            }
        } else {
            body_start = i;
            break;
        }
    }
    let new_content = if body_start < lines.len() {
        lines[body_start..].join("\n")
    } else {
        String::new()
    };
    (decorators, new_content)
}

pub fn parse_wi_entry(raw: &Value, world: &str, idx: usize) -> Option<WiEntry> {
    if !raw.is_object() {
        return None;
    }
    let ext = raw.get("extensions").cloned().unwrap_or(json!({}));

    let keys = {
        let mut k = as_str_list(raw.get("keys").or_else(|| raw.get("key")));
        if k.is_empty() {
            k = as_str_list(ext.get("keys"));
        }
        k
    };
    let keysecondary = as_str_list(
        raw.get("keysecondary")
            .or_else(|| raw.get("secondary_keys"))
            .or_else(|| raw.get("secondaryKeys")),
    );
    let raw_content = str_field(raw, &["content", "entry", "text"]);
    let (decorators, content) = parse_decorators(&raw_content);
    if content.trim().is_empty() && keys.is_empty() && !bool_field(raw, &["constant"], false)
        && !decorators.iter().any(|d| d == "@@activate")
    {
        return None;
    }

    let disable = {
        if raw.get("disable").and_then(|v| v.as_bool()) == Some(true) {
            true
        } else if raw.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
            true
        } else if raw.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
            true
        } else {
            false
        }
    };

    let position_raw = i32_field(
        raw,
        &["position"],
        i32_field(&ext, &["position"], {
            // character_book uses position string before_char/after_char
            match str_field(raw, &["position"]).as_str() {
                "before_char" | "before" => 0,
                "after_char" | "after" => 1,
                _ => 0,
            }
        }),
    );

    let uid = {
        let u = str_field(raw, &["uid", "id"]);
        if u.is_empty() {
            format!("{world}-{idx}")
        } else {
            u
        }
    };

    let selective_default = !keysecondary.is_empty();
    Some(WiEntry {
        uid,
        world: world.to_string(),
        keys,
        keysecondary,
        content,
        comment: str_field(raw, &["comment", "name"]),
        constant: bool_field(raw, &["constant"], false),
        disable,
        selective: bool_field(raw, &["selective"], selective_default),
        selective_logic: SelectiveLogic::from_i32(i32_field(
            raw,
            &["selectiveLogic", "selective_logic"],
            i32_field(&ext, &["selectiveLogic", "selective_logic"], 0),
        )),
        order: i32_field(
            raw,
            &["order", "insertion_order", "insertionOrder"],
            i32_field(&ext, &["insertion_order"], idx as i32),
        ),
        position: WiPosition::from_i32(position_raw),
        depth: i32_field(raw, &["depth"], i32_field(&ext, &["depth"], 4)),
        probability: f64_field(
            raw,
            &["probability"],
            f64_field(&ext, &["probability"], 100.0),
        )
        .clamp(0.0, 100.0),
        use_probability: bool_field(
            raw,
            &["useProbability", "use_probability"],
            bool_field(&ext, &["useProbability", "use_probability"], true),
        ),
        scan_depth: {
            let d = i32_field(raw, &["scanDepth", "scan_depth"], -1);
            let d2 = i32_field(&ext, &["scan_depth", "scanDepth"], d);
            if d2 < 0 {
                None
            } else {
                Some(d2)
            }
        },
        case_sensitive: {
            if raw.get("caseSensitive").is_some() || raw.get("case_sensitive").is_some() {
                Some(bool_field(raw, &["caseSensitive", "case_sensitive"], false))
            } else if ext.get("case_sensitive").is_some() || ext.get("caseSensitive").is_some() {
                Some(bool_field(
                    &ext,
                    &["case_sensitive", "caseSensitive"],
                    false,
                ))
            } else {
                None
            }
        },
        match_whole_words: {
            if raw.get("matchWholeWords").is_some() || raw.get("match_whole_words").is_some() {
                Some(bool_field(
                    raw,
                    &["matchWholeWords", "match_whole_words"],
                    false,
                ))
            } else if ext.get("match_whole_words").is_some() {
                Some(bool_field(&ext, &["match_whole_words"], false))
            } else {
                None
            }
        },
        group: str_field(raw, &["group"]).or_else(|| str_field(&ext, &["group"])),
        group_override: bool_field(
            raw,
            &["groupOverride", "group_override"],
            bool_field(&ext, &["group_override", "groupOverride"], false),
        ),
        group_weight: f64_field(
            raw,
            &["groupWeight", "group_weight"],
            f64_field(&ext, &["group_weight", "groupWeight"], 100.0),
        ),
        use_group_scoring: {
            if raw.get("useGroupScoring").is_some() {
                Some(bool_field(raw, &["useGroupScoring"], false))
            } else if ext.get("use_group_scoring").is_some() {
                Some(bool_field(&ext, &["use_group_scoring"], false))
            } else {
                None
            }
        },
        exclude_recursion: bool_field(
            raw,
            &["excludeRecursion", "exclude_recursion"],
            bool_field(&ext, &["exclude_recursion"], false),
        ),
        prevent_recursion: bool_field(
            raw,
            &["preventRecursion", "prevent_recursion"],
            bool_field(&ext, &["prevent_recursion"], false),
        ),
        delay_until_recursion: bool_field(
            raw,
            &["delayUntilRecursion", "delay_until_recursion"],
            bool_field(&ext, &["delay_until_recursion"], false),
        ),
        ignore_budget: bool_field(
            raw,
            &["ignoreBudget", "ignore_budget"],
            bool_field(&ext, &["ignore_budget"], false),
        ),
        sticky: {
            let v = i32_field(raw, &["sticky"], i32_field(&ext, &["sticky"], -1));
            if v > 0 { Some(v) } else { None }
        },
        cooldown: {
            let v = i32_field(raw, &["cooldown"], i32_field(&ext, &["cooldown"], -1));
            if v > 0 { Some(v) } else { None }
        },
        delay: {
            let v = i32_field(raw, &["delay"], i32_field(&ext, &["delay"], -1));
            if v > 0 { Some(v) } else { None }
        },
        decorators,
        outlet_name: {
            let n = str_field(raw, &["outletName", "outlet_name"]);
            if n.is_empty() {
                str_field(&ext, &["outlet_name", "outletName"])
            } else {
                n
            }
        },
        character_filter: parse_character_filter(raw, &ext),
        triggers: {
            let mut tr = as_str_list(raw.get("triggers"));
            if tr.is_empty() {
                tr = as_str_list(ext.get("triggers"));
            }
            tr
        },
        vectorized: bool_field(
            raw,
            &["vectorized"],
            bool_field(&ext, &["vectorized"], false),
        ),
        automation_id: {
            let a = str_field(raw, &["automationId", "automation_id"]);
            if a.is_empty() {
                str_field(&ext, &["automation_id", "automationId"])
            } else {
                a
            }
        },
        role: i32_field(raw, &["role"], i32_field(&ext, &["role"], 0)),
    })
}

/// Pipeline: card `world_book` raw entries → structured `WiEntry` list (X6d).
///
/// 吞噬自 tavern-card-distiller extract_card.py `normalize_card` world_book 分支 (MIT)。
/// Filters out `enabled=false` / `disable=true` entries; each surviving entry is
/// parsed by the shared `parse_wi_entry` so keys/content/constant/order all land
/// in the same ST-compatible shape as imported lorebooks.
pub fn import_card_world_book(card: &crate::StCardData, world_name: &str) -> Vec<WiEntry> {
    card.world_book
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let disabled = e
                .get("disable")
                .or_else(|| e.get("disabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let enabled = e.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            enabled && !disabled
        })
        .filter_map(|(i, e)| parse_wi_entry(e, world_name, i))
        .collect()
}

pub(crate) fn parse_character_filter(raw: &Value, ext: &Value) -> Option<CharacterFilter> {
    let src = raw
        .get("characterFilter")
        .or_else(|| raw.get("character_filter"))
        .or_else(|| ext.get("character_filter"))
        .or_else(|| ext.get("characterFilter"));
    let Some(obj) = src else {
        return None;
    };
    if !obj.is_object() {
        return None;
    }
    let names = as_str_list(obj.get("names"));
    let tags = as_str_list(obj.get("tags"));
    let is_exclude = bool_field(obj, &["isExclude", "is_exclude", "exclude"], false);
    if names.is_empty() && tags.is_empty() {
        return None;
    }
    Some(CharacterFilter {
        names,
        tags,
        is_exclude,
    })
}
