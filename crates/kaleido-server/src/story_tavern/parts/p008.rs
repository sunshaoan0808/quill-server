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

/// [吞噬 Front Porch AI pockets.dart] 口袋与衣物（per-character, per-session）。
pub(crate) async fn get_pockets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => Json(json!({ "pockets": s.pockets })).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn put_pockets(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let mut sess = match state.sessions_tavern.get(&id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    let map = match &body {
        serde_json::Value::Object(m) => m,
        _ => return bad_request("ST_POCKETS_BAD_BODY", "expected object {characterId: Pockets}"),
    };
    let mut updated = 0usize;
    let is_ops_form = map.contains_key("characterId") && map.contains_key("ops");
    for (cid, v) in map {
        if cid.is_empty() { continue; }
        // allow pocketsEnabled alongside pocket entries: {"cc-xxx": {...}, "pocketsEnabled": true}
        if cid == "pocketsEnabled" || cid == "enabled" {
            if let Some(b) = v.as_bool() { sess.pockets_enabled = b; }
            continue;
        }
        // ops 形态保留键不进 pockets map
        if is_ops_form && (cid == "characterId" || cid == "ops") { continue; }
        if !v.is_object() { continue; }
        let p = kaleido_core::pockets::Pockets::from_json(v);
        // normalize via from_json (caps, tidy)
        sess.pockets.insert(cid.clone(), p);
        updated += 1;
    }
    // [物品记忆卡] ops 形态 {characterId, ops:[...]} → apply + 确定性写 Journal item 卡
    if let (Some(cid), Some(ops_v)) = (body.get("characterId").and_then(|v| v.as_str()), body.get("ops").and_then(|v| v.as_array())) {
        let ops: Vec<kaleido_core::pockets::PocketOpReport> = ops_v.iter().filter_map(kaleido_core::pockets::PocketOpReport::from_json).collect();
        if !ops.is_empty() && !cid.is_empty() {
            let day = sess.game_clock.day;
            let entry = sess.pockets.entry(cid.to_string()).or_default();
            let mut events: Vec<kaleido_core::pockets::PocketEvent> = vec![];
            kaleido_core::pockets::apply_pocket_ops(entry, &ops, None, day, Some(&mut events));
            let drafts = kaleido_core::pockets::item_cards_from(&events);
            let sid = sess.session_id.clone();
            for (item, content) in drafts {
                let mut card = kaleido_core::journal_store::JournalCard::new(sid.clone(), cid.to_string(), content, sess.turn);
                card.kind = Some("item".into());
                card.metadata_item = Some(item);
                card.category = "moment".into();
                sess.journal.add_card(card, 50);
            }
            updated += 1;
        }
    }
    match state.sessions_tavern.save(sess) {
        Ok(_) => Json(json!({ "ok": true, "updated": updated })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// [P1-B Porch Life À la carte] 口袋开关（默认开，Own switch）。
pub(crate) async fn get_pockets_enabled(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => Json(json!({ "pocketsEnabled": s.pockets_enabled })).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn put_pockets_enabled(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let mut sess = match state.sessions_tavern.get(&id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    let enabled = body.get("pocketsEnabled").and_then(|v| v.as_bool())
        .or_else(|| body.get("enabled").and_then(|v| v.as_bool()))
        .unwrap_or(true);
    sess.pockets_enabled = enabled;
    match state.sessions_tavern.save(sess) {
        Ok(_) => Json(json!({ "ok": true, "pocketsEnabled": enabled })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// [P2+P3 吞噬 Front Porch AI needs/growth/climate] Needs/Growth/Climate API.
pub(crate) async fn get_needs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) {
        Ok(sess) => Json(json!({ "needs": sess.needs })).into_response(),
        Err(e) => map_core_err(e),
    }
}
pub(crate) async fn put_needs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let map = match &body { serde_json::Value::Object(m)=>m, _=>return bad_request("ST_NEEDS_BAD_BODY","expected object {characterId: Needs}")};
    let mut updated=0usize;
    for (cid, v) in map {
        if cid.is_empty(){continue;}
        let n = kaleido_core::needs::Needs::from_json(v);
        sess.needs.insert(cid.clone(), n);
        updated+=1;
    }
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"updated":updated})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn tick_needs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let rough = sess.world_climate.atmosphere != kaleido_core::world_climate::WorldAtmosphere::Breathable;
    let clear = sess.game_clock.weather == "晴";
    for needs in sess.needs.values_mut() { needs.tick_decay(rough, clear); }
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn get_growth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) {
        Ok(sess) => Json(json!({ "growth": sess.growth })).into_response(),
        Err(e) => map_core_err(e),
    }
}
pub(crate) async fn put_growth(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    // body: {characterId, event, strength} or {rings: [GrowthRing]}
    if let Some(arr) = body.get("rings").and_then(|v| v.as_array()) {
        let rings: Vec<kaleido_core::character_arc::GrowthRing> = arr.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
        sess.growth.rings = rings;
    } else {
        let cid = body.get("characterId").or_else(|| body.get("character")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let ev = body.get("event").or_else(|| body.get("triggerEvent")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let strength = body.get("strength").and_then(|v| v.as_f64()).unwrap_or(0.6) as f32;
        if cid.is_empty() || ev.is_empty() { return bad_request("ST_GROWTH_BAD_BODY","need characterId + event") }
        sess.growth.strengthen(&cid, &ev, strength, sess.turn);
    }
    let g = sess.growth.clone();
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"growth":g})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn get_world_climate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) {
        Ok(sess) => Json(json!({ "worldClimate": sess.world_climate })).into_response(),
        Err(e) => map_core_err(e),
    }
}
pub(crate) async fn put_world_climate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    sess.world_climate = kaleido_core::world_climate::WorldClimate::from_json(&body);
    let wc = sess.world_climate.clone();
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"worldClimate": wc})).into_response(), Err(e)=>map_core_err(e)}
}

