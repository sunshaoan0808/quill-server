//! author/publish_inject —— publish 发布入 Works + inject 注入会话。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PublishBody {
    /// lore | chapter | worldBook | promoteLive
    kind: String,
    /// works-relative path under project (or absolute works path projects/{id}/...)
    #[serde(default)]
    path: Option<String>,
    /// inline content if path omitted
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    title: Option<String>,
    /// for worldBook: target book id (default first project worldBookId)
    #[serde(default)]
    target_world_book_id: Option<String>,
    /// for chapter: pack chapter id to overwrite body
    #[serde(default)]
    chapter_id: Option<String>,
    /// for lore: permanent entry (default true)
    #[serde(default)]
    permanent: Option<bool>,
    /// for promoteLive: destination under canon/ (default canon/from-live-{ts}.md)
    #[serde(default)]
    dest_path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InjectBody {
    session_id: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    content: Option<String>,
    /// system | director (default system)
    #[serde(default)]
    as_role: Option<String>,
}

pub(crate) fn resolve_publish_text(
    state: &AppState,
    workspace_id: &str,
    project: &AuthorProject,
    path: Option<&str>,
    content: Option<&str>,
) -> Result<(String, Option<String>), Response> {
    if let Some(c) = content.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok((c.to_string(), path.map(|p| p.to_string())));
    }
    let rel = path
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            bad_request("AUTHOR_PATH_REQUIRED",  "path or content required")
        })?;
    // allow projects/{id}/... or relative to works_root
    let full = if rel.starts_with("projects/") {
        rel.to_string()
    } else {
        format!(
            "{}/{}",
            project.works_root.trim_end_matches('/'),
            rel.trim_start_matches('/')
        )
    };
    if !full.starts_with(&project.works_root) && !full.starts_with(&format!("{}/", project.works_root.trim_end_matches('/'))) {
        // still ok if under projects/{id}
        let prefix = format!("projects/{}/", project.id);
        if !full.starts_with(&prefix) {
            return Err(bad_request("AUTHOR_PATH_ESCAPES",  "path must be under project worksRoot"));
        }
    }
    match state.works.read_text(workspace_id, &full) {
        Ok(body) => Ok((body.content, Some(full))),
        Err(e) => Err(map_core_err(e)),
    }
}

pub(crate) fn lore_entry_from_text(title: &str, text: &str, permanent: bool) -> serde_json::Value {
    let keys: Vec<String> = title
        .split(|c: char| c.is_whitespace() || c == '/' || c == '-' || c == '_')
        .filter(|s| s.len() >= 2)
        .take(8)
        .map(|s| s.to_string())
        .collect();
    json!({
        "id": format!("lore-az-{}", Uuid::new_v4()),
        "title": title,
        "content": text,
        "keys": keys,
        "permanent": permanent,
        "source": "author-publish",
        "updatedAt": Utc::now().to_rfc3339(),
    })
}

pub(crate) fn ensure_project_pack(
    state: &AppState,
    project: &mut AuthorProject,
    partner_chars: Vec<PackCharacterRef>,
) -> Result<String, Response> {
    if let Some(id) = project.pack_id.clone().filter(|s| !s.is_empty()) {
        if state.packs.get(&id).is_ok() {
            return Ok(id);
        }
    }
    let chapters = split_chapters("");
    let pack = build_composed_pack(
        project,
        &project.title,
        partner_chars,
        project.world_book_ids.clone(),
        chapters,
    );
    match state.packs.save(pack) {
        Ok(saved) => {
            let _ = state.packs.write_chapter_body(
                &saved.id,
                "chapters/ch01.md",
                &format!("# {}\n\n（发布自动建包占位）\n", project.title),
            );
            let _ = state.packs.write_chapter_body(
                &saved.id,
                "chapters/ch02.md",
                "# 第2章\n\n（占位）\n",
            );
            project.pack_id = Some(saved.id.clone());
            project.updated_at = now();
            Ok(saved.id)
        }
        Err(e) => Err(map_core_err(e)),
    }
}

