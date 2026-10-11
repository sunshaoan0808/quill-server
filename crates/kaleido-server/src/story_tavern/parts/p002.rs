#![allow(unused_imports)]
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::Utc;
use futures_util::stream::Stream;
use kaleido_core::{
    ActorStateUpdate, Compass, CompassStore, ContentTier, CreateSessionRequest, DirectorPlan,
    DirectorPlanRunStatus, PackStore, StageDirectorConfig, StoryPack, TavernMessage,
    TavernPersona, TavernSession, TavernSessionStore, TurnPhase, build_side_branch_catalog,
    enter_side_branch, seed_opening_if_needed,
    MemoryL2Event,
};
use kaleido_core::ledger::{LedgerKind, LedgerStore};
use kaleido_core::st_skimming::{
    ReaderPlatform, ReaderProfile, ReaderSkimmingConfig, SkimIssue, analyze_skimming,
};
use kaleido_core::st_emotional_hooks::{
    EmotionalHookConfig, PlotSignalSample, render_hook_execution_contract,
    repeated_recent_hook_signal,
};
use kaleido_core::st_simulation::{
    ChapterSimulation, SYSTEM_PROMPT, render as render_simulation, validate_simulation,
};
use kaleido_core::st_outline::{ChapterBriefView, build_chapter_execution_contract, render_execution_contract};
use serde::Deserialize;
use serde_json::{json, Value};
use rand::Rng;
use std::collections::HashMap;
use std::convert::Infallible;
use std::hash::Hasher;
use std::time::Duration as StdDuration;
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;
use crate::{map_core_err, session_from, AppState, ChatStreamEvent};
use crate::error_codes::*;
use crate::llm_stream::{
    stream_chat_completions_dispatch, TurnStreamError,
};
use crate::skill_layer::{SkillDoc, append_writing_skill_hint, load_writing_skill};
use crate::state::StreamHub;
use super::*;

pub(crate) async fn write_chapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, rel)): Path<(String, String)>,
    Json(body): Json<ChapterBody>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.packs.write_chapter_body(&id, &rel, &body.content) {
        Ok(()) => Json(json!({ "ok": true, "path": rel })).into_response(),
        Err(e) => map_core_err(e),
    }
}

// ─── Session handlers ────────────────────────────────────────────────────────

pub(crate) async fn list_sessions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.list_owned(&session.user_id) {
        Ok(list) => Json(json!({ "sessions": list })).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn get_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(mut s) => {
            // Repair legacy messages that still embed 【选项】 in content
            let mut dirty = sanitize_session_messages(&mut s);
            // Legacy empty sessions (created before opening seed): auto-seed on first load.
            if !s.pack_missing && !s.opening_seeded && s.messages.is_empty() {
                if let Ok(pack) = state.packs.get(&s.pack_id) {
                    if seed_opening_if_needed(&mut s, &pack) {
                        dirty = true;
                    }
                }
            }
            // Do not write-repair a session on GET: the concurrent first-turn
            // worker may have acquired it between read and response, and this
            // legacy cleanup can otherwise overwrite its fresh revision.
            if dirty {
                tracing::debug!(session_id=%id, "skipping legacy GET repair write");
            }
            Json(s).into_response()
        }
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn create_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut req): Json<CreateSessionRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // F1: stamp owner from authenticated user.
    req.owner = Some(session.user_id.clone());
    match state
        .sessions_tavern
        .create_from_pack(&state.packs, req)
    {
        Ok(s) => Json(s).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn save_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(mut session): Json<TavernSession>,
) -> Response {
    let auth = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // F1: ownership check — load existing and verify owner before allowing save.
    // M-2 (CAS): use update_session so the read-then-write is a single locked
    // transaction; concurrent saves cannot overwrite each other.
    // We do the ownership + tier tightening + field reconciliation inside the
    // closure, then return the persisted session.
    if session.session_id.is_empty() {
        session.session_id = id.clone();
    } else if session.session_id != id {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "path id and body sessionId mismatch".into(),
        ));
    }
    let mut body_session = session;
    match state.sessions_tavern.update_session(&id, |existing| {
        // F1: ownership check inside the lock.
        if existing.owner.as_deref() != Some(&auth.user_id) {
            return Err(kaleido_core::CoreError::Forbidden(format!(
                "session not owned by user: {id}"
            )));
        }
        // Mid-session tier: only tighten (CONTEXT)
        if body_session.content_tier.rank() > existing.content_tier.rank() {
            body_session.content_tier = existing.content_tier;
        }
        body_session.checkpoints = existing.checkpoints.clone();
        // F1: preserve owner from existing session.
        body_session.owner = existing.owner.clone();
        // Replace in-place with the reconciled body.
        *existing = body_session.clone();
        Ok(())
    }) {
        Ok(s) => Json(s).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // F1: ownership check before delete.
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    match state.sessions_tavern.delete(&id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => map_core_err(e),
    }
}