/// [P4 吞噬 Front Porch AI chaos/tiers/objectives/dreams] P4 API handlers.
pub(crate) async fn get_chaos(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"chaos": sess.chaos})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn put_chaos(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    if let Some(b) = body.get("enabled").and_then(|v| v.as_bool()) { sess.chaos.enabled = b; }
    if let Some(b) = body.get("nsfw").and_then(|v| v.as_bool()) { sess.chaos.nsfw = b; }
    match state.sessions_tavern.save(sess.clone()) { Ok(_)=>Json(json!({"ok":true,"chaos": sess.chaos})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn tick_chaos(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    sess.chaos.tick();
    let should = sess.chaos.should_trigger(rand::random::<u32>() % 100);
    if should && !sess.chaos.has_pending() {
        let char_name = sess.present_character_ids.first().and_then(|cid| sess.world.entities.values().find(|e| e.id==*cid).map(|e| e.name.clone())).unwrap_or_else(|| "角色".into());
        let idx = rand::random::<usize>() % 20;
        sess.chaos.arm_event(&char_name, idx);
    }
    let c = sess.chaos.clone();
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"chaos": c, "triggered": should})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn get_milestones(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"milestones": sess.milestones})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn get_objectives(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"objectives": sess.objectives})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn create_objective(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let title = body.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if title.is_empty() { return bad_request("ST_OBJ_BAD","need title"); }
    let owner = body.get("owner").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let tasks: Vec<String> = body.get("tasks").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let obj = kaleido_core::objectives::Objective::new(owner, title, tasks, sess.turn);
    let ret = obj.clone();
    sess.objectives.push(obj);
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"objective": ret})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn update_objective(State(state): State<AppState>, headers: HeaderMap, Path((id, oid)): Path<(String,String)>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let Some(obj) = sess.objectives.iter_mut().find(|o| o.id==oid) else { return not_found("ST_OBJ_NOT_FOUND","objective not found") };
    if let Some(tid)=body.get("taskId").and_then(|v| v.as_str()) { if let Some(c)=body.get("completed").and_then(|v| v.as_bool()) { obj.mark_task(tid,c); obj.auto_complete_if_all_done(); } }
    if let Some(s)=body.get("status").and_then(|v| v.as_str()) { obj.status=s.to_string(); }
    let ret = obj.clone();
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"objective": ret})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn delete_objective(State(state): State<AppState>, headers: HeaderMap, Path((id, oid)): Path<(String,String)>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let before=sess.objectives.len(); sess.objectives.retain(|o| o.id!=oid);
    if sess.objectives.len()==before { return not_found("ST_OBJ_NOT_FOUND","not found") }
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn get_ambitions(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"ambitions": sess.ambitions})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn create_ambition(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let character = body.get("character").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if text.is_empty() { return bad_request("ST_AMB_BAD","need text") }
    let amb = kaleido_core::objectives::Ambition::new(character, text, sess.turn);
    let ret=amb.clone(); sess.ambitions.push(amb);
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"ambition": ret})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn get_dreams(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"dream": sess.dream, "episodes": sess.episodes})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn push_dream(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    if let Some(t)=body.get("dream").and_then(|v| v.as_str()) { sess.dream.last_dream = Some(t.to_string()); sess.dream.pending = false; }
    if let Some(b)=body.get("pending").and_then(|v| v.as_bool()) { sess.dream.pending = b; }
    match state.sessions_tavern.save(sess.clone()) { Ok(_)=>Json(json!({"ok":true,"dream": sess.dream})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn get_episodes(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"episodes": sess.episodes})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn push_episode(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let kind = body.get("kind").and_then(|v| v.as_str()).unwrap_or("episode").to_string();
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if content.is_empty() { return bad_request("ST_EP_BAD","need content") }
    sess.episodes.push(kind, content, sess.turn);
    match state.sessions_tavern.save(sess.clone()) { Ok(_)=>Json(json!({"ok":true,"episodes": sess.episodes})).into_response(), Err(e)=>map_core_err(e)}
}

/// [Journal 存量 吞噬 Front Porch AI journal_store] Journal card CRUD.
pub(crate) async fn get_journals(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"journals": sess.journal.cards})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn create_journal_card(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let content = body.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if content.is_empty() { return bad_request("ST_JOURNAL_BAD","need content") }
    let character_id = body.get("characterId").or_else(|| body.get("character_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if character_id.is_empty() { return bad_request("ST_JOURNAL_BAD","need characterId") }
    let category = body.get("category").and_then(|v| v.as_str()).unwrap_or("memory").to_string();
    let emotion_label = body.get("emotionLabel").or_else(|| body.get("emotion_label")).and_then(|v| v.as_str()).map(|s| s.to_string());
    let emotion_intensity = body.get("emotionIntensity").or_else(|| body.get("emotion_intensity")).and_then(|v| v.as_str()).map(|s| s.to_string());
    let kind = body.get("kind").and_then(|v| v.as_str()).map(|s| s.to_string());
    let mut card = kaleido_core::journal_store::JournalCard::new(id.clone(), character_id, content, sess.turn);
    card.category = category;
    card.emotion_label = emotion_label;
    card.emotion_intensity = emotion_intensity;
    card.kind = kind;
    let ret = card.clone();
    sess.journal.add_card(card, 50);
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"journal": ret})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn update_journal_card(State(state): State<AppState>, headers: HeaderMap, Path((id, card_id)): Path<(String,String)>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let content = body.get("content").and_then(|v| v.as_str()).map(|s| s.to_string());
    let feeling = body.get("feeling").or_else(|| body.get("emotionLabel")).and_then(|v| v.as_str()).map(|s| s.to_string());
    if !sess.journal.revise(&card_id, content, feeling) { return not_found("ST_JOURNAL_NOT_FOUND","card not found") }
    let ret = sess.journal.cards.iter().find(|c| c.id==card_id).cloned();
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"journal": ret})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn delete_journal_card(State(state): State<AppState>, headers: HeaderMap, Path((id, card_id)): Path<(String,String)>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    if !sess.journal.retire(&card_id) { return not_found("ST_JOURNAL_NOT_FOUND","card not found") }
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn toggle_pin_journal(State(state): State<AppState>, headers: HeaderMap, Path((id, card_id)): Path<(String,String)>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let pinned = body.get("pinned").and_then(|v| v.as_bool()).unwrap_or(true);
    if !sess.journal.set_pinned(&card_id, pinned) { return not_found("ST_JOURNAL_NOT_FOUND","card not found") }
    match state.sessions_tavern.save(sess.clone()) { Ok(_)=>Json(json!({"ok":true,"pinned": pinned})).into_response(), Err(e)=>map_core_err(e)}
}

/// [羁绊活数值 吞噬 Front Porch AI relationship_service] Bond/Trust API.
pub(crate) async fn get_relationships(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"relationships": sess.relationships})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn put_relationships(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    // body: {characterId, bondDelta, trustDelta, fixation, stance, withUser}
    let cid = body.get("characterId").or_else(|| body.get("character_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if cid.is_empty() { return bad_request("ST_REL_BAD","need characterId") }
    let cid0: String = body.get("characterId").or_else(|| body.get("character_id")).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    // [关系自检] 空角色拒绝（吞噬 AI-Reader-V2 profile_quality 口径：脏数据不进库）
    if cid0.is_empty() {
        return map_core_err(kaleido_core::CoreError::BadRequest("characterId required".into()));
    }
    let bond_d = body.get("bondDelta").or_else(|| body.get("bond_delta")).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let trust_d = body.get("trustDelta").or_else(|| body.get("trust_delta")).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    // [偏好加权] event 文本命中 likes/dislikes 则 bond 放大 1.5x
    let event_text = body.get("event").or_else(|| body.get("reason")).and_then(|v| v.as_str()).unwrap_or("");
    let weight = sess.preferences.get(&cid).map(|p| kaleido_core::promise::preference_weight(event_text, &p.likes, &p.dislikes)).unwrap_or(1.0);
    let bond_d = ((bond_d as f64) * weight).round() as i32;
    let b = sess.relationships.entry(cid.clone()).or_default();
    let (bond_cross, trust_cross) = b.apply_delta(bond_d, trust_d);
    if let Some(fix)=body.get("fixation").and_then(|v| v.as_str()) { b.set_fixation(fix); }
    if let Some(st)=body.get("stance").or_else(|| body.get("spatialStance")).and_then(|v| v.as_str()) { b.set_spatial(st); }
    if let Some(v)=body.get("withUser").and_then(|v| v.as_bool()) { b.with_user = Some(v); }
    // long check + decay bookkeeping + milestone
    b.maybe_long_check();
    // tier milestone + diary plant (warm/hurt/relieved/wary, salience-gated)
    let sid0 = sess.session_id.clone();
    let turn0 = sess.turn;
    let msg_n0 = sess.messages.len() as i64;
    let kick0 = sess.journal.salience_gate.allow(&sid0, msg_n0);
    if let Some(tier)=bond_cross { if let Some(m)=kaleido_core::relationship_tiers::check_milestone(&cid, "bond", tier, turn0, &sess.milestones) {
        let label = m.label.clone(); let rose = tier > 0; sess.milestones.push(m);
        if kick0 { sess.journal.plant_milestone(&sid0, &cid, format!("Bond {}{} ({})", if rose {"deepened to "} else {"cooled to "}, label, cid), "bond", rose, turn0); }
    } }
    if let Some(tier)=trust_cross { if let Some(m)=kaleido_core::relationship_tiers::check_milestone(&cid, "trust", tier, turn0, &sess.milestones) {
        let label = m.label.clone(); let rose = tier > 0; sess.milestones.push(m);
        if kick0 { sess.journal.plant_milestone(&sid0, &cid, format!("Trust {}{} ({})", if rose {"rose to "} else {"fell to "}, label, cid), "trust", rose, turn0); }
    } }
    let ret = sess.relationships.get(&cid).cloned();
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"bond": ret})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn tick_relationships(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let mut decayed=false;
    for b in sess.relationships.values_mut() { if b.maybe_decay() { decayed=true; } }
    match state.sessions_tavern.save(sess.clone()) { Ok(_)=>Json(json!({"ok":true,"decayed": decayed, "relationships": sess.relationships})).into_response(), Err(e)=>map_core_err(e)}
}

/// [Swipe 多备选] 同条回复备选查看/切换。
pub(crate) async fn get_swipe(State(state): State<AppState>, headers: HeaderMap, Path((id, msg_id)): Path<(String,String)>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) {
        Ok(sess) => match sess.messages.iter().find(|m| m.id==msg_id) {
            Some(m) => Json(json!({"id": m.id, "content": m.content, "swipes": m.swipes, "swipe_index": m.swipe_index})).into_response(),
            None => not_found("ST_MSG_NOT_FOUND","message not found"),
        },
        Err(e)=>map_core_err(e),
    }
}
pub(crate) async fn put_swipe(State(state): State<AppState>, headers: HeaderMap, Path((id, msg_id)): Path<(String,String)>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let idx = body.get("index").or_else(|| body.get("swipe_index")).and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let Some(m) = sess.messages.iter_mut().find(|m| m.id==msg_id) else { return not_found("ST_MSG_NOT_FOUND","message not found") };
    if m.role != "assistant" { return bad_request("ST_SWIPE_ROLE","only assistant messages have swipes") }
    // all versions = [content] + swipes dedup; index addresses that list
    let mut versions: Vec<String> = vec![m.content.clone()];
    for s in &m.swipes { if !versions.contains(s) { versions.push(s.clone()); } }
    if idx >= versions.len() { return bad_request("ST_SWIPE_RANGE","index out of range") }
    let new_content = versions[idx].clone();
    // rebuild swipes = all other versions
    let mut new_swipes: Vec<String> = versions.into_iter().enumerate().filter(|(i,_)| *i!=idx).map(|(_,s)| s).collect();
    // cap 10
    if new_swipes.len() > 10 { new_swipes = new_swipes[new_swipes.len()-10..].to_vec(); }
    m.content = new_content;
    m.swipes = new_swipes;
    m.swipe_index = 0;
    match state.sessions_tavern.save(sess.clone()) {
        Ok(_)=> {
            let mm = sess.messages.iter().find(|x| x.id==msg_id).cloned();
            Json(json!({"ok":true,"message": mm})).into_response()
        },
        Err(e)=>map_core_err(e),
    }
}

/// [Our Story 时间线] milestones/journal/growth/objectives/chance 聚合只读。
pub(crate) async fn get_storyline(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) {
        Ok(sess) => {
            let mut items: Vec<serde_json::Value> = vec![];
            for m in &sess.milestones { items.push(json!({"kind":"milestone","turn": m.turn, "text": format!("{} · {} · {}", m.character, m.label, m.kind)})); }
            for c in &sess.journal.cards { if c.pinned || c.emotion_intensity.as_deref()==Some("strong") || matches!(c.kind.as_deref(), Some("dream")|Some("milestone")|Some("promise")|Some("ambition")|Some("item")) { items.push(json!({"kind":"memory","turn": c.created_at_turn, "text": c.content, "receipts": c.source_positions, "id": c.id})); } }
            let actives = sess.growth.active_for_all();
            for (cid, rings) in actives { for r in rings { items.push(json!({"kind":"ring","turn": r.created_at_turn, "text": format!("{cid}：{}", r.trigger_event)})); } }
            for o in &sess.objectives { if o.status=="completed" { items.push(json!({"kind":"objective","turn": o.created_at_turn, "text": o.title})); } }
            for c in sess.episodes.crumbs.iter().take(20) { items.push(json!({"kind":"episode","turn": c.created_at_turn, "text": format!("[{}] {}", c.kind, c.content)})); }
            items.sort_by(|a,b| a.get("turn").and_then(|v| v.as_u64()).unwrap_or(0).cmp(&b.get("turn").and_then(|v| v.as_u64()).unwrap_or(0)));
            Json(json!({"storyline": items, "count": items.len()})).into_response()
        },
        Err(e)=>map_core_err(e),
    }
}