pub(crate) async fn publish_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<PublishBody>,
) -> Response {
    let sess = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let _index_guard = AUTHOR_INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = match load_index_unlocked(&state) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let pos = match list.iter().position(|p| p.id == id && p.workspace_id == sess.workspace_id) {
        Some(i) => i,
        None => {
            return not_found("AUTHOR_NOT_FOUND",  "project not found")
        }
    };

    let kind = body.kind.trim().to_lowercase();

    // Auto-build pack for lore/chapter if compose was skipped (P1 legacy)
    if matches!(kind.as_str(), "lore" | "chapter") {
        let partner = match state.partner.clone().scoped(&sess.user_id).load() {
            Ok(p) => p,
            Err(e) => return map_core_err(e),
        };
        let mut pack_chars: Vec<PackCharacterRef> = Vec::new();
        for cid in &list[pos].character_ids {
            if let Some(cc) = partner.character_cards.iter().find(|c| &c.id == cid) {
                pack_chars.push(partner_to_pack_char(cc));
            } else {
                pack_chars.push(PackCharacterRef {
                    id: cid.clone(),
                    name: cid.clone(),
                    role: "character".into(),
                    importance: "medium".into(),
                    gender: "未知".into(),
                    appearance: "未知".into(),
                    opening_scene: "未知".into(),
                    opening_lines: "".into(),
                    nsfw_profile: String::new(),
                    content_tier: Some(ContentTier::Standard),
                    example_dialogs: vec![],
                    boundaries: vec![],
                    personality: String::new(),
                    speech_style: String::new(),
                    voice_profile: String::new(),
                    motivation: String::new(),
                    relationships: vec![],
                    evidence_refs: vec![],
                    mental_models: vec![],
                    decision_heuristics: vec![],
                    beliefs: vec![],
                    expressions: Default::default(),
            voice: None,
            archive: None,
                    avatar: None,
                    starting_wardrobe: Default::default(),
                });
            }
        }
        match ensure_project_pack(&state, &mut list[pos], pack_chars) {
            Ok(_) => {
                if let Err(r) = write_project_mirror(&state.works, &sess.workspace_id, &list[pos]) {
                    return r;
                }
                if let Err(r) = save_index_unlocked(&state, &list) {
                    return r;
                }
            }
            Err(r) => return r,
        }
    }
    let kind = match kind.as_str() {
        "lore" | "chapter" | "worldbook" | "world_book" | "promotelive" | "promote_live" | "promote-live" => kind,
        _ => {
            return bad_request("AUTHOR_BAD_KIND",  "kind must be lore|chapter|worldBook|promoteLive")
        }
    };

    // promoteLive: copy live path content into canon/
    if kind == "promotelive" || kind == "promote_live" || kind == "promote-live" {
        let (text, src) = match resolve_publish_text(
            &state,
            &sess.workspace_id,
            &list[pos],
            body.path.as_deref(),
            body.content.as_deref(),
        ) {
            Ok(v) => v,
            Err(r) => return r,
        };
        let dest = body
            .dest_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s.starts_with("projects/") {
                    s.to_string()
                } else {
                    format!(
                        "{}/{}",
                        list[pos].works_root.trim_end_matches('/'),
                        s.trim_start_matches('/')
                    )
                }
            })
            .unwrap_or_else(|| {
                format!(
                    "{}/canon/from-live-{}.md",
                    list[pos].works_root.trim_end_matches('/'),
                    Utc::now().format("%Y%m%d-%H%M%S")
                )
            });
        // ensure parent via write
        let header = format!(
            "<!-- promoted from {} at {} -->\n\n",
            src.as_deref().unwrap_or("(inline)"),
            Utc::now().to_rfc3339()
        );
        let full = format!("{}{}", header, text);
        if let Some(parent) = std::path::Path::new(&dest).parent() {
            let parent_s = parent.to_string_lossy();
            if !parent_s.is_empty() && parent_s != "." {
                let _ = state.works.mkdir(&sess.workspace_id, &parent_s);
            }
        }
        if let Err(e) = state.works.write_text(&sess.workspace_id, &dest, &full) {
            return map_core_err(e);
        }
        let entry = AuthorDocEntry {
            path: dest.clone(),
            kind: "canon".into(),
            source_session_id: None,
            playable: list[pos].default_playable.clone(),
            updated_at: Utc::now().to_rfc3339(),
        };
        list[pos].doc_index.retain(|d| d.path != dest);
        list[pos].doc_index.push(entry);
        list[pos].updated_at = Utc::now().to_rfc3339();
        if let Err(r) = save_index_unlocked(&state, &list) {
            return r;
        }
        return Json(json!({
            "ok": true,
            "kind": "promoteLive",
            "destPath": dest,
            "bytes": full.len(),
            "project": list[pos],
        }))
        .into_response();
    }

    let (text, src_path) = match resolve_publish_text(
        &state,
        &sess.workspace_id,
        &list[pos],
        body.path.as_deref(),
        body.content.as_deref(),
    ) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let title = body
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            src_path.as_ref().map(|p| {
                std::path::Path::new(p)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("设定")
                    .to_string()
            })
        })
        .unwrap_or_else(|| "作者发布".into());

    if kind == "lore" {
        let pack_id = match list[pos].pack_id.clone() {
            Some(id) if !id.is_empty() => id,
            _ => {
                return bad_request("AUTHOR_NO_PACK",  "project has no packId; compose first")
            }
        };
        let mut pack = match state.packs.get(&pack_id) {
            Ok(p) => p,
            Err(e) => return map_core_err(e),
        };
        let permanent = body.permanent.unwrap_or(true);
        let entry = lore_entry_from_text(&title, &text, permanent);
        let entry_id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
        pack.lore_entries.push(entry);
        pack.updated_at = Utc::now().to_rfc3339();
        let saved = match state.packs.save(pack) {
            Ok(p) => p,
            Err(e) => return map_core_err(e),
        };
        list[pos].updated_at = Utc::now().to_rfc3339();
        let _ = save_index_unlocked(&state, &list);
        return Json(json!({
            "ok": true,
            "kind": "lore",
            "loreId": entry_id,
            "packId": saved.id,
            "loreCount": saved.lore_entries.len(),
            "sourcePath": src_path,
        }))
        .into_response();
    }

    if kind == "chapter" {
        let pack_id = match list[pos].pack_id.clone() {
            Some(id) if !id.is_empty() => id,
            _ => {
                return bad_request("AUTHOR_NO_PACK",  "project has no packId; compose first")
            }
        };
        let mut pack = match state.packs.get(&pack_id) {
            Ok(p) => p,
            Err(e) => return map_core_err(e),
        };
        if pack.chapters.is_empty() {
            return bad_request("AUTHOR_PACK_EMPTY",  "pack has no chapters");
        }
        let ch_id = body
            .chapter_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let ch_idx = if let Some(ref cid) = ch_id {
            pack.chapters.iter().position(|c| &c.id == cid)
        } else {
            Some(0)
        };
        let Some(ci) = ch_idx else {
            return bad_request("AUTHOR_CHAPTER_MISSING",  "chapterId not found in pack");
        };
        let body_path = if pack.chapters[ci].body_path.trim().is_empty() {
            format!("chapters/ch{:02}.md", pack.chapters[ci].order.max(1))
        } else {
            pack.chapters[ci].body_path.clone()
        };
        if let Err(e) = state.packs.write_chapter_body(&pack_id, &body_path, &text) {
            return map_core_err(e);
        }
        pack.chapters[ci].body_path = body_path.clone();
        if !title.is_empty() {
            pack.chapters[ci].title = title.clone();
        }
        pack.updated_at = Utc::now().to_rfc3339();
        let saved = match state.packs.save(pack) {
            Ok(p) => p,
            Err(e) => return map_core_err(e),
        };
        list[pos].updated_at = Utc::now().to_rfc3339();
        let _ = save_index_unlocked(&state, &list);
        return Json(json!({
            "ok": true,
            "kind": "chapter",
            "packId": saved.id,
            "chapterId": saved.chapters.get(ci).map(|c| c.id.clone()),
            "bodyPath": body_path,
            "note": "已开 mainline 会话请新节点或重开以读到新正文",
            "sourcePath": src_path,
        }))
        .into_response();
    }

    // worldBook
    if kind == "worldbook" || kind == "world_book" {
        let wb_id = body
            .target_world_book_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| list[pos].world_book_ids.first().cloned());
        let Some(wb_id) = wb_id else {
            return bad_request("AUTHOR_WB_TARGET_MISSING",  "no targetWorldBookId and project has no worldBookIds");
        };
        let mut partner = match state.partner.clone().scoped(&sess.user_id).load() {
            Ok(p) => p,
            Err(e) => return map_core_err(e),
        };
        let block = format!("\n\n## {}\n\n{}\n", title, text.trim());
        let mut found = false;
        for w in partner.world_books.iter_mut() {
            if w.id == wb_id {
                if !w.content.contains(text.trim()) {
                    w.content.push_str(&block);
                }
                found = true;
                break;
            }
        }
        if !found {
            // create new world book bound to project
            let item = PartnerItem {
                id: wb_id.clone(),
                name: title.clone(),
                item_type: "world_book".into(),
                content: format!("# {}\n\n{}", title, text.trim()),
                fields: None,
                world_book_id: None,
            };
            partner.world_books.push(item);
            if !list[pos].world_book_ids.iter().any(|x| x == &wb_id) {
                list[pos].world_book_ids.push(wb_id.clone());
            }
        }
        // upsert via partner store
        // 不变量：上方两分支必居其一 —— 原地更新（found）或新建 push，wb_id 必存在
        let item = partner
            .world_books
            .iter()
            .find(|w| w.id == wb_id)
            .cloned()
            .expect("world book present: updated in place or just pushed");
        match state.partner.clone().scoped(&sess.user_id).upsert_world_book(item) {
            Ok(saved_item) => {
                list[pos].updated_at = Utc::now().to_rfc3339();
                let _ = save_index_unlocked(&state, &list);
                return Json(json!({
                    "ok": true,
                    "kind": "worldBook",
                    "worldBookId": saved_item.id,
                    "name": saved_item.name,
                    "contentLen": saved_item.content.len(),
                    "sourcePath": src_path,
                }))
                .into_response();
            }
            Err(e) => return map_core_err(e),
        }
    }

    bad_request("AUTHOR_BAD_KIND",  "unhandled kind")
}

