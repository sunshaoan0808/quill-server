#![allow(unused_imports)]
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;
use super::*;

pub(crate) fn normalize_job_kind(kind: &str) -> String {
    let k = kind.trim().to_ascii_lowercase().replace('-', "_");
    match k.as_str() {
        "background" | "book_travel" | "outline" | "agent" | "chat" | "other" | "noop"
        | "test" => k,
        "booktravel" => "book_travel".into(),
        "" => "other".into(),
        _ => {
            // Allow free-form kinds but keep known aliases; unknown stays as given (trimmed).
            if k.is_empty() {
                "other".into()
            } else {
                k
            }
        }
    }
}

/// P13: delta 事件不占 JOB_EVENTS_CAP 名额——流式 token 会瞬间挤掉
/// started/progress 等关键事件（实测 book-travel assemble 62 个空 delta 全部
/// 覆盖了 "job started"）。delta 只保留最近 DELTA_TAIL_CAP 条供 SSE 补发。
pub(crate) const DELTA_TAIL_CAP: usize = 16;

pub(crate) fn trim_events_with_delta_tail(events: &mut Vec<JobEvent>) {
    // 先按常规规则裁剪，再从被裁掉的头部回收最近的 delta 到尾部保留窗。
    if events.len() <= JOB_EVENTS_CAP {
        return;
    }
    let drain = events.len() - JOB_EVENTS_CAP;
    let mut kept_deltas: Vec<JobEvent> = events[..drain]
        .iter()
        .filter(|e| e.event_type == "delta")
        .cloned()
        .collect();
    if kept_deltas.len() > DELTA_TAIL_CAP {
        let skip = kept_deltas.len() - DELTA_TAIL_CAP;
        kept_deltas.drain(0..skip);
    }
    events.drain(0..drain);
    // 尾插保留的 delta（保持时间序：老 delta 在新非 delta 之前）。
    for ev in kept_deltas {
        events.insert(0, ev);
    }
}

// --- Partner (world books + character cards) ---

#[derive(Debug, Clone)]
pub struct GenerationPromptResult {
    pub system_prompt: String,
    pub wi: crate::WiScanResult,
    pub regex_script_count: usize,
    /// Updated timed effects to persist for this chat (if any).
    pub timed_world_info: Option<crate::TimedWorldInfo>,
    /// Extra OpenAI-style messages to splice into the chat (EM/depth/outlet slots).
    pub message_injections: Vec<serde_json::Value>,
    /// automationId from activated WI entries.
    pub automation_ids: Vec<String>,
    /// EM example dialogue pairs (already role-split).
    pub example_messages: Vec<crate::WiExampleMessage>,
}



#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartnerItem {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub item_type: String, // world_book | character_card
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_book_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartnerState {
    #[serde(default)]
    pub world_books: Vec<PartnerItem>,
    #[serde(default)]
    pub character_cards: Vec<PartnerItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_world_book_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_character_card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_type: Option<String>,
}

impl Default for PartnerState {
    fn default() -> Self {
        Self {
            world_books: Vec::new(),
            character_cards: Vec::new(),
            selected_world_book_id: None,
            selected_character_card_id: None,
            selected_id: None,
            selected_type: None,
        }
    }
}