/// [承诺债务/偏好 吞噬 Front Porch AI promise_debt/preference] Promises + Prefs API.
pub(crate) async fn get_promises(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"promises": sess.promises.promises})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn create_promise(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if text.is_empty() { return bad_request("ST_PROMISE_BAD","need text") }
    let character = body.get("character").or_else(|| body.get("characterId")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let party = body.get("party").and_then(|v| v.as_str()).unwrap_or("char").to_string();
    let p = kaleido_core::promise::Promise::new(character, party, text, sess.turn);
    let ret=p.clone(); sess.promises.push(p);
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"promise": ret})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn resolve_promise(State(state): State<AppState>, headers: HeaderMap, Path((id, pid)): Path<(String,String)>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let kept = body.get("kept").and_then(|v| v.as_bool()).unwrap_or(true);
    let deltas = sess.promises.resolve(&pid, kept, sess.turn);
    // 失信/守信联动 relationship + diary promise card (salience-gated)
    let sid_p = sess.session_id.clone();
    let turn_p = sess.turn;
    let kick_p = sess.journal.salience_gate.allow(&sid_p.clone(), sess.messages.len() as i64);
    if let Some((bond_d, trust_d)) = deltas {
        let ptext = sess.promises.promises.iter().find(|p| p.id==pid).map(|p| (p.character.clone(), p.text.clone()));
        // diary promise card (kept/broken, salience-gated)
        if kick_p {
            if let Some((ch, tx)) = ptext.clone() {
                let emo = if kept {"relieved"} else {"hurt"};
                sess.journal.maybe_write_auto(&sid_p, if ch.is_empty() {"narrator"} else {ch.as_str()}, format!("Promise {}: {}", if kept {"kept"} else {"broken"}, tx), "promise", Some(emo.into()), turn_p);
            }
        }
        let character = ptext.map(|(c,_)| c).unwrap_or_default();
        if !character.is_empty() {
            let b = sess.relationships.entry(character.clone()).or_default();
            let (bc, tc) = b.apply_delta(bond_d, trust_d);
            if let Some(tier)=bc { if let Some(m)=kaleido_core::relationship_tiers::check_milestone(&character, "bond", tier, turn_p, &sess.milestones) {
                let lb=m.label.clone(); let rose=tier>0; sess.milestones.push(m);
                if kick_p { sess.journal.plant_milestone(&sid_p, &character, format!("Bond {}{}", if rose {"deepened to "} else {"cooled to "}, lb), "bond", rose, turn_p); }
            } }
            if let Some(tier)=tc { if let Some(m)=kaleido_core::relationship_tiers::check_milestone(&character, "trust", tier, turn_p, &sess.milestones) {
                let lb=m.label.clone(); let rose=tier>0; sess.milestones.push(m);
                if kick_p { sess.journal.plant_milestone(&sid_p, &character, format!("Trust {}{}", if rose {"rose to "} else {"fell to "}, lb), "trust", rose, turn_p); }
            } }
        }
    }
    match state.sessions_tavern.save(sess.clone()) { Ok(_)=>Json(json!({"ok":true,"deltas": deltas.map(|(b,t)| json!({"bond":b,"trust":t}))})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn get_preferences(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"preferences": sess.preferences})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn put_preferences(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    // body: {characterId, likes:[], dislikes:[]}
    let cid = body.get("characterId").or_else(|| body.get("character_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if cid.is_empty() { return bad_request("ST_PREF_BAD","need characterId") }
    let likes: Vec<String> = body.get("likes").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let dislikes: Vec<String> = body.get("dislikes").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    sess.preferences.insert(cid, kaleido_core::promise::Prefs { likes, dislikes });
    match state.sessions_tavern.save(sess.clone()) { Ok(_)=>Json(json!({"ok":true,"preferences": sess.preferences})).into_response(), Err(e)=>map_core_err(e)}
}

/// [心情/在场 吞噬 Front Porch AI mood/presence] Mood + Presence API.
pub(crate) async fn get_mood(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) {
        Ok(sess) => {
            let cid0 = sess.present_character_ids.first().cloned().unwrap_or_default();
            let mb = sess.needs.get(&cid0).map(|nd| {
                let m: std::collections::HashMap<String,i32> = nd.vector.clone();
                let miserable = ["暴雨","大雨","暴雪","大雪","雾","大雾"].iter().any(|x| sess.game_clock.weather.contains(x));
                let beautiful = sess.game_clock.weather=="晴";
                kaleido_core::mood_presence::derive_mood(&m, &sess.game_clock.time_of_day, miserable, beautiful)
            }).unwrap_or_else(kaleido_core::mood_presence::MoodBaseline::neutral);
            Json(json!({"mood": mb})).into_response()
        },
        Err(e)=>map_core_err(e),
    }
}
pub(crate) async fn get_presence(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"presence": sess.presence})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn put_presence(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    // body: {characterId, occupation, hours, brief, workDays}
    let cid = body.get("characterId").or_else(|| body.get("character_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    if cid.is_empty() { return bad_request("ST_PRES_BAD","need characterId") }
    let e = sess.presence.entry(cid).or_default();
    if let Some(v)=body.get("occupation").and_then(|v| v.as_str()) { e.occupation=v.to_string(); }
    if let Some(v)=body.get("hours").and_then(|v| v.as_str()) { e.hours=v.to_string(); }
    if let Some(v)=body.get("brief").and_then(|v| v.as_str()) { e.brief=v.to_string(); }
    if let Some(a)=body.get("workDays").or_else(|| body.get("work_days")).and_then(|v| v.as_array()) { e.work_days=Some(a.iter().filter_map(|v| v.as_i64().map(|x| x as i32)).collect()); }
    match state.sessions_tavern.save(sess.clone()) { Ok(_)=>Json(json!({"ok":true,"presence": sess.presence})).into_response(), Err(e)=>map_core_err(e)}
}

/// [Journal 冷卡召回] cosine(0.45, top3) + 关键词 floor；命中 rewarm。
pub(crate) async fn recall_journals(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let query_text = body.get("query_text").or_else(|| body.get("queryText")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let query_vector: Vec<f32> = body.get("query_vector").or_else(|| body.get("queryVector")).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect()).unwrap_or_default();
    let character_id = body.get("characterId").or_else(|| body.get("character_id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    // character scope: explicit or all present
    let cids: Vec<String> = if character_id.is_empty() {
        if sess.present_character_ids.is_empty() {
            sess.journal.cards.iter().map(|c| c.character_id.clone()).collect::<std::collections::HashSet<_>>().into_iter().collect()
        } else { sess.present_character_ids.clone() }
    } else { vec![character_id] };
    let mut recalled: Vec<serde_json::Value> = vec![];
    for cid in cids {
        let ids = sess.journal.recall_cold(&sess.session_id, &cid, &query_vector, &query_text, 0.45, 3);
        for rid in ids {
            sess.journal.rewarm(&rid);
            if let Some(c) = sess.journal.cards.iter().find(|c| c.id==rid) {
                recalled.push(json!({"id": c.id, "characterId": c.character_id, "content": c.content, "heat": c.heat}));
            }
        }
    }
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"recalled": recalled})).into_response(), Err(e)=>map_core_err(e)}
}
/// [Journal embedding 回填] 无向量卡片用 fastembed 本地补向量（无则跳过）。
pub(crate) async fn embed_missing_journals(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    // collect contents needing vectors
    let need: Vec<(String, String)> = sess.journal.cards.iter().filter(|c| c.embedding.is_empty()).map(|c| (c.id.clone(), c.content.clone())).collect();
    if need.is_empty() { return Json(json!({"ok":true,"embedded": 0})).into_response(); }
    // use embed_local singleton via AppState? fallback: skip if unavailable — try kaleido_server embed helper
    let mut done = 0usize;
    for (cid, content) in need.iter().take(20) {
        // best-effort local fastembed; unavailable → keep no-RAG floor
        match crate::embed_local::embed_one(content) {
            Ok(vec) => { if let Some(card) = sess.journal.cards.iter_mut().find(|c| &c.id==cid) { card.embedding = vec; done += 1; } }
            Err(_) => break,
        }
    }
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"embedded": done})).into_response(), Err(e)=>map_core_err(e)}
}