pub(crate) async fn inject_to_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<InjectBody>,
) -> Response {
    let sess = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let list = match load_index(&state) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let project = match list.iter().find(|p| p.id == id && p.workspace_id == sess.workspace_id) {
        Some(p) => p.clone(),
        None => {
            return not_found("AUTHOR_NOT_FOUND",  "project not found")
        }
    };
    let sid = body.session_id.trim();
    if sid.is_empty() {
        return bad_request("AUTHOR_SESSION_REQUIRED",  "sessionId required");
    }
    let (text, src_path) = match resolve_publish_text(
        &state,
        &sess.workspace_id,
        &project,
        body.path.as_deref(),
        body.content.as_deref(),
    ) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut tavern = match state.sessions_tavern.get(sid) {
        Ok(t) => t,
        Err(e) => return map_core_err(e),
    };
    // soft check: prefer sessions bound to this project
    if let Some(ref pid) = tavern.author_project_id {
        if pid != &project.id {
            // allow but warn in response
        }
    } else {
        tavern.author_project_id = Some(project.id.clone());
    }
    let role = match body
        .as_role
        .as_deref()
        .unwrap_or("system")
        .trim()
        .to_lowercase()
        .as_str()
    {
        "director" | "dm" | "note" => "system",
        _ => "system",
    };
    let clipped: String = text.chars().take(6000).collect();
    let msg = kaleido_core::TavernMessage {
        id: format!("inj-{}", Uuid::new_v4()),
        role: role.into(),
        reasoning: None,
        content: format!(
            "【作者区同步 · 一次性】
来源: {}

{}",
            src_path.as_deref().unwrap_or("(inline)"),
            clipped
        ),
        created_at: Utc::now().to_rfc3339(),
        options: vec![],
        engine_tag: None,
        program: None,
        swipes: vec![],
        swipe_index: 0,
        tokens: 0,
        mvu_blocks: Vec::new(),
    };
    // Check TavernMessage fields - may differ
    // We'll fix compile if struct differs
    tavern.messages.push(msg);
    tavern.updated_at = Utc::now().to_rfc3339();
    match state.sessions_tavern.save(tavern) {
        Ok(saved) => Json(json!({
            "ok": true,
            "sessionId": saved.session_id,
            "messageCount": saved.messages.len(),
            "injected": true,
            "note": "一次性 system 注，不改写历史对白",
        }))
        .into_response(),
        Err(e) => map_core_err(e),
    }
}
