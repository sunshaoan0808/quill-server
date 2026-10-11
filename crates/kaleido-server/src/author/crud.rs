//! author/crud —— 项目 CRUD 路由 + bind_session + append_session_live。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn list_projects(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match load_index(&state) {
        Ok(list) => {
            let list = filter_workspace(list, &session.workspace_id);
            Json(json!({"ok": true, "count": list.len(), "projects": list})).into_response()
        }
        Err(r) => r,
    }
}

pub(crate) async fn create_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateProjectBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let id = format!("ap-{}", Uuid::new_v4());
    let title = body
        .title
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "未命名创作".into());
    let ts = now();
    let works_root = format!("projects/{id}/");
    let project = AuthorProject {
        id: id.clone(),
        title,
        character_ids: body.character_ids,
        world_book_ids: body.world_book_ids,
        pack_id: body
            .pack_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        works_root: works_root.clone(),
        default_playable: body
            .default_playable
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        doc_index: vec![],
        live_policy: body
            .live_policy
            .unwrap_or_default()
            .normalize(),
        created_at: ts.clone(),
        updated_at: ts,
        workspace_id: session.workspace_id.clone(),
    };

    if let Err(r) = scaffold_project_dirs(&state.works, &session.workspace_id, &id) {
        return r;
    }
    if let Err(r) = write_project_mirror(&state.works, &session.workspace_id, &project) {
        return r;
    }

    let project_out = match with_index_mut(&state, |list| {
        list.push(project.clone());
        Ok(project.clone())
    }) {
        Ok(p) => p,
        Err(r) => return r,
    };

    (
        StatusCode::CREATED,
        Json(json!({
            "ok": true,
            "project": project_out,
        })),
    )
        .into_response()
}

pub(crate) async fn get_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let id = match safe_project_id(&id) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let list = match load_index(&state) {
        Ok(l) => filter_workspace(l, &session.workspace_id),
        Err(r) => return r,
    };
    match list.into_iter().find(|p| p.id == id) {
        Some(p) => Json(json!({"ok": true, "project": p})).into_response(),
        None => not_found("AUTHOR_NOT_FOUND", format!("project not found: {id}")),
    }
}

pub(crate) async fn patch_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<PatchProjectBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let id = match safe_project_id(&id) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let _index_guard = AUTHOR_INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = match load_index_unlocked(&state) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let Some(pos) = list.iter().position(|p| {
        p.id == id && (p.workspace_id.is_empty() || p.workspace_id == session.workspace_id)
    }) else {
        return not_found("AUTHOR_NOT_FOUND", format!("project not found: {id}"));
    };

    let p = &mut list[pos];
    if let Some(t) = body.title {
        let t = t.trim().to_string();
        if !t.is_empty() {
            p.title = t;
        }
    }
    if let Some(v) = body.character_ids {
        p.character_ids = v;
    }
    if let Some(v) = body.world_book_ids {
        p.world_book_ids = v;
    }
    if let Some(v) = body.pack_id {
        p.pack_id = v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    if let Some(v) = body.default_playable {
        p.default_playable = v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    if let Some(v) = body.doc_index {
        p.doc_index = v;
    }
    if let Some(v) = body.live_policy {
        p.live_policy = v.normalize();
    }
    p.updated_at = now();
    if p.workspace_id.is_empty() {
        p.workspace_id = session.workspace_id.clone();
    }

    let project = p.clone();
    if let Err(r) = write_project_mirror(&state.works, &session.workspace_id, &project) {
        return r;
    }
    if let Err(r) = save_index_unlocked(&state, &list) {
        return r;
    }
    Json(json!({"ok": true, "project": project})).into_response()
}

pub(crate) async fn delete_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let id = match safe_project_id(&id) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let _index_guard = AUTHOR_INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = match load_index_unlocked(&state) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let before = list.len();
    list.retain(|p| {
        !(p.id == id && (p.workspace_id.is_empty() || p.workspace_id == session.workspace_id))
    });
    if list.len() == before {
        return not_found("AUTHOR_NOT_FOUND", format!("project not found: {id}"));
    }
    // Soft-delete index only; keep works tree for recovery (user can wipe via works UI).
    if let Err(r) = save_index_unlocked(&state, &list) {
        return r;
    }
    Json(json!({"ok": true, "deleted": id})).into_response()
}

