//! st_world_info/serde_convert —— WiEntry ↔ ST JSON 双向转换与条目合并。
#![allow(unused_imports)]

use super::*;

pub fn substitute_params(content: &str, ctx: &WiScanContext) -> String {
    if content.is_empty() {
        return String::new();
    }
    let user = if ctx.user_name.is_empty() {
        "User"
    } else {
        ctx.user_name.as_str()
    };
    let char = if ctx.char_name.is_empty() {
        "Char"
    } else {
        ctx.char_name.as_str()
    };
    let mut out = content.to_string();
    for (pat, rep) in [
        ("{{user}}", user),
        ("{{User}}", user),
        ("{{USER}}", user),
        ("<USER>", user),
        ("{{char}}", char),
        ("{{Char}}", char),
        ("{{CHAR}}", char),
        ("<BOT>", char),
        ("<CHAR>", char),
        ("{{char}}", char),
    ] {
        // case-insensitive replace for {{user}}/{{char}}
        if pat.starts_with("{{") {
            // manual ci
            let lower_pat = pat.to_lowercase();
            let mut tmp = String::new();
            let mut rest = out.as_str();
            loop {
                let low = rest.to_lowercase();
                if let Some(i) = low.find(&lower_pat) {
                    tmp.push_str(&rest[..i]);
                    tmp.push_str(rep);
                    rest = &rest[i + pat.len()..];
                } else {
                    tmp.push_str(rest);
                    break;
                }
            }
            out = tmp;
        } else {
            out = out.replace(pat, rep);
        }
    }
    out
}


pub(crate) trait StrOrElse {
    fn or_else(self, f: impl FnOnce() -> String) -> String;
}
impl StrOrElse for String {
    fn or_else(self, f: impl FnOnce() -> String) -> String {
        if self.is_empty() {
            f()
        } else {
            self
        }
    }
}