/// [全自动事件提取] 开关。
pub(crate) async fn get_event_extract(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) { Ok(sess)=>Json(json!({"eventExtract": sess.event_extract})).into_response(), Err(e)=>map_core_err(e)}
}
pub(crate) async fn put_event_extract(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>, Json(body): Json<serde_json::Value>) -> Response {
    if let Err(r) = session_from(&state, &headers) { return r; }
    let mut sess = match state.sessions_tavern.get(&id) { Ok(s)=>s, Err(e)=>return map_core_err(e) };
    let v = body.get("eventExtract").or_else(|| body.get("enabled")).and_then(|x| x.as_bool()).unwrap_or(true);
    sess.event_extract = v;
    match state.sessions_tavern.save(sess) { Ok(_)=>Json(json!({"ok":true,"eventExtract": v})).into_response(), Err(e)=>map_core_err(e)}
}

/// [世界书定时] sticky/cooldown 剩余只读（侧栏 pill）。
pub(crate) async fn get_timed_world_info(State(state): State<AppState>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    let s = match session_from(&state, &headers) { Ok(x)=>x, Err(r)=>return r };
    match state.sessions_tavern.get_for_owner(&id, &s.user_id) {
        Ok(sess) => {
            let clen = sess.messages.len() as i32;
            let sticky: Vec<serde_json::Value> = sess.timed_world_info.sticky.iter()
                .filter(|(_, e)| clen >= e.start && clen < e.end)
                .map(|(k, e)| json!({"key": k, "remaining": e.end - clen, "start": e.start, "end": e.end})).collect();
            let cooldown: Vec<serde_json::Value> = sess.timed_world_info.cooldown.iter()
                .filter(|(_, e)| clen >= e.start && clen < e.end)
                .map(|(k, e)| json!({"key": k, "remaining": e.end - clen})).collect();
            Json(json!({"sticky": sticky, "cooldown": cooldown, "chatLen": clen})).into_response()
        },
        Err(e)=>map_core_err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActorArchiveRequest {
    /// 只归档该角色；缺省归档全部。
    #[serde(default)]
    character_id: Option<String>,
    /// 归档原因（auto | manual | story），默认 manual。
    #[serde(default)]
    reason: Option<String>,
}

/// S6: 将角色当前状态快照写入 actor_states.archive。
pub(crate) async fn archive_actor_state(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ActorArchiveRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    // M-2 (CAS): capture revision for optimistic-concurrency save below.
    let base_revision = sess.updated_at.clone();
    let reason = body.reason.unwrap_or_else(|| "manual".into());
    let now = Utc::now().to_rfc3339();
    let mut archived = 0u32;
    match &body.character_id {
        Some(cid) => {
            if sess.actor_states.archive_actor(cid, &reason, &now).is_none() {
                return not_found("ST_ACTOR_NOT_FOUND", format!("actor not found: {cid}"));
            }
            archived = 1;
        }
        None => {
            let ids: Vec<String> = sess.actor_states.actors.keys().cloned().collect();
            for cid in ids {
                if sess.actor_states.archive_actor(&cid, &reason, &now).is_some() {
                    archived += 1;
                }
            }
        }
    }
    match state.sessions_tavern.save_with_revision(sess, &base_revision) {
        Ok(_) => Json(json!({ "ok": true, "archived": archived })).into_response(),
        Err(kaleido_core::CoreError::Conflict(_msg)) => return conflict("ST_CONCURRENT_WRITE", "session was modified concurrently; please retry"),
        Err(e) => map_core_err(e),
    }
}

/// 归档列表。
pub(crate) async fn list_actor_archives(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => Json(json!({ "archives": s.actor_states.archive })).into_response(),
        Err(e) => map_core_err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActorArchiveRestoreRequest {
    /// 只恢复该角色最近一次归档。
    #[serde(default)]
    character_id: Option<String>,
    /// 全部恢复到各自最近一次快照。
    #[serde(default)]
    restore_all: bool,
}

/// S6: 从归档恢复角色状态（缺省恢复最近一条快照）。
pub(crate) async fn restore_actor_archive(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ActorArchiveRestoreRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    // M-2 (CAS): capture revision for optimistic-concurrency save below.
    let base_revision = sess.updated_at.clone();
    let mut restored: Vec<String> = Vec::new();
    match (&body.character_id, body.restore_all) {
        (Some(cid), _) => {
            if sess.actor_states.restore_actor(cid) {
                restored.push(cid.clone());
            } else {
                return not_found("ST_ACTOR_NO_ARCHIVE", format!("no archive for actor: {cid}"));
            }
        }
        (None, true) => {
            let mut seen = std::collections::HashSet::new();
            let ids: Vec<String> = sess
                .actor_states
                .archive
                .iter()
                .rev()
                .filter_map(|snap| {
                    if seen.insert(snap.character_id.clone()) {
                        Some(snap.character_id.clone())
                    } else {
                        None
                    }
                })
                .collect();
            for cid in ids {
                if sess.actor_states.restore_actor(&cid) {
                    restored.push(cid);
                }
            }
        }
        (None, false) => {
            if let Some(snap) = sess.actor_states.archive.last() {
                let cid = snap.character_id.clone();
                if sess.actor_states.restore_actor(&cid) {
                    restored.push(cid);
                }
            }
        }
    }
    match state.sessions_tavern.save_with_revision(sess, &base_revision) {
        Ok(_) => Json(json!({ "ok": true, "restored": restored })).into_response(),
        Err(kaleido_core::CoreError::Conflict(_msg)) => return conflict("ST_CONCURRENT_WRITE", "session was modified concurrently; please retry"),
        Err(e) => map_core_err(e),
    }
}

// ─── X5: 作品级文笔风格分析（吞噬自 xiami writing_style.rs）────────────────

/// X5 (吞噬自 xiami writing_style.rs): `POST /api/v1/story-tavern/style-analysis`
/// 请求：`{ "sourceText": "...", "workId": "<可选>" }`
/// 流程：`prepare_analysis_sample` 采样校验 → 失败 400；成功走现有
/// `resolve_llm` + `stream_chat_completions`（system=12 维文笔分析提示词，
/// user=`<novel_sample>` 包装，温度 0.2 / max_tokens 4096 / 120s）。
/// 响应：`{ "stylePrompt": "...", "sampleChars": N }`。
/// workId 落库为 YAGNI 后置（pack 尚无直接 narrative_style 字段，仅返回不落库）。
pub async fn style_analysis(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<StyleAnalysisRequest>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    // 采样 + 校验（过短/过长/空输入 → 400 带错误文案）。
    let sample = match kaleido_core::st_writing_style::prepare_analysis_sample(&body.source_text) {
        Ok(s) => s,
        Err(e) => {
            return bad_request("ST_BAD_REQUEST", e);
        }
    };
    let llm = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    let prov_kind = crate::llm_stream::runtime_provider_kind(&llm, &state.provider_kind);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return internal("ST_LLM_NOT_CONFIGURED", "LLM not configured: base_url or api_key is empty");
    }
    let system = kaleido_core::st_writing_style::analysis_system_prompt();
    let user = kaleido_core::st_writing_style::analysis_user_prompt(&sample);
    let raw = match stream_chat_completions_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        &prov_kind,
        system,
        &user,
        0.2,
        8192,
        120,
        |_| true,
    )
    .await
    {
        Ok(raw) => raw,
        Err(e) => {
            return internal("ST_LLM_CALL_FAILED", e);
        }
    };
    // [fix 2026-08-15 style_stats 接线] 文风确定性统计（吸收自 oh-story Step 4）：
    // 句长分布/标点密度/段落节奏 为确定性测量（confidence: high），
    // 与 LLM 风格提示词互补——量化基准 + 质性描述。
    let st = kaleido_core::style_stats::compute_style_stats(&sample);
    let ps = kaleido_core::style_stats::compute_paragraph_stats(&sample);
    // [fix 2026-08-15 文风落库] workId（pack id）提供时，把 stylePrompt 落库为
    // `data/story-packs/<pack>/style-profiles/<workId>.txt`——供 /config style_source
    // 一键外挂（18+ 特化文风等）。落库失败仅告警不阻断响应。
    let mut saved_profile: Option<String> = None;
    if let Some(wid) = body.work_id.as_deref().filter(|w| !w.trim().is_empty()) {
        // workId 必须是已存在的 pack id；按标题模糊匹配兜底（用户可能传「智取」而非全 id）
        let resolved_id = {
            let packs = state.packs.list().map_err(|e| e.to_string()).unwrap_or_default();
            let q = wid.trim();
            packs
                .iter()
                .find(|p| p.id == q)
                .or_else(|| packs.iter().find(|p| p.title.contains(q) || q.contains(&p.title)))
                .map(|p| p.id.clone())
                .unwrap_or_else(|| wid.to_string())
        };
        let pack_dir = state.app_state.data_root().story_packs_dir().join(&resolved_id);
        let profile_dir = pack_dir.join("style-profiles");
        let profile_path = profile_dir.join(format!("{resolved_id}.txt"));
        if let Some(parent) = profile_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&profile_path, raw.trim()) {
            Ok(_) => saved_profile = Some(profile_path.display().to_string()),
            Err(e) => {
                tracing::warn!("文风落库失败 {wid}: {e}");
            }
        }
    }
    Json(json!({
        "stylePrompt": raw.trim(),
        "sampleChars": sample.chars().count(),
        "savedProfile": saved_profile,
        "sentenceStats": {
            "total": st.total,
            "shortLt15Pct": st.short_lt15_pct,
            "mid15to30Pct": st.mid_15to30_pct,
            "longGt30Pct": st.long_gt30_pct,
            "avgLen": st.avg_len,
            "punctDensityPct": st.punct_density_pct,
        },
        "paragraphStats": {
            "paragraphs": ps.paragraphs,
            "avgParaLen": ps.avg_para_len,
        },
    }))
    .into_response()
}

