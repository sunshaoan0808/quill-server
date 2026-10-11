//! suggest/advance —— 推进模式状态机（AdvanceState / permit）。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn advance_path(pid: &str) -> String {
    format!("projects/{pid}/advance.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdvanceState {
    #[serde(default = "auto_mode")]
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) permit_chapter: String,
}

pub(crate) fn auto_mode() -> String {
    "auto".into()
}

pub(crate) fn read_advance(state: &AppState, ws: &str, pid: &str) -> AdvanceState {
    state
        .works
        .read_text(ws, &advance_path(pid))
        .ok()
        .and_then(|b| serde_json::from_str::<AdvanceState>(&b.content).ok())
        .unwrap_or(AdvanceState { mode: "auto".into(), permit_chapter: String::new() })
}

pub(crate) async fn get_advance(
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
    let adv = read_advance(&state, &session.workspace_id, &pid);
    Json(json!({"ok": true, "mode": adv.mode, "permitChapter": adv.permit_chapter})).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetAdvanceBody {
    #[serde(default = "auto_mode")]
    pub(crate) mode: String,
}

pub(crate) async fn set_advance(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
    Json(body): Json<SetAdvanceBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pid = match safe_id(&pid) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if body.mode != "auto" && body.mode != "review" {
        return bad_request("ADVANCE_BAD_MODE", "mode must be auto|review");
    }
    let adv = AdvanceState { mode: body.mode, permit_chapter: String::new() };
    let s = serde_json::to_string_pretty(&adv).unwrap_or_default();
    let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}"));
    if let Err(e) = state.works.write_text(&session.workspace_id, &advance_path(&pid), &s) {
        return map_core_err(e);
    }
    Json(json!({"ok": true, "mode": adv.mode})).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PermitBody {
    #[serde(default)]
    pub(crate) chapter_id: String,
}

pub(crate) async fn grant_permit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
    Json(body): Json<PermitBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pid = match safe_id(&pid) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let ch_id = match safe_id(&body.chapter_id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if read_ch(&state, &session.workspace_id, &pid, &ch_id).is_err() {
        return bad_request("PERMIT_BAD_CHAPTER", "chapter not found");
    }
    let adv = AdvanceState { mode: "review".into(), permit_chapter: ch_id.clone() };
    let s = serde_json::to_string_pretty(&adv).unwrap_or_default();
    let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}"));
    if let Err(e) = state.works.write_text(&session.workspace_id, &advance_path(&pid), &s) {
        return map_core_err(e);
    }
    Json(json!({"ok": true, "permitChapter": ch_id})).into_response()
}