/// Bind a tavern session to this project: set authorProjectId + authorLivePath, ensure session dir.
pub(crate) async fn bind_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<BindSessionBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let id = match safe_project_id(&id) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let sid = body.session_id.trim().to_string();
    if sid.is_empty() {
        return bad_request("AUTHOR_SESSION_REQUIRED",  "sessionId required");
    }

    let list = match load_index(&state) {
        Ok(l) => filter_workspace(l, &session.workspace_id),
        Err(r) => return r,
    };
    let Some(project) = list.iter().find(|p| p.id == id) else {
        return not_found("AUTHOR_NOT_FOUND", format!("project not found: {id}"));
    };

    let mut tavern = match state.sessions_tavern.get(&sid) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };

    let live_path = format!("projects/{id}/sessions/{sid}/live.md");
    // ensure session dir under project
    let sess_dir = format!("projects/{id}/sessions/{sid}");
    if let Err(e) = state.works.mkdir(&session.workspace_id, &sess_dir) {
        match state.works.stat(&session.workspace_id, &sess_dir) {
            Ok(st) if st.kind == "dir" => {}
            _ => return map_core_err(e),
        }
    }
    // touch live.md if missing
    if state.works.stat(&session.workspace_id, &live_path).is_err() {
        let header = format!(
            "# {} · live\n\n> session `{sid}` · project `{}` · playable {:?}\n\n",
            project.title, project.id, tavern.playable
        );
        if let Err(e) = state
            .works
            .write_text(&session.workspace_id, &live_path, &header)
        {
            return map_core_err(e);
        }
    }

    tavern.author_project_id = Some(id.clone());
    tavern.author_live_path = Some(live_path.clone());
    let pol = project.live_policy.clone().normalize();
    tavern.author_live_enabled = pol.enabled;
    tavern.author_live_every_n = pol.every_n.max(1);
    tavern.author_live_write_turns = pol.write_turns || pol.write_summary_every_n.is_some();
    let saved = match state.sessions_tavern.save(tavern) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };

    // update docIndex on project
    let _index_guard = AUTHOR_INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut full = match load_index_unlocked(&state) {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Some(p) = full.iter_mut().find(|p| p.id == id) {
        let entry = AuthorDocEntry {
            path: live_path.clone(),
            kind: "live".into(),
            source_session_id: Some(sid.clone()),
            playable: Some(match saved.playable {
                kaleido_core::Playable::P1 => "P1".into(),
                kaleido_core::Playable::P2 => "P2".into(),
                kaleido_core::Playable::P3 => "P3".into(),
                kaleido_core::Playable::P4 => "P4".into(),
            }),
            updated_at: now(),
        };
        if let Some(existing) = p.doc_index.iter_mut().find(|d| d.path == live_path) {
            *existing = entry;
        } else {
            p.doc_index.push(entry);
        }
        p.updated_at = now();
        let _ = write_project_mirror(&state.works, &session.workspace_id, p);
    }
    let _ = save_index_unlocked(&state, &full);

    Json(json!({
        "ok": true,
        "session": saved,
        "authorLivePath": live_path,
        "projectId": id,
    }))
    .into_response()
}


/// AZ-2: append one turn to live.md (fail-open). Call from tavern turn completion.
pub fn append_session_live(
    works: &WorksFs,
    workspace_id: &str,
    live_path: &str,
    turn: u32,
    user_text: &str,
    assistant_text: &str,
    live_enabled: bool,
    every_n: u32,
    write_turns: bool,
) {
    if !live_enabled {
        return;
    }
    let every = every_n.max(1);
    if turn == 0 || turn % every != 0 {
        return;
    }
    let ts = Utc::now().to_rfc3339();
    let chunk = format!(
        "\n---\n## Turn {turn} · {ts}\n\n### 玩家\n\n{}\n\n### 叙事\n\n{}\n",
        user_text.trim(),
        assistant_text.trim()
    );
    if let Err(e) = works.append_text(workspace_id, live_path, &chunk) {
        tracing::warn!(error=%e, %live_path, "author live.md append failed (fail-open)");
    }
    if write_turns {
        // sessions/{sid}/live.md -> sessions/{sid}/turns/{n}.md + summary.md
        if let Some(parent) = std::path::Path::new(live_path).parent() {
            let turns_dir = parent.join("turns");
            let turns_dir_s = turns_dir.to_string_lossy().replace('\\', "/");
            let _ = works.mkdir(workspace_id, &turns_dir_s);
            let turn_path = format!("{turns_dir_s}/{turn:04}.md");
            let body = format!(
                "# Turn {turn} · {ts}\n\n### 玩家\n\n{}\n\n### 叙事\n\n{}\n",
                user_text.trim(),
                assistant_text.trim()
            );
            if let Err(e) = works.write_text(workspace_id, &turn_path, &body) {
                tracing::warn!(error=%e, %turn_path, "author turn slice write failed (fail-open)");
            }
            let summary_path = parent.join("summary.md");
            let summary_s = summary_path.to_string_lossy().replace('\\', "/");
            let summary = format!(
                "# 会话小结（自动）\n\n> 更新至 Turn {turn} · {ts}\n\n## 最近一轮\n\n### 玩家\n\n{}\n\n### 叙事\n\n{}\n\n",
                user_text.trim(),
                assistant_text.trim()
            );
            if let Err(e) = works.write_text(workspace_id, &summary_s, &summary) {
                tracing::warn!(error=%e, path=%summary_s, "author summary write failed (fail-open)");
            }
        }
    }
}


// ─── AZ-3 compose / launch ───────────────────────────────────────────────────