// ─── U13 M1: Character Memory Analysis / Optimization ───────────────────────

/// Helper: non-streaming LLM call returning text content.
/// 回合 worker 保存前重合并后台字段：worker sess 在回合初加载，后台任务
/// （事件提取/roster/记忆提取）在回合中落盘的增量会被 worker 的整包 save 覆盖。
/// 保存前从盘 fresh 读一次，把 7 组后台字段并回 sess（worker 本回合的同名字段以 worker 为准合并）。
pub(crate) fn remerge_bg_fields(store: &kaleido_core::TavernSessionStore, sess: &mut kaleido_core::TavernSession) {
    let Ok(fresh) = store.get(&sess.session_id) else { return; };
    for (cid, v) in fresh.pockets { sess.pockets.entry(cid).or_insert(v); }
    for c in fresh.journal.cards {
        if !sess.journal.cards.iter().any(|x| x.id == c.id) { sess.journal.cards.push(c); }
    }
    for pr in fresh.promises.promises {
        if !sess.promises.promises.iter().any(|x| x.id == pr.id) { sess.promises.push(pr); }
    }
    for r in fresh.growth.rings {
        if !sess.growth.rings.iter().any(|x| x.id == r.id) { sess.growth.rings.push(r); }
    }
    for (cid, b) in fresh.relationships { sess.relationships.entry(cid).or_insert(b); }
    for m in fresh.milestones {
        if !sess.milestones.iter().any(|x| x.character == m.character && x.label == m.label) { sess.milestones.push(m); }
    }
    for (cid, n) in fresh.needs { sess.needs.entry(cid).or_insert(n); }
}

