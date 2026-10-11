//! suggest/views —— get_arcs / get_ledger / get_suggest 只读端点。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn get_arcs(
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
    let dir = format!("projects/{pid}/chapters");
    let mut all: Vec<ChapterDoc> = Vec::new();
    if let Ok(entry) = state.works.list(&session.workspace_id, &dir, 1) {
        for child in entry.children {
            if child.kind == "dir" {
                continue;
            }
            let rel = format!("{dir}/{}", child.name);
            if let Ok(body) = state.works.read_text(&session.workspace_id, &rel) {
                if let Ok(ch) = serde_json::from_str::<ChapterDoc>(&body.content) {
                    all.push(ch);
                }
            }
        }
    }
    // 按 (volume, arc, id) 分组
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<(u32, u32), Vec<Value>> = BTreeMap::new();
    for ch in &all {
        groups.entry((ch.volume, ch.arc)).or_default().push(
            json!({"chapterId": ch.id, "title": ch.title, "versionNo": ch.version_no}),
        );
    }
    let arcs: Vec<Value> = groups
        .into_iter()
        .map(|((v, a), chapters)| json!({"volume": v, "arc": a, "count": chapters.len(), "chapters": chapters}))
        .collect();
    Json(json!({"ok": true, "count": all.len(), "arcs": arcs})).into_response()
}

/// Token 账本查询（总额+分环节）。
pub(crate) async fn get_ledger(
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
    let v = read_ledger(&state, &session.workspace_id, &pid);
    Json(json!({"ok": true, "ledger": v})).into_response()
}

pub(crate) async fn get_suggest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pid, sg)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (pid, sg) = match (safe_id(&pid), safe_id(&sg)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return bad_request("SUGGEST_BAD_ID", "invalid id"),
    };
    match read_sg(&state, &session.workspace_id, &pid, &sg) {
        Ok(doc) => {
            let guard = read_guard(&state, &session.workspace_id, &pid, &sg);
            Json(json!({"ok": true, "suggestion": doc, "guard": guard})).into_response()
        }
        Err(r) => r,
    }
}