/// Serialize a WiEntry back to ST-ish JSON (array element under stBookRaw.entries).
pub fn wi_entry_to_st_json(e: &WiEntry) -> Value {
    let mut ext = json!({});
    if e.position as i32 >= 2 {
        ext["position"] = json!(e.position as i32);
    }
    if e.depth != 4 {
        ext["depth"] = json!(e.depth);
    }
    if !e.outlet_name.is_empty() {
        ext["outlet_name"] = json!(e.outlet_name);
    }
    if let Some(s) = e.sticky {
        ext["sticky"] = json!(s);
    }
    if let Some(c) = e.cooldown {
        ext["cooldown"] = json!(c);
    }
    if let Some(d) = e.delay {
        ext["delay"] = json!(d);
    }
    if e.vectorized {
        ext["vectorized"] = json!(true);
    }
    if !e.automation_id.is_empty() {
        ext["automation_id"] = json!(e.automation_id);
    }
    if e.role != 0 {
        ext["role"] = json!(e.role);
    }
    if !e.triggers.is_empty() {
        ext["triggers"] = json!(e.triggers);
    }
    if let Some(ref cf) = e.character_filter {
        ext["character_filter"] = json!({
            "names": cf.names,
            "tags": cf.tags,
            "isExclude": cf.is_exclude,
        });
    }

    let mut content = e.content.clone();
    if !e.decorators.is_empty() {
        let head = e
            .decorators
            .iter()
            .map(|d| {
                if d.starts_with("@@") {
                    d.clone()
                } else {
                    format!("@@{d}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        content = format!("{head}\n{content}");
    }

    let mut obj = json!({
        "uid": e.uid,
        "id": e.uid,
        "keys": e.keys,
        "key": e.keys,
        "keysecondary": e.keysecondary,
        "content": content,
        "comment": e.comment,
        "constant": e.constant,
        "disable": e.disable,
        "enabled": !e.disable,
        "selective": e.selective,
        "selectiveLogic": e.selective_logic as i32,
        "order": e.order,
        "position": e.position as i32,
        "depth": e.depth,
        "probability": e.probability,
        "useProbability": e.use_probability,
        "group": e.group,
        "groupOverride": e.group_override,
        "groupWeight": e.group_weight,
        "excludeRecursion": e.exclude_recursion,
        "preventRecursion": e.prevent_recursion,
        "delayUntilRecursion": e.delay_until_recursion,
        "ignoreBudget": e.ignore_budget,
    });
    if let Some(sd) = e.scan_depth {
        obj["scanDepth"] = json!(sd);
    }
    if let Some(cs) = e.case_sensitive {
        obj["caseSensitive"] = json!(cs);
    }
    if let Some(mw) = e.match_whole_words {
        obj["matchWholeWords"] = json!(mw);
    }
    if let Some(ugs) = e.use_group_scoring {
        obj["useGroupScoring"] = json!(ugs);
    }
    if ext.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
        obj["extensions"] = ext;
    }
    obj
}

/// Build stBookRaw object from entry list (array form).
pub fn st_book_raw_from_entries(name: &str, entries: &[WiEntry]) -> Value {
    let arr: Vec<Value> = entries.iter().map(wi_entry_to_st_json).collect();
    json!({
        "name": name,
        "entries": arr,
    })
}

/// API-facing entry list (ST JSON objects) from a world book item fields/content.
pub fn entry_values_from_world_book(world_name: &str, fields: Option<&Value>, content: &str) -> Vec<Value> {
    entries_from_world_book(world_name, fields, content)
        .iter()
        .map(wi_entry_to_st_json)
        .collect()
}

/// Merge patch JSON onto an existing ST entry value (shallow + keys arrays).
pub fn merge_wi_entry_value(base: &Value, patch: &Value) -> Value {
    let mut out = base.clone();
    let Some(obj) = out.as_object_mut() else {
        return patch.clone();
    };
    let Some(pobj) = patch.as_object() else {
        return out;
    };
    for (k, v) in pobj {
        if k == "extensions" {
            let mut ext = obj
                .get("extensions")
                .cloned()
                .unwrap_or(json!({}));
            if let Some(eo) = ext.as_object_mut() {
                if let Some(po) = v.as_object() {
                    for (ek, ev) in po {
                        eo.insert(ek.clone(), ev.clone());
                    }
                }
            }
            obj.insert("extensions".into(), ext);
        } else if k == "uid" || k == "id" {
            // allow rename only if explicitly set non-empty
            if v.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                obj.insert("uid".into(), v.clone());
                obj.insert("id".into(), v.clone());
            }
        } else {
            obj.insert(k.clone(), v.clone());
        }
    }
    // keep key/keys in sync
    if let Some(keys) = obj.get("keys").cloned() {
        obj.insert("key".into(), keys);
    } else if let Some(key) = obj.get("key").cloned() {
        obj.insert("keys".into(), key);
    }
    if let Some(dis) = obj.get("disable").and_then(|v| v.as_bool()) {
        obj.insert("enabled".into(), json!(!dis));
    } else if let Some(en) = obj.get("enabled").and_then(|v| v.as_bool()) {
        obj.insert("disable".into(), json!(!en));
    }
    out
}

/// Rebuild freeform markdown content from entries (for legacy content field).
pub fn content_from_wi_entries(name: &str, entries: &[WiEntry]) -> String {
    let mut out = format!("# {name}\n\n");
    if entries.is_empty() {
        out.push_str("_（无世界书条目）_\n");
        return out;
    }
    out.push_str("## 世界书条目\n\n");
    let mut sorted: Vec<&WiEntry> = entries.iter().collect();
    sorted.sort_by_key(|e| e.order);
    for e in sorted {
        let title = if e.comment.is_empty() {
            e.uid.clone()
        } else {
            e.comment.clone()
        };
        let keys = if e.keys.is_empty() {
            if e.constant {
                "constant".into()
            } else {
                "—".into()
            }
        } else {
            e.keys.join(", ")
        };
        out.push_str(&format!("### {title}\n"));
        out.push_str(&format!("- keys: {keys}\n"));
        if e.constant {
            out.push_str("- constant: true\n");
        }
        if e.disable {
            out.push_str("- disabled: true\n");
        }
        out.push('\n');
        out.push_str(e.content.trim());
        out.push_str("\n\n");
    }
    out
}

/// Load entries from a partner world_book item's fields / content.
pub fn entries_from_world_book(world_name: &str, fields: Option<&Value>, content: &str) -> Vec<WiEntry> {
    let mut out = Vec::new();
    if let Some(fields) = fields {
        // Prefer full ST book raw
        if let Some(book) = fields
            .get("stBookRaw")
            .or_else(|| fields.get("character_book"))
        {
            if let Some(arr) = book.get("entries").and_then(|e| e.as_array()) {
                for (i, e) in arr.iter().enumerate() {
                    if let Some(ent) = parse_wi_entry(e, world_name, i) {
                        out.push(ent);
                    }
                }
            } else if let Some(map) = book.get("entries").and_then(|e| e.as_object()) {
                // ST world_info uses object map uid -> entry
                for (i, (_k, e)) in map.iter().enumerate() {
                    if let Some(ent) = parse_wi_entry(e, world_name, i) {
                        out.push(ent);
                    }
                }
            }
        }
        if out.is_empty() {
            if let Some(arr) = fields.get("stEntries").and_then(|e| e.as_array()) {
                for (i, e) in arr.iter().enumerate() {
                    if let Some(ent) = parse_wi_entry(e, world_name, i) {
                        out.push(ent);
                    }
                }
            }
        }
        // plain ST world_info export shape on fields.entries
        if out.is_empty() {
            if let Some(arr) = fields.get("entries").and_then(|e| e.as_array()) {
                for (i, e) in arr.iter().enumerate() {
                    if let Some(ent) = parse_wi_entry(e, world_name, i) {
                        out.push(ent);
                    }
                }
            } else if let Some(map) = fields.get("entries").and_then(|e| e.as_object()) {
                for (i, (_k, e)) in map.iter().enumerate() {
                    if let Some(ent) = parse_wi_entry(e, world_name, i) {
                        out.push(ent);
                    }
                }
            }
        }
    }
    if out.is_empty() && !content.trim().is_empty() {
        // Legacy freeform world book: treat whole content as one constant entry
        out.push(WiEntry {
            uid: format!("{world_name}-legacy"),
            world: world_name.to_string(),
            keys: vec![],
            keysecondary: vec![],
            content: content.to_string(),
            comment: world_name.to_string(),
            constant: true,
            disable: false,
            selective: false,
            selective_logic: SelectiveLogic::AndAny,
            order: 100,
            position: WiPosition::Before,
            depth: 4,
            probability: 100.0,
            use_probability: false,
            scan_depth: None,
            case_sensitive: None,
            match_whole_words: None,
            group: String::new(),
            group_override: false,
            group_weight: 100.0,
            use_group_scoring: None,
            exclude_recursion: false,
            prevent_recursion: false,
            delay_until_recursion: false,
            ignore_budget: false,
            sticky: None,
            cooldown: None,
            delay: None,
            decorators: Vec::new(),
            outlet_name: String::new(),
            character_filter: None,
            triggers: Vec::new(),
            vectorized: false,
            automation_id: String::new(),
            role: 0,
        });
    }
    out
}

// --- matching (ST WorldInfoBuffer.matchKeys) ---