/// 角色名→会话 key 归一：pack id 优先；已有 key（名或 id 任一命中）复用，防编外角色双 key 分裂；
/// 都没有则回退名本身。
pub(crate) fn resolve_cid(pack_chars: &[(String, String)], existing: impl Fn(&str) -> bool, name: &str) -> String {
    if let Some((id, _)) = pack_chars.iter().find(|(_, n)| n == name) { return id.clone(); }
    if existing(name) { return name.to_string(); }
    for (id, n) in pack_chars {
        if existing(id) && (n == name || id == name) { return id.clone(); }
    }
    name.to_string()
}

/// [全自动事件提取] 回合末后台 LLM：从玩家消息+本回合正文提取结构化事件，直写存储。
/// 小模型低 token（max_tokens 512, temperature 0.1）：只做提取不做创作。
/// 返回 JSON：{gives:[{from,to,item}], promises:[{character,text}], growth:[{character,event,strength}],
///  bond:[{character,bondDelta,trustDelta}], needs:[{character,need,delta}], journal:[{character,content,kind}]}。
/// 全部 fail-open：解析失败/空即跳过；写前去重；salience 门限流种卡。
pub(crate) async fn run_event_extract(
    store: &kaleido_core::TavernSessionStore,
    appst: &kaleido_core::AppStateStore,
    base: Option<&str>,
    key: Option<&str>,
    model: &str,
    session_id: &str,
    turn: u32,
    user_msg: &str,
    full_text: &str,
    present: &[String],
    focus: &str,
    pack_chars: &[(String, String)],
) -> Result<(), String> {
    let names: Vec<String> = pack_chars.iter().map(|(_, n)| n.clone()).collect();
    let name_of = |cid: &str| pack_chars.iter().find(|(id, _)| id == cid).map(|(_, n)| n.clone()).unwrap_or_else(|| cid.to_string());
    let sys = format!(
        "你是剧情事件提取器。只做提取，不创作、不续写。角色：{}。只输出 JSON（无 markdown 包裹），七个数组键必须全有（gives/promises/growth/bond/needs/journal/checks），无事件则为空数组。checks：正文或玩家输入中有人尝试了结果不确定的行动（潜行/偷听/攀爬/说服/攻击/闪避/追踪/开锁等）即报一笔(action,intent)（Scribe 低语，吞噬 loreweaver scribe 口径；关键词触发已废弃）。规则：gives 包括人物间给予（给出/递出/塞入/放在桌上，填 from/to/item）与自取拾取（拿起/收起/攥在手心/装进口袋，to 填玩家，from 可空）；promises 仅当明确将来时承诺；growth：任何角色态度软化/强硬/透露身世/情绪波动都算一次（strength 0.3-0.8）；bond：任何好感/信任增减都给分（正负1到10），初次善意至少+2；journal：每回合至少记1条本回合最值得记住的事（kind=moment）；needs：疲惫/饥饿/寒冷/口渴等生理信号出现才记。",
        names.join("、")
    );
    let user = format!("玩家输入：{}\n\n本回合正文：{}\n\n在场角色 id：{}", user_msg, full_text.chars().take(3000).collect::<String>(), present.join(","));
    let raw = call_llm_extract_with(appst, base, key, model, &sys, &user).await?;
    let mut v: Option<serde_json::Value> = crate::llm_stream::extract_json_value(&raw)
        .or_else(|| serde_json::from_str(&raw).ok());
    // [waidrin ②] schema 拒收返工（吸收自 waidrin State.parse 双校验 + DeterminFlow output_validation 口径）：parse 失败重试一次
    if v.is_none() {
        tracing::warn!(%session_id, turn, "st event_extract bg: json parse failed, retry once");
        let retry_sys = "你是剧情事件提取器。只输出 JSON（无 markdown 包裹），七个数组键必须全有（gives/promises/growth/bond/needs/journal/checks），无事件则为空数组。不要输出任何解释文字。";
        if let Ok(retry_raw) = call_llm_extract_with(appst, base, key, model, retry_sys, &user).await {
            v = crate::llm_stream::extract_json_value(&retry_raw)
                .or_else(|| serde_json::from_str(&retry_raw).ok());
        }
    }
    let v: serde_json::Value = v.ok_or_else(|| "extract json parse failed".to_string())?;
    let mut sess = store.get(session_id).map_err(|e| e.to_string())?;
    let sid = sess.session_id.clone();
    let mut dirty = false;
    // [幻觉过滤] grounding 语料 = 本回合正文+玩家输入（吞噬 AI-Reader-V2 hallucination_filter 口径）
    let corpus = format!("{user_msg}\n{full_text}");
    let roster: Vec<String> = pack_chars.iter().map(|(_, n)| n.clone()).collect();
    let mut hallu: Vec<String> = Vec::new();
    // [waidrin ①] ** 引用=强 grounding 证据（扫正文 bold 引用，命中即算登场实锤）
    let bold_refs = scan_bold_refs(&corpus);
    let grounded = |name: &str| -> bool {
        roster.iter().any(|n| n == name)
            || bold_refs.iter().any(|n| n == name)
            || kaleido_core::grounding::grounded_in(name, &[], &corpus)
    };
    {
        let counts = ["gives","promises","growth","bond","needs","journal","checks"].iter()
            .map(|k| format!("{}={}", k, v.get(k).and_then(|x| x.as_array()).map(|a| a.len()).unwrap_or(0)))
            .collect::<Vec<_>>().join(" ");
        let all_zero = ["gives","promises","growth","bond","needs","journal","checks"].iter()
            .all(|k| v.get(k).and_then(|x| x.as_array()).map(|a| a.is_empty()).unwrap_or(true));
        if all_zero {
            tracing::info!(%session_id, turn, counts = %counts, raw_head = %raw.chars().take(800).collect::<String>(), "st event_extract bg: parsed ZERO");
        } else {
            tracing::info!(%session_id, turn, counts = %counts, raw_len = raw.len(), "st event_extract bg: parsed");
        }
    }
    // gives → pockets apply + item journal card
    if let Some(arr) = v.get("gives").and_then(|x| x.as_array()) {
        for g in arr {
            let (from_name, to_name, item) = (
                g.get("from").and_then(|x| x.as_str()).unwrap_or("").trim(),
                g.get("to").or_else(|| g.get("target")).or_else(|| g.get("character")).and_then(|x| x.as_str()).unwrap_or("").trim(),
                g.get("item").or_else(|| g.get("itemName")).or_else(|| g.get("name")).and_then(|x| x.as_str()).unwrap_or("").trim(),
            );
            if item.is_empty() { continue; }
            // to 缺省/玩家 → 自取 Pickup（narrator 口袋）；to 明确他人 → Give 转移（扣 from 加 to）。
            let to_is_player = to_name.is_empty() || to_name.contains("玩家") || to_name.to_lowercase().contains("player");
            let day = sess.game_clock.day;
            if to_is_player {
                let ops = vec![kaleido_core::pockets::PocketOpReport { kind: kaleido_core::pockets::PocketOpKind::Pickup, item: item.to_string(), to: String::new(), state: String::new(), where_: String::new() }];
                let entry = sess.pockets.entry("narrator".to_string()).or_default();
                let mut events: Vec<kaleido_core::pockets::PocketEvent> = vec![];
                kaleido_core::pockets::apply_pocket_ops(entry, &ops, None, day, Some(&mut events));
                for (it, content) in kaleido_core::pockets::item_cards_from(&events) {
                    let mut card = kaleido_core::journal_store::JournalCard::new(sid.clone(), "narrator".to_string(), content, turn);
                    card.kind = Some("item".into());
                    card.metadata_item = Some(it);
                    card.category = "moment".into();
                    sess.journal.add_card(card, 50);
                }
                dirty = true;
            } else {
                let to_cid = resolve_cid(pack_chars, |k| sess.pockets.contains_key(k), to_name);
                let from_cid = if from_name.is_empty() { "narrator".to_string() }
                    else if from_name.contains("玩家") || from_name.to_lowercase().contains("player") { "narrator".into() }
                    else { resolve_cid(pack_chars, |k| sess.pockets.contains_key(k), from_name) };
                // 先从 from 扣（Give 会清掉 from 持有的同名物），再给 to 加
                let ops = vec![kaleido_core::pockets::PocketOpReport { kind: kaleido_core::pockets::PocketOpKind::Give, item: item.to_string(), to: to_cid.clone(), state: String::new(), where_: String::new() }];
                let mut moved: Vec<(String, kaleido_core::pockets::PocketItem)> = vec![];
                {
                    let entry = sess.pockets.entry(from_cid.clone()).or_default();
                    let mut cb = |to: String, it: kaleido_core::pockets::PocketItem| { moved.push((to, it)); };
                    let mut events: Vec<kaleido_core::pockets::PocketEvent> = vec![];
                    kaleido_core::pockets::apply_pocket_ops(entry, &ops, Some(&mut cb), day, Some(&mut events));
                    for (it, content) in kaleido_core::pockets::item_cards_from(&events) {
                        let mut card = kaleido_core::journal_store::JournalCard::new(sid.clone(), from_cid.clone(), content, turn);
                        card.kind = Some("item".into());
                        card.metadata_item = Some(it);
                        card.category = "moment".into();
                        sess.journal.add_card(card, 50);
                    }
                }
                for (to, it) in moved {
                    let dest = sess.pockets.entry(to.clone()).or_default();
                    if !dest.carrying.iter().any(|x| kaleido_core::pockets::same_item(&x.name, &it.name)) {
                        dest.carrying.push(it);
                    }
                }
                // from==narrator 且持有 → 上面 Give 已扣；若 from 无此物（凭空给）→ to 直接 Pickup 补
                if sess.pockets.get(&to_cid).map(|p| !p.carrying.iter().any(|x| kaleido_core::pockets::same_item(&x.name, item))).unwrap_or(true) {
                    let dest = sess.pockets.entry(to_cid.clone()).or_default();
                    let ops2 = vec![kaleido_core::pockets::PocketOpReport { kind: kaleido_core::pockets::PocketOpKind::Pickup, item: item.to_string(), to: String::new(), state: String::new(), where_: String::new() }];
                    let mut events2: Vec<kaleido_core::pockets::PocketEvent> = vec![];
                    kaleido_core::pockets::apply_pocket_ops(dest, &ops2, None, day, Some(&mut events2));
                }
                dirty = true;
            }
        }
    }
    tracing::info!(%session_id, turn, gives_raw = %v.get("gives").map(|x| x.to_string()).unwrap_or_default().chars().take(500).collect::<String>(), "st event_extract bg: gives raw");
    // promises → promise store
    if let Some(arr) = v.get("promises").and_then(|x| x.as_array()) {
        for pr in arr {
            let (ch_name, text) = (
                pr.get("character").and_then(|x| x.as_str()).unwrap_or("").trim(),
                pr.get("text").or_else(|| pr.get("promise")).or_else(|| pr.get("content")).and_then(|x| x.as_str()).unwrap_or("").trim(),
            );
            if text.is_empty() { continue; }
            let cid = if ch_name.is_empty() { "narrator".into() } else { resolve_cid(pack_chars, |k| sess.promises.promises.iter().any(|x| x.character == k), ch_name) };
            sess.promises.push(kaleido_core::promise::Promise::new(&cid, "char", text, turn));
            dirty = true;
        }
    }
    // checks → scribe_whispers（cap 5；下回合 prompt 消费后清空）
    if let Some(arr) = v.get("checks").and_then(|x| x.as_array()) {
        let mut ws: Vec<String> = vec![];
        for c in arr.iter().take(5) {
            let (act, intent) = (
                c.get("action").and_then(|x| x.as_str()).unwrap_or("").trim(),
                c.get("intent").and_then(|x| x.as_str()).unwrap_or("").trim(),
            );
            if act.is_empty() && intent.is_empty() { continue; }
            if intent.is_empty() { ws.push(act.to_string()); }
            else { ws.push(format!("{}（{}）", act, intent)); }
        }
        if !ws.is_empty() {
            sess.scribe_whispers = ws;
            dirty = true;
            tracing::info!(%session_id, turn, n = sess.scribe_whispers.len(), "st event_extract bg: scribe whispers stored");
        }
    }
    // growth → strengthen
    tracing::info!(%session_id, turn, growth_raw = %v.get("growth").map(|x| x.to_string()).unwrap_or_default().chars().take(500).collect::<String>(), "st event_extract bg: growth raw");
    if let Some(arr) = v.get("growth").and_then(|x| x.as_array()) {
        for g in arr {
            let (ch_name, ev, st) = (
                g.get("character").and_then(|x| x.as_str()).unwrap_or("").trim(),
                g.get("event").or_else(|| g.get("description")).or_else(|| g.get("trigger")).or_else(|| g.get("eventText")).and_then(|x| x.as_str()).unwrap_or("").trim(),
                g.get("strength").and_then(|x| x.as_f64()).unwrap_or(0.6) as f32,
            );
            if ch_name.is_empty() || ev.is_empty() { continue; }
            if !grounded(ch_name) { if !hallu.contains(&ch_name.to_string()) { hallu.push(ch_name.to_string()); } continue; }
            let cid = resolve_cid(pack_chars, |k| sess.growth.rings.iter().any(|r| r.character == k), ch_name);
            sess.growth.strengthen(&cid, ev, st, turn);
            dirty = true;
        }
    }
    // bond → relationships + milestone plant (salience-gated)
    tracing::info!(%session_id, turn, bond_raw = %v.get("bond").map(|x| x.to_string()).unwrap_or_default().chars().take(500).collect::<String>(), journal_raw = %v.get("journal").map(|x| x.to_string()).unwrap_or_default().chars().take(500).collect::<String>(), "st event_extract bg: bond/journal raw");
    if let Some(arr) = v.get("bond").and_then(|x| x.as_array()) {
        let msg_n = sess.messages.len() as i64;
        let kick = sess.journal.salience_gate.allow(&sid, msg_n);
        for b in arr {
            let chg = b.get("bondDelta").or_else(|| b.get("change")).or_else(|| b.get("delta")).or_else(|| b.get("score")).and_then(|x| x.as_i64()).unwrap_or(0);
            let (ch_name, bd, td) = (
                b.get("character").and_then(|x| x.as_str()).unwrap_or("").trim(),
                chg.clamp(-10, 10) as i32,
                b.get("trustDelta").or_else(|| b.get("trustChange")).or_else(|| b.get("trust")).and_then(|x| x.as_i64()).unwrap_or_else(|| if chg != 0 { chg * 6 / 10 } else { 0 }).clamp(-10, 10) as i32,
            );
            if ch_name.is_empty() || (bd == 0 && td == 0) { continue; }
            if !grounded(ch_name) { if !hallu.contains(&ch_name.to_string()) { hallu.push(ch_name.to_string()); } continue; }
            let cid = resolve_cid(pack_chars, |k| sess.relationships.contains_key(k), ch_name);
            let entry = sess.relationships.entry(cid.clone()).or_default();
            let (bc, tc) = entry.apply_delta(bd, td);
            if let Some(tier) = bc {
                if let Some(m) = kaleido_core::relationship_tiers::check_milestone(&cid, "bond", tier, turn, &sess.milestones) {
                    let lb = m.label.clone();
                    sess.milestones.push(m);
                    if kick { sess.journal.plant_milestone(&sid, &cid, format!("Bond deepened to {} ({})", lb, name_of(&cid)), "bond", tier > 0, turn); }
                }
            }
            if let Some(tier) = tc {
                if let Some(m) = kaleido_core::relationship_tiers::check_milestone(&cid, "trust", tier, turn, &sess.milestones) {
                    let lb = m.label.clone();
                    sess.milestones.push(m);
                    if kick { sess.journal.plant_milestone(&sid, &cid, format!("Trust rose to {} ({})", lb, name_of(&cid)), "trust", tier > 0, turn); }
                }
            }
            dirty = true;
        }
    }
    // needs → apply_scene_impact (single-need map)
    if let Some(arr) = v.get("needs").and_then(|x| x.as_array()) {
        for n in arr {
            let (ch_name, need, delta) = (
                n.get("character").and_then(|x| x.as_str()).unwrap_or("").trim(),
                n.get("need").or_else(|| n.get("needKey")).or_else(|| n.get("key")).and_then(|x| x.as_str()).unwrap_or("").trim(),
                n.get("delta").or_else(|| n.get("change")).or_else(|| n.get("value")).and_then(|x| x.as_i64()).unwrap_or(0).clamp(-30, 30) as i32,
            );
            if ch_name.is_empty() || need.is_empty() || delta == 0 { continue; }
            if !kaleido_core::needs::NEED_KEYS.contains(&need) { continue; }
            let cid = resolve_cid(pack_chars, |k| sess.needs.contains_key(k), ch_name);
            let entry = sess.needs.entry(cid).or_insert_with(kaleido_core::needs::Needs::default);
            let mut d = std::collections::HashMap::new();
            d.insert(need.to_string(), delta);
            entry.apply_scene_impact(&d, 1);
            dirty = true;
        }
    }
    // journal → maybe_write_auto (character 缺省 → 在场首位 → focus → pack 首位)
    let fallback_ch = present.first().and_then(|cid| pack_chars.iter().find(|(id, _)| id == cid).map(|(_, n)| n.as_str()))
        .or_else(|| pack_chars.iter().find(|(id, _)| id == focus).map(|(_, n)| n.as_str()))
        .or_else(|| pack_chars.first().map(|(_, n)| n.as_str()))
        .unwrap_or("");
    if let Some(arr) = v.get("journal").and_then(|x| x.as_array()) {
        for j in arr {
            let raw_ch = j.get("character").and_then(|x| x.as_str()).unwrap_or("").trim();
            let ch_name = if raw_ch.is_empty() { fallback_ch } else { raw_ch };
            let (content, kind) = (
                j.get("content").and_then(|x| x.as_str()).unwrap_or("").trim(),
                j.get("kind").and_then(|x| x.as_str()).unwrap_or("moment").trim(),
            );
            if ch_name.is_empty() || content.is_empty() { continue; }
            let cid = resolve_cid(pack_chars, |k| sess.journal.cards.iter().any(|c| c.character_id == k), ch_name);
            sess.journal.maybe_write_auto(&sid, &cid, content.to_string(), kind, None, turn);
            dirty = true;
        }
    }
    if !dirty { return Ok(()); }
    // 原子 read-modify-write（update_session 持锁）：避免与 roster/记忆提取的
    // 后保存互相覆盖。逐字段合并而非整包替换——只写本提取产出的增量。
    let want_pockets = sess.pockets.clone();
    let want_journal = sess.journal.cards.clone();
    let want_promises = sess.promises.promises.clone();
    let want_growth = sess.growth.rings.clone();
    let want_rels = sess.relationships.clone();
    let want_miles = sess.milestones.clone();
    let want_needs = sess.needs.clone();
    store.update_session(session_id, |live| {
        for (cid, p) in want_pockets { live.pockets.insert(cid, p); }
        for c in want_journal {
            if !live.journal.cards.iter().any(|x| x.id == c.id) { live.journal.cards.push(c); }
        }
        for pr in want_promises {
            if !live.promises.promises.iter().any(|x| x.id == pr.id) { live.promises.push(pr); }
        }
        for r in want_growth {
            if !live.growth.rings.iter().any(|x| x.id == r.id) { live.growth.strengthen(&r.character, &r.trigger_event, r.strength, turn); }
        }
        for (cid, b) in want_rels { live.relationships.insert(cid, b); }
        for m in want_miles {
            if !live.milestones.iter().any(|x| x.character == m.character && x.label == m.label) { live.milestones.push(m); }
        }
        for (cid, n) in want_needs { live.needs.insert(cid, n); }
        // [冲突三维] milestones bond 时间线 flip-flop + 死亡弱信号（吞噬 AI-Reader-V2 conflict_detector 口径）
        {
            use std::collections::HashMap;
            let mut tl: HashMap<(String, String), Vec<(i64, String)>> = HashMap::new();
            for m in &live.milestones {
                if m.kind == "bond" || m.kind == "trust" {
                    tl.entry((m.character.clone(), m.kind.clone())).or_default().push((m.turn as i64, m.label.clone()));
                }
            }
            let tls: Vec<((String, String), Vec<(i64, String)>)> = tl.into_iter()
                .map(|((ch, _k), v)| ((ch.clone(), ch), v))
                .collect();
            let mut conf: Vec<String> = kaleido_core::grounding::check_relation_flip(&tls)
                .into_iter().map(|c| c.description).collect();
            // 死亡弱信号：正文含 死/亡/尸/杀 + 角色在场 → low（只提示，不断言）
            for cid in live.present_character_ids.iter().chain(live.focus_character_id.as_ref()) {
                if full_text.contains("死") || full_text.contains("亡") || full_text.contains("尸体") {
                    conf.push(format!("死亡信号：{cid} 在场且本回合出现死亡描写（待核实）"));
                    break;
                }
            }
            if let Some(dg) = live.last_turn_diagnostic.as_mut() {
                dg.hallu_names = hallu.clone();
                dg.conflicts = conf;
            }
        }
        Ok(())
    }).map_err(|e| e.to_string())?;
    tracing::info!(%session_id, turn, hallu_n = hallu.len(), "st event_extract bg: wrote events");
    Ok(())
}