#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetPlayModeRequest {
    play_mode: String,
}

/// Hot-switch session playMode mainline ↔ free ↔ side (ST-6/ST-9).
/// - free: freeze node advance; stay on current node
/// - side: freeze mainline cursor, stash resumeNodeId from current node before leaving mainline
/// - mainline from side: restore nodeId from resumeNodeId (if set)
pub(crate) async fn set_play_mode(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SetPlayModeRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mode = match kaleido_core::PlayMode::parse(&body.play_mode) {
        Some(m) => m,
        None => {
            return map_core_err(kaleido_core::CoreError::BadRequest(
                "playMode must be mainline, free, or side".into(),
            ));
        }
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if sess.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    if sess.active_run_id.is_some() {
        return conflict("ST_TURN_BUSY", "turn in progress; stop or wait before switching mode");
    }
    // M-2 (CAS): capture the on-disk revision at read time; the save below
    // uses save_with_revision so a concurrent write between read and write
    // surfaces as 409 instead of silently overwriting.
    let base_revision = sess.updated_at.clone();
    let prev = sess.play_mode;
    if prev == mode {
        return Json(sess).into_response();
    }

    // Entering side from mainline: stash resume point
    if mode == kaleido_core::PlayMode::Side && prev == kaleido_core::PlayMode::Mainline {
        if sess.resume_node_id.is_none() {
            sess.resume_node_id = sess.node_id.clone();
        }
    }
    // Entering side from free: also stash if empty
    if mode == kaleido_core::PlayMode::Side && prev == kaleido_core::PlayMode::Free {
        if sess.resume_node_id.is_none() {
            sess.resume_node_id = sess.node_id.clone();
        }
    }
    // Leaving side → mainline: restore resume node if present
    if mode == kaleido_core::PlayMode::Mainline && prev == kaleido_core::PlayMode::Side {
        if let Some(rid) = sess.resume_node_id.clone() {
            // resolve chapter from pack if possible
            if let Ok(pack) = state.packs.get(&sess.pack_id) {
                if let Some(n) = pack.nodes.iter().find(|n| n.id == rid) {
                    sess.node_id = Some(rid.clone());
                    sess.chapter_cursor = Some(n.chapter_id.clone());
                } else {
                    sess.node_id = Some(rid);
                }
            } else {
                sess.node_id = Some(rid);
            }
            // clear after successful resume
            sess.resume_node_id = None;
        }
        sess.side_branch_node_id = None;
        sess.side_branch_label = None;
    }

    sess.play_mode = mode;
    let detail = match mode {
        kaleido_core::PlayMode::Free => "自由模式：节点不再推进，可自由对话。".to_string(),
        kaleido_core::PlayMode::Side => format!(
            "支线模式：主线游标冻结；回主线将恢复到节点 {}。",
            sess.resume_node_id.as_deref().unwrap_or(sess.node_id.as_deref().unwrap_or("?"))
        ),
        kaleido_core::PlayMode::Mainline => {
            if prev == kaleido_core::PlayMode::Side {
                format!(
                    "主线模式：已从支线回档到节点 {}。",
                    sess.node_id.as_deref().unwrap_or("?")
                )
            } else {
                "主线模式：从当前节点继续推进。".to_string()
            }
        }
    };
    let note = format!(
        "〔模式切换〕{} → {}。{}",
        prev.as_str(),
        mode.as_str(),
        detail
    );
    sess.messages.push(TavernMessage {
        id: format!("msg-{}", Uuid::new_v4()),
        role: "assistant".into(),
        content: note,
        created_at: Utc::now().to_rfc3339(),
        options: vec![],
        engine_tag: None,
        program: None,
        reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
    });
    match state.sessions_tavern.save_with_revision(sess, &base_revision) {
        Ok(s) => Json(s).into_response(),
        Err(kaleido_core::CoreError::Conflict(_msg)) => return conflict("ST_CONCURRENT_WRITE", "session was modified concurrently; please retry"),
        Err(e) => map_core_err(e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetContentTierRequest {
    content_tier: String,
    adult_confirmed: Option<bool>,
}

/// Explicit mid-session content-tier switch (魔棒「内容档位」).
/// - Unlike save_session (which only ever tightens), this endpoint lets the
///   user loosen the tier, but loosening to Open requires adultConfirmed=true.
/// - final = min3(requested, card_max, global) still applies — a pack/card
///   capped at Safe cannot be opened past its card max.
pub(crate) async fn set_session_tier(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SetContentTierRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let req_tier = match ContentTier::parse(&body.content_tier) {
        Some(t) => t,
        None => {
            return map_core_err(kaleido_core::CoreError::BadRequest(
                "contentTier must be safe, standard, or open".into(),
            ));
        }
    };
    let probe = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if probe.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    if probe.active_run_id.is_some() {
        return conflict("ST_TURN_BUSY", "turn in progress; stop or wait before switching tier");
    }
    // Recompute card max like create_from_pack: min across pack characters, then pack.max_tier.
    let mut card_max = ContentTier::Open;
    if let Ok(pack) = state.packs.get(&probe.pack_id) {
        card_max = pack.max_tier;
        for c in &pack.characters {
            if let Some(t) = c.content_tier {
                if t.rank() < card_max.rank() {
                    card_max = t;
                }
            }
        }
    }
    let global = ContentTier::Open;
    let final_tier = ContentTier::min3(req_tier, card_max, global);

    let prev_tier = probe.content_tier;
    if prev_tier == final_tier {
        return Json(probe).into_response();
    }
    // Loosening to Open requires explicit adult confirmation.
    let adult_ok = body.adult_confirmed.unwrap_or(probe.adult_confirmed);
    if final_tier.rank() > prev_tier.rank() && final_tier == ContentTier::Open && !adult_ok {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "放宽到开放档需要成年确认（adultConfirmed=true）".into(),
        ));
    }
    // [fix 2026-08-15] 档位切换只更新状态，不写入消息流：此前把
    // 「〔内容档位〕open → standard。」作为 assistant 消息 push 进会话，
    // 模型会把这条系统噪音当剧情正文读，污染上下文（宿醉颠三倒四实踩）。
    // 前端选择器读 session.contentTier 即可反映状态。
    // M-2 (CAS): apply tier mutation inside a locked transaction.
    let user_id = session.user_id.clone();
    match state.sessions_tavern.update_session(&id, |sess| {
        if sess.owner.as_deref() != Some(&user_id) {
            return Err(kaleido_core::CoreError::Forbidden(format!(
                "session not owned by user: {id}"
            )));
        }
        if sess.pack_missing {
            return Err(kaleido_core::CoreError::BadRequest(
                "pack missing: session is read-only".into(),
            ));
        }
        if sess.active_run_id.is_some() {
            return Err(kaleido_core::CoreError::Conflict(
                "turn in progress; stop or wait before switching tier".into(),
            ));
        }
        // Mid-session tier can only tighten via this path when re-read inside
        // the lock differs from the probe (concurrent write). Re-clamp:
        if final_tier.rank() > sess.content_tier.rank() && final_tier == ContentTier::Open && !adult_ok {
            return Err(kaleido_core::CoreError::BadRequest(
                "放宽到开放档需要成年确认（adultConfirmed=true）".into(),
            ));
        }
        sess.content_tier = final_tier;
        sess.user_tier_request = req_tier;
        if body.adult_confirmed.is_some() {
            sess.adult_confirmed = adult_ok;
        }
        Ok(())
    }) {
        Ok(s) => Json(s).into_response(),
        Err(kaleido_core::CoreError::Conflict(msg)) => return conflict("ST_CONFLICT", msg),
        Err(e) => map_core_err(e),
    }
}

/// Ensure first-open / empty session has an opening monologue (idempotent).
pub(crate) async fn ensure_opening(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // Load pack before the atomic mutation; avoid a stale probe write racing the
    // first turn's acquire/save path.
    let probe = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if probe.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    let pack = match state.packs.get(&probe.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let pack_ref = &pack;
    let user_id = session.user_id.clone();
    match state.sessions_tavern.update_session(&id, |sess| {
        if sess.owner.as_deref() != Some(&user_id) {
            return Err(kaleido_core::CoreError::Forbidden(format!(
                "session not owned by user: {id}"
            )));
        }
        if sess.pack_missing {
            return Err(kaleido_core::CoreError::BadRequest(
                "pack missing: session is read-only".into(),
            ));
        }
        seed_opening_if_needed(sess, pack_ref);
        Ok(())
    }) {
        Ok(saved) => {
            Json(json!({ "seeded": saved.opening_seeded, "session": saved })).into_response()
        }
        Err(e) => map_core_err(e),
    }
}

/// List whole-novel summary + key nodes for side-branch picker.
pub(crate) async fn list_side_branches(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if sess.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    let pack = match state.packs.get(&sess.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let resume = if sess.play_mode == kaleido_core::PlayMode::Side {
        sess.resume_node_id.clone().or_else(|| sess.node_id.clone())
    } else {
        sess.node_id.clone()
    };
    let catalog = build_side_branch_catalog(&pack, resume);
    Json(catalog).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EnterSideBranchRequest {
    node_id: String,
}

/// Enter a side branch at a key node: switch to side mode + seed side opening.
pub(crate) async fn enter_side_branch_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<EnterSideBranchRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if sess.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    if sess.active_run_id.is_some() {
        return conflict("ST_TURN_BUSY", "turn in progress; stop or wait before entering side branch");
    }
    // M-2 (CAS): capture revision for optimistic-concurrency save below.
    let base_revision = sess.updated_at.clone();
    let pack = match state.packs.get(&sess.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let node_id = body.node_id.trim();
    if node_id.is_empty() {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "nodeId required".into(),
        ));
    }
    if let Err(e) = enter_side_branch(&mut sess, &pack, node_id) {
        return map_core_err(e);
    }
    match state.sessions_tavern.save_with_revision(sess, &base_revision) {
        Ok(s) => Json(s).into_response(),
        Err(kaleido_core::CoreError::Conflict(_msg)) => return conflict("ST_CONCURRENT_WRITE", "session was modified concurrently; please retry"),
        Err(e) => map_core_err(e),
    }
}





#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetFocusRequest {
    /// character id, or null/"auto" to keep rotation-only
    #[serde(default)]
    character_id: Option<String>,
    #[serde(default)]
    speaker_rotation: Option<bool>,
}

pub(crate) async fn set_focus_character(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<SetFocusRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if sess.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    if sess.active_run_id.is_some() {
        return conflict("ST_TURN_BUSY", "turn in progress");
    }
    // M-2 (CAS): capture revision for optimistic-concurrency save below.
    let base_revision = sess.updated_at.clone();
    if let Some(rot) = body.speaker_rotation {
        sess.speaker_rotation = rot;
    }
    if let Some(cid) = body.character_id {
        let cid = cid.trim().to_string();
        if cid.is_empty() || cid == "auto" {
            kaleido_core::ensure_focus_character(&mut sess);
        } else {
            if !sess.present_character_ids.is_empty() && !sess.present_character_ids.contains(&cid) {
                // allow if in pack
                if let Ok(pack) = state.packs.get(&sess.pack_id) {
                    if !pack.characters.iter().any(|c| c.id == cid) {
                        return map_core_err(kaleido_core::CoreError::BadRequest(
                            format!("character not present: {cid}"),
                        ));
                    }
                    if !sess.present_character_ids.contains(&cid) {
                        sess.present_character_ids.push(cid.clone());
                    }
                }
            }
            sess.focus_character_id = Some(cid);
        }
    } else {
        kaleido_core::ensure_focus_character(&mut sess);
    }
    match state.sessions_tavern.save_with_revision(sess, &base_revision) {
        Ok(s) => Json(s).into_response(),
        Err(kaleido_core::CoreError::Conflict(_msg)) => return conflict("ST_CONCURRENT_WRITE", "session was modified concurrently; please retry"),
        Err(e) => map_core_err(e),
    }
}


#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RebindVesselRequest {
    /// New vessel character id from pack (or empty to clear for isekai/extra)
    #[serde(default)]
    vessel_character_id: Option<String>,
    /// Optional entryRole override when rebinding (supporting/protagonist/extra/isekai)
    #[serde(default)]
    entry_role: Option<String>,
}

/// ST-11: rebind player vessel body mid-session.
/// Updates entry.vesselCharacterId + player.controlCharacterId; appends vessel_change note.
pub(crate) async fn rebind_vessel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RebindVesselRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if sess.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    if sess.active_run_id.is_some() {
        return conflict("ST_TURN_BUSY", "turn in progress; stop or wait before rebind");
    }

    let pack = match state.packs.get(&sess.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    // M-2 (CAS): capture revision for optimistic-concurrency save below.
    let base_revision = sess.updated_at.clone();

    let old_vessel = sess
        .entry
        .vessel_character_id
        .clone()
        .or_else(|| sess.player.control_character_id.clone());

    let new_vessel = body
        .vessel_character_id
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(ref vid) = new_vessel {
        if !pack.characters.iter().any(|c| c.id == *vid) {
            return map_core_err(kaleido_core::CoreError::BadRequest(format!(
                "vessel not in pack: {vid}"
            )));
        }
    }

    // optional entry role
    if let Some(role_s) = body.entry_role.as_ref().map(|s| s.trim().to_ascii_lowercase()) {
        let role = match role_s.as_str() {
            "supporting" => Some(kaleido_core::EntryRole::Supporting),
            "protagonist" => Some(kaleido_core::EntryRole::Protagonist),
            "extra" => Some(kaleido_core::EntryRole::Extra),
            "isekai" => Some(kaleido_core::EntryRole::Isekai),
            "" => None,
            _ => {
                return map_core_err(kaleido_core::CoreError::BadRequest(
                    "entryRole must be supporting|protagonist|extra|isekai".into(),
                ));
            }
        };
        if let Some(r) = role {
            sess.entry.entry_role = Some(r);
        }
    }

    let role = sess.entry.entry_role;
    // A9: isekai should not require vessel
    let is_isekai = matches!(role, Some(kaleido_core::EntryRole::Isekai));
    let is_extra = matches!(role, Some(kaleido_core::EntryRole::Extra));

    if (matches!(role, Some(kaleido_core::EntryRole::Protagonist) | Some(kaleido_core::EntryRole::Supporting))
        || role.is_none())
        && new_vessel.is_none()
        && !is_isekai
        && !is_extra
    {
        // allow clear only for isekai/extra; for protagonist/supporting require vessel
        if matches!(role, Some(kaleido_core::EntryRole::Protagonist) | Some(kaleido_core::EntryRole::Supporting)) {
            return map_core_err(kaleido_core::CoreError::BadRequest(
                "protagonist/supporting rebind requires vesselCharacterId".into(),
            ));
        }
    }

    let control_id = if is_isekai {
        Some("player_isekai".to_string())
    } else if is_extra && new_vessel.is_none() {
        Some("player_extra".to_string())
    } else {
        new_vessel.clone()
    };

    sess.entry.vessel_character_id = new_vessel.clone();
    sess.player.control_character_id = control_id.clone();

    // ensure vessel stays present in scene cast
    if let Some(ref vid) = new_vessel {
        if !sess.present_character_ids.contains(vid) {
            sess.present_character_ids.push(vid.clone());
        }
    }

    // if focus was old vessel body, move focus to another present NPC
    if let Some(ref foc) = sess.focus_character_id.clone() {
        if old_vessel.as_ref() == Some(foc) || control_id.as_ref() == Some(foc) {
            let next = sess
                .present_character_ids
                .iter()
                .find(|id| control_id.as_ref() != Some(id))
                .cloned()
                .or_else(|| sess.present_character_ids.first().cloned());
            sess.focus_character_id = next;
        }
    }
    kaleido_core::ensure_focus_character(&mut sess);

    let old_name = old_vessel
        .as_ref()
        .and_then(|id| pack.characters.iter().find(|c| c.id == *id).map(|c| c.name.clone()))
        .unwrap_or_else(|| old_vessel.clone().unwrap_or_else(|| "无".into()));
    let new_name = match &control_id {
        Some(id) if id == "player_isekai" => "异世界身份".into(),
        Some(id) if id == "player_extra" => "路人身份".into(),
        Some(id) => pack
            .characters
            .iter()
            .find(|c| c.id == *id)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| id.clone()),
        None => "未绑定".into(),
    };

    let note = format!(
        "〔换壳 vessel_change〕玩家身子：{} → {}。关系记忆保留，身份锚点已更新。",
        old_name, new_name
    );
    sess.messages.push(TavernMessage {
        id: format!("msg-{}", Uuid::new_v4()),
        role: "assistant".into(),
        content: note,
        created_at: Utc::now().to_rfc3339(),
        options: vec![],
        engine_tag: None,
        program: None,
        reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
    });

    match state.sessions_tavern.save_with_revision(sess, &base_revision) {
        Ok(s) => Json(s).into_response(),
        Err(kaleido_core::CoreError::Conflict(_msg)) => return conflict("ST_CONCURRENT_WRITE", "session was modified concurrently; please retry"),
        Err(e) => map_core_err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateSaveRequest {
    #[serde(default)]
    pub(crate) label: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RewindRequest {
    /// 回退回合数（N>=1，缺省 1）
    #[serde(default)]
    pub(crate) steps: Option<usize>,
}

pub(crate) async fn list_saves(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // F1: ownership check.
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    match state.sessions_tavern.list_saves(&id) {
        Ok(list) => Json(json!({ "saves": list })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// 世界线视图（/line 全景）：按分叉关系组织存档线，标注当前所在世界线。
pub(crate) async fn get_worldline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.sessions_tavern.worldline(&id) {
        Ok(view) => Json(view).into_response(),
        Err(e) => map_core_err(e),
    }
}

// ─── T2 世界认知 handlers（U2 实体图谱 / U7 真相账本）────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorldEntitiesQuery {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    q: Option<String>,
}

pub(crate) async fn world_entities(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<WorldEntitiesQuery>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state
        .sessions_tavern
        .world_entities(&id, query.kind, query.q)
    {
        Ok(view) => Json(view).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn world_entity_detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, entity_id)): Path<(String, String)>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state
        .sessions_tavern
        .world_entity_detail(&id, &entity_id)
    {
        Ok(view) => Json(view).into_response(),
        Err(e) => map_core_err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorldEventsRequest {
    events: Vec<kaleido_core::world_state::WorldEvent>,
}

pub(crate) async fn world_apply_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<WorldEventsRequest>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.sessions_tavern.world_apply_events(&id, req.events) {
        Ok(view) => Json(view).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn world_truth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.sessions_tavern.world_truth(&id) {
        Ok(view) => Json(view).into_response(),
        Err(e) => map_core_err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorldTruthCheckRequest {
    entity_id: String,
    key: String,
    expected: Value,
}

pub(crate) async fn world_truth_check(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<WorldTruthCheckRequest>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state
        .sessions_tavern
        .world_truth_check(&id, req.entity_id, req.key, req.expected)
    {
        Ok(view) => Json(view).into_response(),
        Err(e) => map_core_err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssistantRequest {
    message: String,
    /// 可选：前端本地保存的助手对话历史（[{role, content}]，不含本次 message）。
    /// 有则作为多轮上下文传给 LLM，解决助手"失忆"问题。
    #[serde(default)]
    history: Vec<AssistHistoryMsg>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AssistHistoryMsg {
    #[serde(default)]
    pub(crate) role: String,
    #[serde(default)]
    pub(crate) content: String,
}

/// 剧情助手系统提示（tavern 剧场与 story/冒险/跑团共用）。
pub(crate) const ASSISTANT_SYSTEM_PROMPT: &str = "你是这场角色扮演的「助手」，职责是保障和优化这场扮演：诊断回复质量（变短/复读/换语言）、\
    调配置与参数、检查记忆账本（场景摘要/事件/好感/关系）、解释当前剧情状态。\
    你有全局视角：能读剧情记录、记忆账本与剧本。但你绝不代写剧情——剧情正文永远只出自剧情模型。\
    回答用中文，简洁直接，不用角色扮演腔。\
    若玩家要求生成可视化面板（地图/线索图谱/线索板/装备栏等），输出格式：【面板】{\"name\":\"面板名\",\"kind\":\"markdown|svg|html\",\"content\":\"...\"}【/面板】，正文不要重复面板内容。";

/// P0-2: /config 白名单 keys。
pub(crate) const CONFIG_KEYS: &[&str] = &[
    "strict_mode_boost",
    "pacing",
    "style_guidance",
    // [fix 2026-08-15 文风外挂] /config style_source=<pack标题> 外挂指定作品文风
    "style_source",
    // [fix 2026-08-15 叙述视角] /config pov=first|third —— 覆盖蒸馏文风的人称设定
    "pov",
];

/// 解析 `/config key=value ...`：白名单 + 值校验。任一非法 token 返回 None。
pub(crate) fn parse_config_args(rest: &str) -> Option<Vec<(String, Value)>> {
    let mut out = Vec::new();
    for tok in rest.split_whitespace() {
        let (k, v) = tok.split_once('=')?;
        let k = k.trim();
        let v = v.trim();
        if !CONFIG_KEYS.contains(&k) {
            return None;
        }
        let val = match k {
            "strict_mode_boost" | "pacing" => {
                let f: f64 = v.parse().ok()?;
                if !(0.0..=1.0).contains(&f) {
                    return None;
                }
                json!(f)
            }
            // [fix 2026-08-15 文风外挂] style_guidance 支持长文风指引（≤2000 字）：
            // 用户可将 style-analysis 生成的 stylePrompt 整段直挂（18+ 描写风格外挂）。
            "style_guidance" => {
                if v.is_empty() || v.chars().count() > 2000 {
                    return None;
                }
                json!(v)
            }
            // [fix 2026-08-15 文风外挂] style_source=<pack标题>：值允许含中文 pack 名，≤40 字
            "style_source" => {
                if v.is_empty() || v.chars().count() > 40 {
                    return None;
                }
                json!(v)
            }
            // [fix 2026-08-15 叙述视角] pov=first|third —— 覆盖蒸馏文风的人称设定；
            // pov=default 表示清除覆盖、回退蒸馏文风的人称。
            "pov" => {
                if v != "first" && v != "third" && v != "default" {
                    return None;
                }
                json!(v)
            }
            _ => return None,
        };
        out.push((k.to_string(), val));
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 将校验后的配置写入 sess.player.flags（保留已有 keys）。
pub(crate) fn apply_config_flags(sess: &mut TavernSession, pairs: &[(String, Value)]) {
    if !sess.player.flags.is_object() {
        sess.player.flags = json!({});
    }
    if let Some(map) = sess.player.flags.as_object_mut() {
        for (k, v) in pairs {
            map.insert(k.clone(), v.clone());
        }
    }
}

/// [fix 2026-08-15 文风外挂] 按 pack 标题（支持模糊包含匹配）读取指定作品的
/// narrative_style，合成为 style_guidance 注入文本。找不到 pack 或无文风 → Err。
/// 用户场景：宿醉 18+ 清淡 → /config style_source=智取美母 外挂浓烈 18+ 描写风格。
pub(crate) fn resolve_external_style(state: &AppState, pack_query: &str) -> Result<String, String> {
    let packs = state.packs.list().map_err(|e| format!("pack 列表读取失败：{e}"))?;
    let q = pack_query.trim();
    if q.is_empty() {
        return Err("pack 标题不能为空".into());
    }
    // 精确匹配优先，其次包含匹配
    let found = packs
        .iter()
        .find(|p| p.title == q)
        .or_else(|| packs.iter().find(|p| p.title.contains(q) || q.contains(&p.title)));
    let pid = match found {
        Some(p) => p.id.clone(),
        None => {
            let names: Vec<&str> = packs.iter().map(|p| p.title.as_str()).collect();
            return Err(format!("未找到 pack「{q}」，现有：{}", names.join(" / ")));
        }
    };
    let pack = state
        .packs
        .get(&pid)
        .map_err(|e| format!("pack 读取失败：{e}"))?;
    // [fix 2026-08-15 落库优先] 若 pack 已有 style-profiles/<pid>.txt（style-analysis
    // 落库的完整 stylePrompt），优先使用——这是特化文风（如 18+ 浓烈描写），
    // 比蒸馏 narrative_style 更精细。无落库则回退蒸馏文风。
    let profile_path = state
        .app_state
        .data_root()
        .story_packs_dir()
        .join(&pid)
        .join("style-profiles")
        .join(format!("{pid}.txt"));
    if let Ok(profile_text) = std::fs::read_to_string(&profile_path) {
        let pt = profile_text.trim();
        if !pt.is_empty() {
            return Ok(format!(
                "（外挂作品《{}》文风·特化版）{}",
                pack.title, pt
            ));
        }
    }
    let ns = pack
        .stage_director
        .resolved_snapshot
        .as_ref()
        .and_then(|snap| snap.narrative_style.as_ref())
        .ok_or_else(|| format!("pack「{}」尚无蒸馏出的文风（先跑 distil-world）", pack.title))?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(s) = ns.get("style").and_then(|v| v.as_str()) {
        if !s.trim().is_empty() {
            parts.push(format!("叙事视角/人称：{}", s.trim()));
        }
    }
    if let Some(t) = ns.get("tone").and_then(|v| v.as_str()) {
        if !t.trim().is_empty() {
            parts.push(format!("基调：{}", t.trim()));
        }
    }
    if let Some(p) = ns.get("prose_guidance").and_then(|v| v.as_str()) {
        if !p.trim().is_empty() {
            parts.push(format!("行文要求：{}", p.trim()));
        }
    }
    if parts.is_empty() {
        return Err(format!("pack「{}」文风字段为空", pack.title));
    }
    Ok(format!(
        "（外挂作品《{}》文风）{}",
        pack.title,
        parts.join("；")
    ))
}

/// 剧情助手（吸收自梨园 assistant-gateway 双agent分治的最小版）：
/// 独立 LLM 会话，带全局视角（剧情记录+记忆账本+剧本），绝不代写剧情。
pub(crate) async fn assistant_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AssistantRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let msg = body.message.trim().to_string();
    if msg.is_empty() {
        return bad_request("ST_EMPTY_MESSAGE", "empty message");
    }
    // P0-1: story_command —— /reroll 重生成上一回合、/rewind N 回退（LLM 调用之前识别）
    if msg.starts_with("/reroll") {
        return match do_reroll(&state, &id).await {
            Ok((turn, last_user)) => Json(json!({
                "reply": "已重新生成上一回合（即将重发）",
                "action": "reroll",
                "lastUserMessage": last_user,
                "turn": turn,
            }))
            .into_response(),
            Err(kaleido_core::CoreError::BadRequest(e)) => {
                Json(json!({ "reply": format!("重生成失败：{e}"), "action": "error" })).into_response()
            }
            Err(e) => map_core_err(e),
        };
    }
    if let Some(steps) = parse_rewind_steps(&msg) {
        return match do_rewind(&state, &id, steps).await {
            Ok((turn, rewound)) => Json(json!({
                "reply": format!("已回退 {steps} 回合"),
                "action": "rewind",
                "rewound": rewound,
                "turn": turn,
            }))
            .into_response(),
            Err(kaleido_core::CoreError::BadRequest(e)) => {
                Json(json!({ "reply": format!("回退失败：{e}"), "action": "error" })).into_response()
            }
            Err(e) => map_core_err(e),
        };
    }
    // P0-2/P0-3: 剧情助手命令 —— /config key=value /remember /event（LLM 调用之前识别，整行匹配）
    if msg.starts_with("/config") {
        return handle_config_cmd(&state, &id, &msg).await;
    }
    if msg.starts_with("/remember") {
        return handle_remember_cmd(&state, &id, &msg).await;
    }
    if msg.starts_with("/event") {
        return handle_event_cmd(&state, &id, &msg).await;
    }
    if msg.starts_with("/time") {
        return handle_time_cmd(&state, &id, &msg).await;
    }
    if msg.starts_with("/weather") {
        return handle_weather_cmd(&state, &id, &msg).await;
    }
    if msg.starts_with("/season") {
        return handle_season_cmd(&state, &id, &msg).await;
    }
    let mut sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    let pack = state.packs.get(&sess.pack_id).ok();
    let pack_title = pack.as_ref().map(|p| p.title.clone()).unwrap_or_default();

    let sys = ASSISTANT_SYSTEM_PROMPT;

    let mut ctx = String::new();
    ctx.push_str(&format!(
        "剧本：{}\n当前回合：{}\n当前节点：{}\n\n",
        pack_title,
        sess.turn,
        sess.node_id.as_deref().unwrap_or("?")
    ));
    // 全本视野：章节目录（骨架）+ 全书向量检索（懒建索引，失败静默降级）+ 世界线（分支路径）
    if let Some(p) = &pack {
        let toc = pack_toc_text(p);
        if !toc.is_empty() {
            ctx.push_str(&toc);
        }
        let vctx = pack_vector_ctx(&state, p, &msg);
        if !vctx.is_empty() {
            ctx.push_str(&vctx);
        }
    }
    if let Ok(wv) = state.sessions_tavern.worldline(&id) {
        let wt = format_worldline_text(&wv);
        if !wt.is_empty() {
            ctx.push_str(&wt);
        }
    }
    if !sess.memory_l1.scene_summary.is_empty() {
        ctx.push_str(&format!("【场景摘要】\n{}\n\n", sess.memory_l1.scene_summary));
    }
    if !sess.memory_l2.events.is_empty() {
        ctx.push_str("【事件账本】\n");
        for ev in sess.memory_l2.events.iter().rev().take(8) {
            ctx.push_str(&format!("• {}（{}）: {}\n", ev.id, ev.kind, ev.summary));
        }
        ctx.push('\n');
    }
    // 关系状态（与 build_tavern_system_prompt 同口径：从 L2 统计亲密事件数）
    // ST-FIX: 弱信号 contains("亲")/("床") 会把「母亲」的「亲」字误计为接吻（宿醉剧本
    // 每条事件摘要都含「母亲」），导致凭空生成「接过吻至少 15 次」的关系状态注入 prompt。
    // 改用强信号：只有明确接吻词（吻/亲嘴/亲吻/亲热/接吻）或 kind==romance 才计数。
    let kiss_count = sess.memory_l2.events.iter().filter(|e| {
        e.kind == "romance" ||
        e.summary.to_lowercase().contains("接吻") ||
        e.summary.to_lowercase().contains("亲嘴") ||
        e.summary.to_lowercase().contains("亲吻") ||
        e.summary.to_lowercase().contains("亲热") ||
        e.summary.to_lowercase().contains("吻")
    }).count();
    if kiss_count > 0 {
        ctx.push_str(&format!("【关系状态】你们已发生亲密互动（至少 {kiss_count} 次），关系已确立/升级。\n\n"));
    }
    ctx.push_str("【最近剧情】\n");
    for m in sess.messages.iter().rev().take(10).rev() {
        let role = if m.role == "user" { "你" } else { "旁白" };
        // 前后各取 150 字符（消息长时保留尾部信息，如关系升级往往在结尾）
        let content: String = m
            .content
            .chars()
            .enumerate()
            .filter(|(i, _)| *i < 150 || (m.content.chars().count() as i64 - *i as i64) <= 150)
            .map(|(_, c)| c)
            .collect();
        ctx.push_str(&format!("[{}] {}\n", role, content));
    }
    let user = format!("玩家对你说：{}\n\n{}", msg, ctx);

    let llm = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    let prov_kind = crate::llm_stream::runtime_provider_kind(&llm, &state.provider_kind);
    // 多轮助手历史：system + 前端 history（若有）+ 本次 user（带剧情上下文）。
    // 解决助手"失忆"——历史使连续咨询可承接上文。
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    msgs.push(json!({"role": "system", "content": sys}));
    for h in body.history.iter().rev().take(12).rev() {
        let role = if h.role == "user" { "user" } else { "assistant" };
        let content = h.content.trim();
        if content.is_empty() {
            continue;
        }
        msgs.push(json!({"role": role, "content": content}));
    }
    msgs.push(json!({"role": "user", "content": user}));
    match crate::llm_stream::stream_chat_completions_msgs_dispatch(&llm.base_url, &llm.api_key, &llm.model, &prov_kind, msgs, 0.1, 8192, 90, |_| true).await {
        Ok(reply) => {
            let reply = reply.trim().to_string();
            // 面板（吸收自梨园 panels.ts）：提取【面板】块并回写 session.panels
            let (clean_reply, panels) = split_panels_from_narrative(&reply);
            if !panels.is_empty() {
                for p in &panels {
                    if let Some(existing) = sess.panels.iter_mut().find(|x| x.name == p.name) {
                        *existing = p.clone();
                    } else {
                        sess.panels.push(p.clone());
                    }
                }
                if let Err(e) = state.sessions_tavern.save(sess.clone()) {
                    tracing::warn!(error = %e, "assistant failed to save panels");
                }
            }
            // 正文只回干净文本；若只剩面板则给一句汇总
            let reply = if clean_reply.trim().is_empty() && !panels.is_empty() {
                format!(
                    "已生成可视化面板：{}",
                    panels.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join("、")
                )
            } else {
                clean_reply
            };
            Json(json!({ "reply": reply })).into_response()
        }
        Err(e) => internal("ST_ASSISTANT_LLM_FAILED", e),
    }
}