/// Minimal markdown compile from fields (subset of upstream compileItemToMarkdown).
pub fn compile_partner_markdown(name: &str, item_type: &str, fields: &Value) -> String {
    let get = |k: &str| -> String {
        fields
            .get(k)
            .and_then(|v| {
                if let Some(s) = v.as_str() {
                    Some(s.to_string())
                } else if let Some(a) = v.as_array() {
                    Some(
                        a.iter()
                            .filter_map(|x| x.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    )
                } else {
                    v.as_i64().map(|n| n.to_string())
                }
            })
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let mut out = String::new();
    if item_type == "world_book" {
        out.push_str(&format!("# {name}\n\n"));
        let mut lines = Vec::new();
        for (label, key) in [
            ("主题", "theme"),
            ("时代", "era"),
            ("科技水平", "techLevel"),
            ("魔法水平", "magicLevel"),
        ] {
            let v = get(key);
            if !v.is_empty() {
                lines.push(format!("- **{label}**: {v}"));
            }
        }
        if !lines.is_empty() {
            out.push_str("## 核心设定\n");
            out.push_str(&lines.join("\n"));
            out.push_str("\n\n");
        }
        for (label, key) in [
            ("地理格局", "geography"),
            ("关键场景", "keyScenes"),
            ("文化特色", "culturalFeatures"),
            ("历史事件", "history"),
            ("核心矛盾", "conflict"),
        ] {
            let v = get(key);
            if !v.is_empty() {
                out.push_str(&format!("## {label}\n{v}\n\n"));
            }
        }
    } else {
        out.push_str(&format!("# 角色卡：{name}\n\n"));
        let mut lines = Vec::new();
        for (label, key) in [
            ("姓名", "name"),
            ("年龄", "age"),
            ("性别", "gender"),
            ("种族", "race"),
            ("出生地", "birthplace"),
            ("职业", "occupation"),
            ("社会阶层", "socialClass"),
        ] {
            let v = if key == "name" && get(key).is_empty() {
                name.to_string()
            } else {
                get(key)
            };
            if !v.is_empty() {
                lines.push(format!("- **{label}**: {v}"));
            }
        }
        if !lines.is_empty() {
            out.push_str("## 基础信息\n");
            out.push_str(&lines.join("\n"));
            out.push_str("\n\n");
        }
        let tags = get("identityTags");
        if !tags.is_empty() {
            out.push_str(&format!("## 身份标签\n{tags}\n\n"));
        }
        for (section, keys) in [
            (
                "外貌气质",
                &[
                    ("身高体型", "heightBuild"),
                    ("标志性特征", "iconicFeatures"),
                    ("衣着风格", "clothingStyle"),
                    ("整体气质", "overallVibe"),
                ][..],
            ),
            (
                "性格特征",
                &[
                    ("外在性格", "externalPersonality"),
                    ("内在性格", "internalPersonality"),
                    ("核心欲望", "coreDesire"),
                    ("恐惧和弱点", "fearWeakness"),
                    ("道德观念", "moralValues"),
                    ("怪癖", "quirk"),
                ][..],
            ),
            (
                "角色记忆",
                &[
                    ("与用户关系类型", "userRelationType"),
                    ("与用户相处模式", "userInteractionModel"),
                    ("与用户关系底线", "userRelationBottomLine"),
                ][..],
            ),
        ] {
            let mut sl = Vec::new();
            for (label, key) in keys {
                let v = get(key);
                if !v.is_empty() {
                    sl.push(format!("- **{label}**: {v}"));
                }
            }
            if !sl.is_empty() {
                out.push_str(&format!("## {section}\n"));
                out.push_str(&sl.join("\n"));
                out.push_str("\n\n");
            }
        }
        for (label, key) in [
            ("技能专长", "skills"),
            ("背景故事", "backgroundStory"),
            ("人际关系", "relationships"),
            ("说话方式", "speakingStyle"),
            ("典型反应", "typicalReactions"),
            ("关键事件", "keyEvents"),
        ] {
            let v = get(key);
            if !v.is_empty() {
                out.push_str(&format!("## {label}\n{v}\n\n"));
            }
        }
    }
    out.trim().to_string() + "\n"
}

/// C2 审计修复：sanitize 一个 user_id 用于作为存储分区后缀。
/// 仅允许 ASCII alnum、`-`、`_`，使 id 不可能引入路径分隔符或穿越出 config 目录。
pub fn validate_user_scope(user_id: &str) -> String {
    let mut out = String::with_capacity(user_id.len());
    for c in user_id.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("anonymous");
    }
    out
}

#[derive(Clone)]
pub struct PartnerStore {
    app_state: AppStateStore,
    /// C2 审计修复：per-user partition。设置时所有读写走
    /// `partner-store-{user_scope}.json` 而非共享全局文件。
    user_scope: Option<String>,
}

impl PartnerStore {
    pub fn new(app_state: AppStateStore) -> Self {
        Self {
            app_state,
            user_scope: None,
        }
    }

    /// C2：返回绑定到 `user_id` 的 per-user scoped 副本。
    /// Handler 必须用认证会话的 user_id 调用本方法，防止跨租户读写。
    pub fn scoped(&self, user_id: &str) -> PartnerStore {
        PartnerStore {
            app_state: self.app_state.clone(),
            user_scope: Some(validate_user_scope(user_id)),
        }
    }

    fn store_name(&self) -> String {
        match &self.user_scope {
            Some(u) => format!("partner-store-{u}"),
            None => "partner-store".into(),
        }
    }

    pub fn load(&self) -> CoreResult<PartnerState> {
        let raw = self.app_state.load(&self.store_name())?;
        let v: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
        let state = v.get("state").cloned().unwrap_or(v);
        let mut ps: PartnerState = serde_json::from_value(state).unwrap_or_default();
        // ensure content compiled if empty but fields present
        for item in ps.world_books.iter_mut().chain(ps.character_cards.iter_mut()) {
            if item.content.trim().is_empty() {
                if let Some(fields) = &item.fields {
                    item.content =
                        compile_partner_markdown(&item.name, &item.item_type, fields);
                }
            }
        }
        Ok(ps)
    }

    pub fn save(&self, mut state: PartnerState) -> CoreResult<PartnerState> {
        for item in state.world_books.iter_mut() {
            item.item_type = "world_book".into();
            if let Some(fields) = &item.fields {
                if item.content.trim().is_empty() {
                    item.content = compile_partner_markdown(&item.name, "world_book", fields);
                }
            }
        }
        for item in state.character_cards.iter_mut() {
            item.item_type = "character_card".into();
            if let Some(fields) = &item.fields {
                if item.content.trim().is_empty() {
                    item.content =
                        compile_partner_markdown(&item.name, "character_card", fields);
                }
            }
        }
        let payload = json!({ "state": state });
        self.app_state
            .save(&self.store_name(), &serde_json::to_string_pretty(&payload)?)?;
        Ok(state)
    }

    pub fn upsert_world_book(&self, mut item: PartnerItem) -> CoreResult<PartnerItem> {
        let mut st = self.load()?;
        item.item_type = "world_book".into();
        if item.id.is_empty() {
            item.id = format!("wb-{}", Uuid::new_v4());
        }
        // Preserve caller-built content (e.g. ST character_book compile). Only
        // synthesize markdown from fields when content is empty.
        if item.content.trim().is_empty() {
            if let Some(fields) = &item.fields {
                item.content = compile_partner_markdown(&item.name, "world_book", fields);
            }
        }
        if let Some(slot) = st.world_books.iter_mut().find(|x| x.id == item.id) {
            *slot = item.clone();
        } else {
            st.world_books.push(item.clone());
        }
        self.save(st)?;
        Ok(item)
    }

    pub fn upsert_character_card(&self, mut item: PartnerItem) -> CoreResult<PartnerItem> {
        let mut st = self.load()?;
        item.item_type = "character_card".into();
        if item.id.is_empty() {
            item.id = format!("cc-{}", Uuid::new_v4());
        }
        if let Some(fields) = &item.fields {
            item.content = compile_partner_markdown(&item.name, "character_card", fields);
        }
        if let Some(slot) = st.character_cards.iter_mut().find(|x| x.id == item.id) {
            *slot = item.clone();
        } else {
            st.character_cards.push(item.clone());
        }
        self.save(st)?;
        Ok(item)
    }

    pub fn delete_world_book(&self, id: &str, cascade: bool) -> CoreResult<()> {
        let mut st = self.load()?;
        st.world_books.retain(|x| x.id != id);
        if cascade {
            st.character_cards.retain(|x| x.world_book_id.as_deref() != Some(id));
        } else {
            for c in st.character_cards.iter_mut() {
                if c.world_book_id.as_deref() == Some(id) {
                    c.world_book_id = None;
                }
            }
        }
        if st.selected_world_book_id.as_deref() == Some(id) {
            st.selected_world_book_id = None;
        }
        self.save(st)?;
        Ok(())
    }


    pub fn get_world_book(&self, id: &str) -> CoreResult<PartnerItem> {
        let st = self.load()?;
        st.world_books
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("world book {id}")))
    }

    /// List structured WI entries for a world book (ST JSON objects).
    pub fn list_world_book_entries(&self, id: &str) -> CoreResult<Vec<Value>> {
        let wb = self.get_world_book(id)?;
        Ok(crate::entry_values_from_world_book(
            &wb.name,
            wb.fields.as_ref(),
            &wb.content,
        ))
    }

    /// Replace entire entry set; writes fields.stBookRaw + fields.stEntries + content.
    pub fn put_world_book_entries(&self, id: &str, entries: Vec<Value>) -> CoreResult<PartnerItem> {
        let mut st = self.load()?;
        let slot = st
            .world_books
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or_else(|| CoreError::NotFound(format!("world book {id}")))?;
        let world = slot.name.clone();
        let mut parsed = Vec::new();
        for (i, e) in entries.iter().enumerate() {
            let mut e = e.clone();
            // ensure uid
            if e.get("uid").and_then(|v| v.as_str()).unwrap_or("").is_empty()
                && e.get("id").and_then(|v| v.as_str()).unwrap_or("").is_empty()
            {
                if let Some(obj) = e.as_object_mut() {
                    let uid = format!("{world}-{i}-{}", Uuid::new_v4());
                    obj.insert("uid".into(), json!(uid));
                    obj.insert("id".into(), json!(uid));
                }
            }
            if let Some(ent) = crate::parse_wi_entry(&e, &world, i) {
                parsed.push(ent);
            } else {
                // keep raw if parse fails but has content/keys
                parsed.push(
                    crate::parse_wi_entry(
                        &json!({
                            "uid": e.get("uid").or_else(|| e.get("id")).cloned().unwrap_or(json!(format!("{i}"))),
                            "keys": e.get("keys").or_else(|| e.get("key")).cloned().unwrap_or(json!([])),
                            "content": e.get("content").cloned().unwrap_or(json!("")),
                            "constant": e.get("constant").cloned().unwrap_or(json!(false)),
                            "comment": e.get("comment").cloned().unwrap_or(json!("")),
                        }),
                        &world,
                        i,
                    )
                    .ok_or_else(|| CoreError::BadRequest(format!("invalid entry at index {i}")))?,
                );
            }
        }
        let raw = crate::st_book_raw_from_entries(&world, &parsed);
        let st_entries: Vec<Value> = parsed.iter().map(crate::wi_entry_to_st_json).collect();
        let mut fields = slot.fields.clone().unwrap_or_else(|| json!({}));
        if let Some(obj) = fields.as_object_mut() {
            obj.insert("stBookRaw".into(), raw);
            obj.insert("stEntries".into(), json!(st_entries));
            // drop ambiguous plain entries map to avoid dual-source drift
            obj.remove("entries");
        } else {
            fields = json!({ "stBookRaw": raw, "stEntries": st_entries });
        }
        slot.fields = Some(fields);
        slot.content = crate::content_from_wi_entries(&world, &parsed);
        let out = slot.clone();
        self.save(st)?;
        Ok(out)
    }

    pub fn create_world_book_entry(&self, id: &str, entry: Value) -> CoreResult<Value> {
        let mut list = self.list_world_book_entries(id)?;
        let world = self.get_world_book(id)?.name;
        let mut entry = entry;
        let uid = entry
            .get("uid")
            .or_else(|| entry.get("id"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("e-{}", Uuid::new_v4()));
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("uid".into(), json!(uid));
            obj.insert("id".into(), json!(uid));
        }
        // reject dup
        if list.iter().any(|e| {
            e.get("uid").and_then(|v| v.as_str()) == Some(uid.as_str())
                || e.get("id").and_then(|v| v.as_str()) == Some(uid.as_str())
        }) {
            return Err(CoreError::BadRequest(format!("entry uid already exists: {uid}")));
        }
        // validate parseable
        let _ = crate::parse_wi_entry(&entry, &world, list.len())
            .ok_or_else(|| CoreError::BadRequest("invalid entry".into()))?;
        list.push(entry.clone());
        self.put_world_book_entries(id, list)?;
        Ok(entry)
    }

    pub fn patch_world_book_entry(
        &self,
        id: &str,
        entry_id: &str,
        patch: Value,
    ) -> CoreResult<Value> {
        let mut list = self.list_world_book_entries(id)?;
        let pos = list.iter().position(|e| {
            e.get("uid").and_then(|v| v.as_str()) == Some(entry_id)
                || e.get("id").and_then(|v| v.as_str()) == Some(entry_id)
        });
        let Some(i) = pos else {
            return Err(CoreError::NotFound(format!("entry {entry_id}")));
        };
        let merged = crate::merge_wi_entry_value(&list[i], &patch);
        // keep uid stable unless patch renames
        let new_uid = merged
            .get("uid")
            .or_else(|| merged.get("id"))
            .and_then(|v| v.as_str())
            .unwrap_or(entry_id)
            .to_string();
        if new_uid != entry_id {
            if list.iter().enumerate().any(|(j, e)| {
                j != i
                    && (e.get("uid").and_then(|v| v.as_str()) == Some(new_uid.as_str())
                        || e.get("id").and_then(|v| v.as_str()) == Some(new_uid.as_str()))
            }) {
                return Err(CoreError::BadRequest(format!("entry uid already exists: {new_uid}")));
            }
        }
        list[i] = merged.clone();
        self.put_world_book_entries(id, list)?;
        Ok(merged)
    }

    pub fn delete_world_book_entry(&self, id: &str, entry_id: &str) -> CoreResult<()> {
        let mut list = self.list_world_book_entries(id)?;
        let before = list.len();
        list.retain(|e| {
            e.get("uid").and_then(|v| v.as_str()) != Some(entry_id)
                && e.get("id").and_then(|v| v.as_str()) != Some(entry_id)
        });
        if list.len() == before {
            return Err(CoreError::NotFound(format!("entry {entry_id}")));
        }
        self.put_world_book_entries(id, list)?;
        Ok(())
    }


    /// Rebuild `stBookRaw` / `stEntries` / content for a world book.
    ///
    /// Sources (via `entries_from_world_book`): existing stBookRaw / character_book /
    /// stEntries / fields.entries, else freeform `content` as one constant legacy entry.
    ///
    /// - `force=false`: if stBookRaw already has ≥1 parseable entry, re-normalize writeback only
    ///   (still ensures stEntries/content sync). Returns `alreadyHadRaw=true`.
    /// - `force=true`: always re-derive from current fields+content (may re-materialize legacy).
    pub fn rebuild_world_book_st_book(
        &self,
        id: &str,
        force: bool,
    ) -> CoreResult<(PartnerItem, Vec<Value>, bool)> {
        let wb = self.get_world_book(id)?;
        let had_raw = wb
            .fields
            .as_ref()
            .and_then(|f| f.get("stBookRaw"))
            .map(|b| {
                b.get("entries")
                    .map(|e| e.as_array().map(|a| !a.is_empty()).unwrap_or(e.as_object().map(|m| !m.is_empty()).unwrap_or(false)))
                    .unwrap_or(false)
            })
            .unwrap_or(false);

        let entries = crate::entries_from_world_book(&wb.name, wb.fields.as_ref(), &wb.content);
        if entries.is_empty() {
            return Err(CoreError::BadRequest(format!(
                "world book {id} has no rebuildable entries (empty content and no ST raw)"
            )));
        }
        let values: Vec<Value> = entries.iter().map(crate::wi_entry_to_st_json).collect();
        // put_world_book_entries always writes stBookRaw/stEntries/content
        let item = self.put_world_book_entries(id, values.clone())?;
        let list = self.list_world_book_entries(id)?;
        Ok((item, list, had_raw && !force))
    }

    /// Rebuild ST book for a character card.
    ///
    /// Priority:
    /// 1. Linked `world_book_id` → rebuild that world book
    /// 2. Card fields carry `character_book` / `stBookRaw` → upsert linked world book then rebuild
    /// 3. Else if card has non-empty content and `create_from_content` → create constant-entry WB + link
    pub fn rebuild_character_card_st_book(
        &self,
        id: &str,
        force: bool,
        create_from_content: bool,
    ) -> CoreResult<(PartnerItem, PartnerItem, Vec<Value>, bool)> {
        let st = self.load()?;
        let cc = st
            .character_cards
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("character card {id}")))?;

        // 1) linked world book
        if let Some(ref wid) = cc.world_book_id {
            if st.world_books.iter().any(|w| &w.id == wid) {
                drop(st);
                let (wb, entries, already) = self.rebuild_world_book_st_book(wid, force)?;
                let cc = self
                    .load()?
                    .character_cards
                    .into_iter()
                    .find(|c| c.id == id)
                    .ok_or_else(|| CoreError::NotFound(format!("character card {id}")))?;
                return Ok((cc, wb, entries, already));
            }
        }

        // 2) embedded book on card fields
        let fields = cc.fields.clone().unwrap_or_else(|| json!({}));
        let embedded = fields
            .get("character_book")
            .or_else(|| fields.get("stBookRaw"))
            .or_else(|| fields.get("stCharacterBookRaw"))
            .cloned();
        if let Some(book) = embedded {
            // synthesize PartnerItem via temporary StCardData-like path
            let book_name = book
                .get("name")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{} · 角色世界书", cc.name));
            let mut wb_item = PartnerItem {
                id: String::new(),
                name: book_name.clone(),
                item_type: "world_book".into(),
                content: String::new(),
                fields: Some(json!({
                    "theme": book_name,
                    "stCharacterBook": true,
                    "stSourceCharacter": cc.name,
                    "stBookRaw": book,
                })),
                world_book_id: None,
            };
            // materialize content from entries
            let ents = crate::entries_from_world_book(
                &book_name,
                wb_item.fields.as_ref(),
                "",
            );
            wb_item.content = crate::content_from_wi_entries(&book_name, &ents);
            let saved = self.upsert_world_book(wb_item)?;
            // link card
            let mut st = self.load()?;
            if let Some(slot) = st.character_cards.iter_mut().find(|c| c.id == id) {
                slot.world_book_id = Some(saved.id.clone());
                if let Some(f) = slot.fields.get_or_insert_with(|| json!({})).as_object_mut() {
                    f.insert("stHasCharacterBook".into(), json!(true));
                    f.insert("stLoreEntryCount".into(), json!(ents.len()));
                }
            }
            self.save(st)?;
            let (wb, entries, already) = self.rebuild_world_book_st_book(&saved.id, force)?;
            let cc = self
                .load()?
                .character_cards
                .into_iter()
                .find(|c| c.id == id)
                .ok_or_else(|| CoreError::NotFound(format!("character card {id}")))?;
            return Ok((cc, wb, entries, already));
        }

        // 3) create from freeform card content
        if create_from_content && !cc.content.trim().is_empty() {
            let book_name = format!("{} · 迁移世界书", cc.name);
            let wb_item = PartnerItem {
                id: String::new(),
                name: book_name.clone(),
                item_type: "world_book".into(),
                content: cc.content.clone(),
                fields: Some(json!({
                    "theme": book_name,
                    "stMigratedFromCard": true,
                    "stSourceCharacter": cc.name,
                })),
                world_book_id: None,
            };
            let saved = self.upsert_world_book(wb_item)?;
            let mut st = self.load()?;
            if let Some(slot) = st.character_cards.iter_mut().find(|c| c.id == id) {
                slot.world_book_id = Some(saved.id.clone());
            }
            self.save(st)?;
            let (wb, entries, already) = self.rebuild_world_book_st_book(&saved.id, force)?;
            let cc = self
                .load()?
                .character_cards
                .into_iter()
                .find(|c| c.id == id)
                .ok_or_else(|| CoreError::NotFound(format!("character card {id}")))?;
            return Ok((cc, wb, entries, already));
        }

        Err(CoreError::BadRequest(format!(
            "character card {id} has no linked world book, embedded character_book, or rebuildable content"
        )))
    }

    /// Rebuild all world books. When `force=false`, still visits every book but
    /// `already=true` marks those that already had stBookRaw (re-normalized only).
    /// Returns vec of (world_book_id, entry_count, already_had_raw).
    pub fn migrate_legacy_world_books(
        &self,
        force: bool,
    ) -> CoreResult<Vec<(String, usize, bool)>> {
        let st = self.load()?;
        let ids: Vec<String> = st.world_books.iter().map(|w| w.id.clone()).collect();
        let mut out = Vec::new();
        for id in ids {
            match self.rebuild_world_book_st_book(&id, force) {
                Ok((_item, entries, already)) => out.push((id, entries.len(), already)),
                Err(_) => {}
            }
        }
        Ok(out)
    }

    pub fn delete_character_card(&self, id: &str) -> CoreResult<()> {
        let mut st = self.load()?;
        st.character_cards.retain(|x| x.id != id);
        if st.selected_character_card_id.as_deref() == Some(id) {
            st.selected_character_card_id = None;
        }
        self.save(st)?;
        Ok(())
    }

    pub fn select(
        &self,
        world_book_id: Option<String>,
        character_card_id: Option<String>,
    ) -> CoreResult<PartnerState> {
        let mut st = self.load()?;
        if let Some(ref id) = world_book_id {
            if id.is_empty() {
                st.selected_world_book_id = None;
            } else if st.world_books.iter().any(|w| &w.id == id) {
                st.selected_world_book_id = Some(id.clone());
            } else {
                return Err(CoreError::NotFound(format!("world book {id}")));
            }
        }
        if let Some(ref id) = character_card_id {
            if id.is_empty() {
                st.selected_character_card_id = None;
            } else if st.character_cards.iter().any(|c| &c.id == id) {
                st.selected_character_card_id = Some(id.clone());
            } else {
                return Err(CoreError::NotFound(format!("character card {id}")));
            }
        }
        self.save(st)
    }

    /// Build partner chat system prompt like MobileChat.tsx
    ///
    /// Legacy entry — no chat scan (constant entries + full legacy books only via
    /// empty chat buffer). Prefer [`Self::build_generation_prompt`].
    pub fn build_system_prompt(
        &self,
        base_prompt: &str,
        world_book_id: Option<&str>,
        character_card_id: Option<&str>,
    ) -> CoreResult<String> {
        self.build_generation_prompt(base_prompt, world_book_id, character_card_id, &[], None)
            .map(|r| r.system_prompt)
    }

    /// ST-aligned generation prompt: World Info scan + character card + regex (prompt path).
    pub fn build_generation_prompt(
        &self,
        base_prompt: &str,
        world_book_id: Option<&str>,
        character_card_id: Option<&str>,
        chat_messages_oldest_first: &[(String, String)],
        wi_settings: Option<crate::WiSettings>,
    ) -> CoreResult<GenerationPromptResult> {
        self.build_generation_prompt_ex(
            base_prompt,
            world_book_id,
            character_card_id,
            chat_messages_oldest_first,
            wi_settings,
            None,
            false,
        )
    }

    /// Extended: pass prior timed state; set dry_run to skip persisting sticky/cooldown writes.
    pub fn build_generation_prompt_ex(
        &self,
        base_prompt: &str,
        world_book_id: Option<&str>,
        character_card_id: Option<&str>,
        chat_messages_oldest_first: &[(String, String)],
        wi_settings: Option<crate::WiSettings>,
        timed_in: Option<crate::TimedWorldInfo>,
        dry_run: bool,
    ) -> CoreResult<GenerationPromptResult> {
        self.build_generation_prompt_full(
            base_prompt,
            world_book_id,
            character_card_id,
            chat_messages_oldest_first,
            wi_settings,
            timed_in,
            dry_run,
            None,
            8192,
        )
    }

    pub fn build_generation_prompt_full(
        &self,
        base_prompt: &str,
        world_book_id: Option<&str>,
        character_card_id: Option<&str>,
        chat_messages_oldest_first: &[(String, String)],
        wi_settings: Option<crate::WiSettings>,
        timed_in: Option<crate::TimedWorldInfo>,
        dry_run: bool,
        scan_ctx: Option<crate::WiScanContext>,
        max_context_tokens: i32,
    ) -> CoreResult<GenerationPromptResult> {
        let st = self.load()?;
        let wb_id = world_book_id
            .map(|s| s.to_string())
            .or(st.selected_world_book_id.clone());
        let cc_id = character_card_id
            .map(|s| s.to_string())
            .or(st.selected_character_card_id.clone());

        let mut prompt = base_prompt.trim().to_string();
        if prompt.is_empty() {
            prompt = "你是一个体贴温和的伴侣，请用温暖、真实而细节丰富的语言与用户交谈，避免机器感。"
                .into();
        }

        let mut settings = wi_settings.unwrap_or_default();
        // W4: default token estimate mode from PublicSettings when not set on request.
        if settings.token_estimate_mode.trim().is_empty() {
            if let Ok(ps) = self.app_state.load_settings_public() {
                let m = ps.token_estimate_mode.trim();
                if !m.is_empty() {
                    settings.token_estimate_mode = m.to_string();
                }
            }
        }
        let scan_buf = crate::chat_to_scan_buffer(chat_messages_oldest_first);

        // Collect WI entries from selected world book (+ card-linked book if different)
        let mut entries: Vec<crate::WiEntry> = Vec::new();
        let mut wb_ids: Vec<String> = Vec::new();
        if let Some(ref id) = wb_id {
            wb_ids.push(id.clone());
        }
        if let Some(ref id) = cc_id {
            if let Some(cc) = st.character_cards.iter().find(|c| c.id == *id) {
                if let Some(ref wid) = cc.world_book_id {
                    if !wb_ids.iter().any(|x| x == wid) {
                        wb_ids.push(wid.clone());
                    }
                }
            }
        }
        for id in &wb_ids {
            if let Some(wb) = st.world_books.iter().find(|w| w.id == *id) {
                let name = if wb.name.is_empty() { id.clone() } else { wb.name.clone() };
                entries.extend(crate::entries_from_world_book(
                    &name,
                    wb.fields.as_ref(),
                    &wb.content,
                ));
            }
        }

        let mut ctx = scan_ctx.unwrap_or_default();
        if ctx.char_name.is_empty() {
            if let Some(ref id) = cc_id {
                if let Some(cc) = st.character_cards.iter().find(|c| c.id == *id) {
                    ctx.char_name = cc.name.clone();
                    ctx.character_name = cc.name.clone();
                    // tags from fields.identityTags if any
                    if let Some(fields) = cc.fields.as_ref() {
                        let mut tags: Vec<String> = Vec::new();
                        if let Some(s) = fields.get("identityTags").and_then(|v| v.as_str()) {
                            tags.extend(s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()));
                        }
                        if let Some(arr) = fields.get("tags").and_then(|v| v.as_array()) {
                            for x in arr {
                                if let Some(s) = x.as_str() {
                                    let t = s.trim();
                                    if !t.is_empty() { tags.push(t.to_string()); }
                                }
                            }
                        }
                        if let Some(st) = fields.get("stSource") {
                            if let Some(arr) = st.get("tags").and_then(|v| v.as_array()) {
                                for x in arr {
                                    if let Some(s) = x.as_str() {
                                        let t = s.trim();
                                        if !t.is_empty() { tags.push(t.to_string()); }
                                    }
                                }
                            }
                        }
                        tags.sort();
                        tags.dedup();
                        ctx.character_tags = tags;
                    }
                }
            }
        }
        if ctx.user_name.is_empty() {
            ctx.user_name = "User".into();
        }
        if ctx.trigger.is_empty() {
            ctx.trigger = "normal".into();
        }
        if ctx.max_context_tokens <= 0 {
            ctx.max_context_tokens = max_context_tokens.max(1024);
        }
        let max_ctx = ctx.max_context_tokens;
        let scan = crate::check_world_info_timed(
            &entries,
            &scan_buf,
            max_ctx,
            &settings,
            timed_in,
            dry_run,
            Some(ctx.clone()),
        );
        let wi_block = crate::format_wi_for_system(&scan); // combined; sections used below

        // Regex scripts: global library ∪ character-scoped (W6). Card > library by default.
        let lib_store = crate::RegexLibraryStore::new(self.app_state.data_root());
        let mut cc_content = String::new();
        let scripts = if let Some(ref id) = cc_id {
            if let Some(cc) = st.character_cards.iter().find(|c| c.id == *id) {
                cc_content = cc.content.trim().to_string();
                crate::resolve_runtime_scripts(&lib_store, cc.fields.as_ref())
            } else {
                lib_store.parsed_scripts()
            }
        } else {
            // No card — still apply global library (W6 gate)
            lib_store.parsed_scripts()
        };

        // Apply promptOnly / dual-mode regex to WI + card before assembly (placement WORLD_INFO / AI paths)
        let _wi_block_regexed = if !wi_block.is_empty() {
            crate::get_regexed_string(
                &wi_block,
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                None,
            )
        } else {
            wi_block
        };

        // Assemble like ST: before WI + base + card + after WI
        // We keep Kaleido section headers for clarity.
        if !scan.world_info_before.trim().is_empty() {
            let before = crate::get_regexed_string(
                scan.world_info_before.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                None,
            );
            prompt.push_str(&format!("\n\n## 世界书（前置）\n{before}"));
        }
        if !scan.an_before.trim().is_empty() {
            let an = crate::get_regexed_string(
                scan.an_before.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                None,
            );
            prompt.push_str(&format!("\n\n## 作者注释（顶）\n{an}"));
        }
        if !cc_content.is_empty() {
            let cc_rx = crate::get_regexed_string(
                &cc_content,
                crate::RegexPlacement::AiOutput,
                &scripts,
                false,
                true,
                None,
            );
            prompt.push_str(&format!("\n\n## 你的角色人设设定（伴侣设定）\n{cc_rx}"));
        } else if let Some(ref id) = cc_id {
            let _ = id;
        }
        if !scan.em_before.trim().is_empty() {
            let em = crate::get_regexed_string(
                scan.em_before.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                None,
            );
            prompt.push_str(&format!("\n\n## 示例消息（前）\n{em}"));
        }
        if !scan.world_info_after.trim().is_empty() {
            let after = crate::get_regexed_string(
                scan.world_info_after.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                None,
            );
            prompt.push_str(&format!("\n\n## 世界书（后置）\n{after}"));
        }
        if !scan.an_after.trim().is_empty() {
            let an = crate::get_regexed_string(
                scan.an_after.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                None,
            );
            prompt.push_str(&format!("\n\n## 作者注释（底）\n{an}"));
        }
        if !scan.em_after.trim().is_empty() {
            let em = crate::get_regexed_string(
                scan.em_after.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                None,
            );
            prompt.push_str(&format!("\n\n## 示例消息（后）\n{em}"));
        }
        for o in &scan.outlet_entries {
            if o.content.trim().is_empty() { continue; }
            let body = crate::get_regexed_string(
                o.content.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                None,
            );
            prompt.push_str(&format!("\n\n## 世界书出口（{}）\n{}", o.name, body));
        }
        for d in &scan.depth_entries {
            if d.content.trim().is_empty() { continue; }
            let body = crate::get_regexed_string(
                d.content.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                Some(d.depth),
            );
            prompt.push_str(&format!(
                "\n\n## 世界书（深度 {}）\n{}",
                d.depth, body
            ));
        }

        // If scan empty but legacy book had only freeform and constants already handled —
        // also: if no structured entries matched and book has legacy constant, scan covers it.

        // Fallback: if zero entries parsed but wb content exists, inject full content (compat)
        if entries.is_empty() {
            if let Some(ref id) = wb_id {
                if let Some(wb) = st.world_books.iter().find(|w| w.id == *id) {
                    let content = wb.content.trim();
                    if !content.is_empty() {
                        prompt.push_str(&format!("\n\n## 伴侣对话世界设定\n{content}"));
                    }
                }
            }
        }

        // Multi-slot injections for chat transcript (ST IN_CHAT / examples / outlets)
        let mut message_injections = Vec::new();
        for inj in &scan.prompt_slots.chat_injections {
            if inj.content.trim().is_empty() {
                continue;
            }
            let body = crate::get_regexed_string(
                inj.content.trim(),
                crate::RegexPlacement::WorldInfo,
                &scripts,
                false,
                true,
                Some(inj.depth),
            );
            // Prefer not duplicating EM/outlet already mirrored in system sections for depth-only?
            // Include all slots as explicit messages so clients/PromptManager can place them.
            if inj.kind == "depth" || inj.kind.starts_with("outlet:") {
                message_injections.push(serde_json::json!({
                    "role": inj.role,
                    "content": body,
                    "depth": inj.depth,
                    "kind": inj.kind,
                    "wiSlot": true,
                }));
            } else if inj.kind == "em_before" || inj.kind == "em_after" {
                // Also as example-style system messages tagged for UI
                message_injections.push(serde_json::json!({
                    "role": "system",
                    "content": body,
                    "depth": inj.depth,
                    "kind": inj.kind,
                    "wiSlot": true,
                    "example": true,
                }));
            }
        }

        // Prefer structured example pairs at head of injections (ST mesExamples before chat)
        let example_messages = scan.example_messages.clone();
        let mut ordered_inj = Vec::new();
        for ex in &example_messages {
            ordered_inj.push(serde_json::json!({
                "role": ex.role,
                "content": ex.content,
                "depth": 0,
                "kind": format!("em_example_{}", ex.anchor),
                "wiSlot": true,
                "example": true,
            }));
        }
        // dedupe: skip em_example_* already in message_injections from chat_injections path
        for inj in message_injections {
            let kind = inj.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            if kind.starts_with("em_example_") {
                continue; // already from example_messages
            }
            ordered_inj.push(inj);
        }
        let automation_ids = scan.automation_ids.clone();
        let timed_out = scan.timed_world_info.clone();
        Ok(GenerationPromptResult {
            system_prompt: prompt,
            wi: scan,
            regex_script_count: scripts.len(),
            timed_world_info: timed_out,
            message_injections: ordered_inj,
            automation_ids,
            example_messages,
        })
    }
}

