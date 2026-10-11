//! suggest/chapters —— 章节 CRUD 路由（list/create/get/put/delete）。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn list_chapters(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pid = match safe_id(&pid) {
        Ok(v) => v,
        Err(r) => return r,
    };
    // 扫 chapters 目录
    let mut out = Vec::new();
    let dir = format!("projects/{pid}/chapters");
    if let Ok(entry) = state.works.list(&session.workspace_id, &dir, 1) {
        for child in entry.children {
            if child.kind == "dir" {
                continue;
            }
            let rel = format!("{dir}/{}", child.name);
            if let Ok(body) = state.works.read_text(&session.workspace_id, &rel) {
                if let Ok(ch) = serde_json::from_str::<ChapterDoc>(&body.content) {
                    out.push(json!({"id": ch.id, "title": ch.title, "versionNo": ch.version_no, "updatedAt": ch.updated_at}));
                }
            }
        }
    }
    Json(json!({"ok": true, "count": out.len(), "chapters": out})).into_response()
}

pub(crate) async fn create_chapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
    Json(body): Json<NewChapterBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pid = match safe_id(&pid) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let ch_id = format!("ch-{}", &Uuid::new_v4().to_string()[..8]);
    let ch = ChapterDoc {
        id: ch_id.clone(),
        title: body.title.unwrap_or_else(|| "未命名章节".into()),
        content: body.content.unwrap_or_default(),
        volume: body.volume,
        arc: body.arc,
        version_no: 1,
        updated_at: now(),
        source: "manual".into(),
    };
    let s = serde_json::to_string_pretty(&ch).map_err(|e| internal("CHAPTER_SERIALIZE", e.to_string()));
    let s = match s {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(e) = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}/chapters")) {
        let _ = e;
    }
    if let Err(e) = state.works.write_text(&session.workspace_id, &ch_path(&pid, &ch_id), &s) {
        return map_core_err(e);
    }
    Json(json!({"ok": true, "chapter": ch})).into_response()
}

pub(crate) async fn get_chapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pid, ch)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (pid, ch) = match (safe_id(&pid), safe_id(&ch)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return bad_request("CHAPTER_BAD_ID", "invalid id"),
    };
    match read_ch(&state, &session.workspace_id, &pid, &ch) {
        Ok(doc) => Json(json!({"ok": true, "chapter": doc})).into_response(),
        Err(r) => r,
    }
}

pub(crate) async fn put_chapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pid, ch)): Path<(String, String)>,
    Json(body): Json<PutChapterBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (pid, ch) = match (safe_id(&pid), safe_id(&ch)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return bad_request("CHAPTER_BAD_ID", "invalid id"),
    };
    let mut doc = match read_ch(&state, &session.workspace_id, &pid, &ch) {
        Ok(d) => d,
        Err(r) => return r,
    };
    doc.content = body.content;
    doc.version_no += 1;
    doc.updated_at = now();
    let s = serde_json::to_string_pretty(&doc).map_err(|e| internal("CHAPTER_SERIALIZE", e.to_string()));
    let s = match s {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(e) = state.works.write_text(&session.workspace_id, &ch_path(&pid, &ch), &s) {
        return map_core_err(e);
    }
    Json(json!({"ok": true, "chapter": doc})).into_response()
}

pub(crate) async fn delete_chapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pid, ch)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (pid, ch) = match (safe_id(&pid), safe_id(&ch)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return bad_request("CHAPTER_BAD_ID", "invalid id"),
    };
    if let Err(e) = state.works.delete(&session.workspace_id, &ch_path(&pid, &ch), false) {
        return map_core_err(e);
    }
    Json(json!({"ok": true})).into_response()
}

// ── A2: 建议/守卫/采纳门禁（Scriverse 原样搬，Kaleido 化） ──────────────
// 建议存 `projects/{pid}/suggestions/{sgId}.json`：
// ```json
// {"id","projectId","chapterId","chapterVersion","taskType":"continue|polish",
//  "action":"append|replace","instruction":"","content":"","status":"pending|accepted|rejected",
//  "sourceText":"","createdAt":"…"}
// ```
// 守卫存 `projects/{pid}/guards/{sgId}.json`（最新一条）：
// ```json
// {"suggestionId","chapterVersion","contentHash","status":"clear|warning|failed",
//  "issues":[{type,severity,title,description,candidateQuote,sourceRefs,suggestion}],
//  "contextRefs":{…},"failure":""}
// ```
// Routes:
// - `POST /api/v1/author/projects/{id}/chapters/{ch}/suggest` — 生成候选（LLM，不写正文）
// - `GET  /api/v1/author/projects/{id}/suggestions` — 列表
// - `GET  /api/v1/author/projects/{id}/suggestions/{sg}` — 读（含守卫）
// - `POST /api/v1/author/projects/{id}/suggestions/{sg}/guard` — 跑守卫（LLM）
// - `POST /api/v1/author/projects/{id}/suggestions/{sg}/accept` — 采纳（五道门）
// - `POST /api/v1/author/projects/{id}/suggestions/{sg}/reject` — 驳回
