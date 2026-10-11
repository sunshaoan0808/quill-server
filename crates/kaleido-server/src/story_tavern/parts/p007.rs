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

pub(crate) async fn session_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    // M-3: support short-lived SSE ticket in query (falls back to header/query token).
    let session = match crate::session_from_any(&state, &headers, Some(&params)) {
        Ok(s) => s,
        Err(r) => return r,
    };

    // Validate session (F1: ownership check)
    if let Err(e) = state.sessions_tavern.get_for_owner(&session_id, &session.user_id) {
        return map_core_err(e);
    }

    let run_id = match params.get("runId").or_else(|| params.get("run_id")) {
        Some(id) => id.to_string(),
        None => {
            return bad_request("ST_RUNID_REQUIRED", "runId required");
        }
    };

    // audit P1 IDOR: 校验 run 归属（job 存在时；查不到时放行——start 竞态窗口）
    if let Some(job) = state.jobs.get(&run_id) {
        if job.workspace_id != session.workspace_id && job.user_id != session.user_id {
            return forbidden("ST_RUN_FORBIDDEN", "run not in your workspace");
        }
    }

    let (rx, replay) = match state.hub.subscribe(&run_id) {
        Some((rx, replay)) => (rx, replay),
        None => {
            // Check if job already finished
            return match state.jobs.get(&run_id) {
                Some(job) => {
                    let status = &job.status;
                    let done = status == "done" || status == "succeeded" || status == "failed"
                        || status == "cancelled" || status == "error";
                    if done && status != "failed" && status != "error" && status != "cancelled" {
                        Json(json!({
                            "type": "result",
                            "subtype": "success",
                            "result": status,
                            "runId": run_id,
                        }))
                        .into_response()
                    } else {
                        Json(json!({
                            "type": "result",
                            "subtype": "error",
                            "result": status,
                            "runId": run_id,
                        }))
                        .into_response()
                    }
                }
                None => {
                    Json(json!({
                        "type": "result",
                        "subtype": "error",
                        "result": "not_found",
                        "runId": run_id,
                    }))
                    .into_response()
                }
            };
        }
    };

    let run_id_for_cleanup = run_id.clone();
    Sse::new(stream_from_rx(
        run_id,
        rx,
        replay,
        // L-2: cleanup hub state when the client stream ends / disconnects.
        Some(Box::new(move || state.hub.cleanup(&run_id_for_cleanup))),
    ))
    .keep_alive(KeepAlive::new().interval(StdDuration::from_secs(15)))
    .into_response()
}

// L-2: runs a callback exactly once when dropped (stream ends / client disconnects).
pub(crate) struct OnEndGuard(Option<Box<dyn Fn() + Send>>);
impl OnEndGuard {
    fn new(cb: Option<Box<dyn Fn() + Send>>) -> Self {
        OnEndGuard(cb)
    }
}
impl Drop for OnEndGuard {
    fn drop(&mut self) {
        if let Some(cb) = self.0.take() {
            cb();
        }
    }
}

pub(crate) fn stream_from_rx(
    run_id: String,
    mut rx: broadcast::Receiver<ChatStreamEvent>,
    replay: Vec<ChatStreamEvent>,
    on_end: Option<Box<dyn Fn() + Send>>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let stream = async_stream::stream! {
        // L-2: ensure hub cleanup runs when the client stream ends / disconnects.
        let _guard = OnEndGuard::new(on_end);
        // F4: Replay persisted events first (reconnect support).
        let mut terminated = false;
        for evt in &replay {
            let json_str = serde_json::to_string(&json!({
                "runId": evt.run_id,
                "type": evt.event_type,
                "delta": evt.delta,
                "message": evt.message,
            }))
            .unwrap_or_default();
            match evt.event_type.as_str() {
                "done" => {
                    terminated = true;
                    yield Ok(Event::default().data(json_str));
                    break;
                }
                "error" => {
                    terminated = true;
                    yield Ok(Event::default().data(json_str));
                    break;
                }
                _ => {
                    yield Ok(Event::default().data(json_str));
                }
            }
        }
        // If replay already hit a terminal event, skip live stream.
        if !terminated {
            loop {
                match rx.recv().await {
                    Ok(evt) => {
                        let json_str = serde_json::to_string(&json!({
                            "runId": evt.run_id,
                            "type": evt.event_type,
                            "delta": evt.delta,
                            "message": evt.message,
                        }))
                        .unwrap_or_default();
                        match evt.event_type.as_str() {
                            "done" => {
                                yield Ok(Event::default().data(json_str));
                                break;
                            }
                            "error" => {
                                yield Ok(Event::default().data(json_str));
                                break;
                            }
                            _ => {
                                yield Ok(Event::default().data(json_str));
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Channel closed (worker finished or all senders dropped).
                        if !terminated {
                            let json_str = serde_json::to_string(&json!({
                                "runId": run_id,
                                "type": "error",
                                "delta": null,
                                "message": "stream ended without terminal event",
                            }))
                            .unwrap_or_default();
                            yield Ok(Event::default().data(json_str));
                        }
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Client is behind — skip missed events and continue.
                        continue;
                    }
                }
            }
        }
    };
    stream
}

pub(crate) async fn stop_turn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<StopPayload>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    // Validate session (F1: ownership check)
    if let Err(e) = state.sessions_tavern.get_for_owner(&session_id, &session.user_id) {
        return map_core_err(e);
    }
    if let Some(job) = state.jobs.get(&body.run_id) {
        if job.workspace_id != session.workspace_id && job.user_id != session.user_id {
            return forbidden("ST_RUN_FORBIDDEN", "run not in your workspace");
        }
    }

    state.hub.cancel(&body.run_id);
    let _ = state.jobs.cancel(&body.run_id);
    // 仅当锁属该 runId（或 pending- 占位）才解锁，绝不无条件清锁，
    // 否则会把另一在用 turn 的锁一并清掉，破坏「并发 turn 拒绝」护栏（409）。
    clear_session_active_run(&state.sessions_tavern, &session_id, Some(&body.run_id));

    // G10 (吞噬 denova D1): 回合停止后写入诊断摘要（turn / accepted=false / duration_ms / llm_ok=false）。
    if let Ok(mut sess) = state.sessions_tavern.get(&session_id) {
        let duration_ms = state
            .jobs
            .get(&body.run_id)
            .and_then(|j| {
                j.updated_at
                    .signed_duration_since(j.created_at)
                    .num_milliseconds()
                    .max(0)
                    .try_into()
                    .ok()
            })
            .unwrap_or(0u64);
        let turn = sess.turn + 1; // 在途回合号（尚未提交完成，turn 在完成时才自增）
        sess.last_turn_diagnostic = Some(kaleido_core::TurnDiagnostic {
            turn,
            accepted: false,
            duration_ms,
            llm_ok: false,
            humanize_total: 0,
            humanize_grade: String::new(),
            humanize_hits: 0,
            voice_drift_score: 0.0,
            voice_drift_char: String::new(),
            voice_drift_reasons: Vec::new(),
            dice_forgery: false,
            dice_contradiction: false,
            dice_reasons: Vec::new(),
            hallu_names: Vec::new(),
            conflicts: Vec::new(),
        });
        tracing::info!(%session_id, run_id = %body.run_id, turn, duration_ms, "st turn stopped (diagnostic recorded)");
        if let Err(e) = state.sessions_tavern.save(sess) {
            tracing::warn!(error = %e, %session_id, "failed to save stop_turn diagnostic");
        }
    }

    Json(json!({
        "ok": true,
        "runId": body.run_id,
        "sessionId": session_id,
    }))
    .into_response()
}

/// S4 (吞噬 denova director_plan): GET 当前导演计划。
/// 返回 `{"plan": DirectorPlan|null}`。
pub(crate) async fn get_director_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&session_id, &session.user_id) {
        Ok(s) => Json(json!({ "plan": s.director_plan })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// G13/G14 (吞噬 denova interactiveDirectorTaskDirectorPlanUpdate): 导演计划后台任务 id。
pub(crate) const DIRECTOR_TASK_DIRECTOR_PLAN_UPDATE: &str = "director_plan_update";

/// G13 (吞噬 denova SubmitDirectorPlanUpdate): 导演 plan 提交请求体。
/// 人工/导演 LLM 均可经 HTTP 提交导演计划（比给 LLM 加工具更贴合现有架构）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorPlanSubmitRequest {
    pub goal: String,
    #[serde(default)]
    pub pressure: Option<String>,
    #[serde(default)]
    pub cost: Option<String>,
    /// 任务书约定 snake_case `hits_beats`；camelCase 也兼容。
    #[serde(default, alias = "hits_beats")]
    pub hits_beats: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// G13 (吞噬 denova SubmitDirectorPlanUpdate): 导演 plan 更新提交。
/// 校验后写回 session.director_plan 并置 last_run=ready（reason 透出为 summary）；
/// 后台生成任务在跑时拒绝提交（避免覆盖竞态）。返回 `{"plan":…}`。
pub(crate) async fn submit_director_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<DirectorPlanSubmitRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let goal = body.goal.trim().to_string();
    if goal.is_empty() {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "director plan goal is required".into(),
        ));
    }
    let mut sess = match state.sessions_tavern.get_for_owner(&session_id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if sess.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    // G13/G14: 后台导演任务在跑时拒绝提交，避免与 LLM 写回竞态。
    if state.director_tasks.is_running(&session_id) {
        return conflict("ST_DIRECTOR_BUSY", "director task running");
    }
    let mut existing = sess.director_plan.clone().unwrap_or_default();
    if existing.created_turn == 0 {
        existing.created_turn = sess.turn;
    }
    existing.goal = goal;
    existing.pressure = body.pressure;
    existing.cost = body.cost;
    existing.hits_beats = body.hits_beats;
    existing.updated_turn = sess.turn;
    // G2: 人工提交 → ready（reason 作为 summary 透出）
    let summary = match body.reason {
        Some(r) if !r.trim().is_empty() => format!("导演提交：{}", r.trim()),
        _ => "导演提交".into(),
    };
    existing.last_run = Some(DirectorPlanRunStatus::ready(summary));
    sess.director_plan = Some(existing.clone());
    sess.director_pending = false;
    sess.director_task = None;
    if let Err(e) = state.sessions_tavern.save(sess) {
        return map_core_err(e);
    }
    Json(json!({ "plan": existing })).into_response()
}

/// Seed demo pack at process start (best-effort).
pub fn bootstrap_demo(packs: &PackStore) {
    match packs.ensure_demo_pack() {
        Ok(p) => tracing::info!(pack_id = %p.id, "story-tavern demo pack ready"),
        Err(e) => tracing::warn!(error = %e, "story-tavern demo pack seed failed"),
    }
}

// ─── S5/S6 演出机读取 + 角色归档（吞噬 denova event_package / actor archive）──

/// G6 (吞噬 denova interactiveDirectorEventCatalog): 事件目录块（纯函数）。
/// 从 pack 收集 enabled 包的 enabled 卡（weight>0；modules.eventPackageIds 非空时过滤只取列出包，
/// 复用 [`kaleido_core::pick_event_card`] 的候选语义但**全量列出**，非抽一张），
/// 格式化为导演计划可引用的「可用事件目录」。一张卡一行：
///     - 包「奇遇包」：外门考核打脸（medium/打脸，冷却2）—— <prompt 前 40 字>…
/// 每包最多列 5 张（超出省略）；无候选卡返回空串（调用方不注入，保持零开销）。
pub(crate) fn build_event_catalog_block(pack: &StoryPack) -> String {
    let allowed = &pack.stage_director.modules.event_package_ids;
    let mut lines: Vec<String> = vec!["可用事件目录（事件卡，导演可按需调度/铺垫）：".into()];
    let mut any = false;
    for pkg in &pack.event_packages {
        if !pkg.enabled {
            continue;
        }
        if !allowed.is_empty() && !allowed.contains(&pkg.id) {
            continue;
        }
        let mut shown = 0usize;
        for card in &pkg.cards {
            if !card.enabled || card.weight == 0 {
                continue;
            }
            if shown >= 5 {
                break;
            }
            let name = if card.type_name.trim().is_empty() {
                card.title.trim().to_string()
            } else {
                card.type_name.trim().to_string()
            };
            let mut meta: Vec<&str> = Vec::new();
            if !card.intensity.trim().is_empty() {
                meta.push(card.intensity.trim());
            }
            if !card.category.trim().is_empty() {
                meta.push(card.category.trim());
            }
            let mut line = format!("- 包「{}」：{}", pkg.name, name);
            if !meta.is_empty() || card.cooldown_turns > 0 {
                let mut parens: Vec<String> = Vec::new();
                if !meta.is_empty() {
                    parens.push(meta.join("/"));
                }
                if card.cooldown_turns > 0 {
                    parens.push(format!("冷却{}", card.cooldown_turns));
                }
                line.push_str(&format!("（{}）", parens.join("，")));
            }
            let trimmed = card.prompt.trim();
            if !trimmed.is_empty() {
                let count = trimmed.chars().count();
                let preview: String = trimmed.chars().take(40).collect();
                line.push_str("—— ");
                line.push_str(&preview);
                if count > 40 {
                    line.push('…');
                }
            }
            lines.push(line);
            shown += 1;
            any = true;
        }
    }
    if !any {
        return String::new();
    }
    lines.join("\n")
}

/// 导演台配置：stage_director + 当前 director_plan + pending 状态。
pub(crate) async fn get_director_config(
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
    let pack = match state.packs.get(&sess.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    Json(json!({
        "stageDirector": pack.stage_director,
        "mainlineExhausted": kaleido_core::is_mainline_exhausted(&pack, &sess),
        "directorPlan": sess.director_plan,
        "directorPending": sess.director_pending,
        // G13/G14: 当前后台导演任务 id（None = 无后台任务在跑）。
        "directorTask": sess.director_task,
        "guardEvents": sess.guard_events.iter().rev().take(20).cloned().collect::<Vec<_>>(),
        // X3: 虾米质检成果（诊断展示用）——速读问题持久化结果；无记录时前端可判空。
        "skimming": {
            "skimIssues": sess.skim_issues,
            "skimSample": sess.skim_sample,
            "lastCheckedTurn": sess.turn,
            // X4: 章节执行合同渲染（诊断展示用；无导演计划/无数据时为 null）。
            "contract": build_director_execution_contract(&sess),
        },
    }))
    .into_response()
}

/// 校验 StageDirectorConfig 枚举字段（G8）。非法值返回错误描述，合法返回 Ok。
/// - mainlineStrength: strong_arc / balanced / soft / soft_guidance（soft 为旧值兼容）
/// - runPolicy.mode: manual / interval / on_demand
/// - failurePolicy: fail_forward / success_at_cost / blocked / hard_failure
/// - pacingCurve: 空 / wave / goal-pressure-payoff / linear
/// - eventFrequency: off / sparse / balanced / frequent
/// - ruleVisibilityMode: audit_only / public_roll
pub(crate) fn validate_stage_director_config(cfg: &StageDirectorConfig) -> Result<(), String> {
    let mainline = cfg.mainline_strength.trim();
    if !mainline.is_empty()
        && !["strong_arc", "balanced", "soft", "soft_guidance"].contains(&mainline)
    {
        return Err(format!("mainlineStrength 非法: {mainline}"));
    }
    let rp = &cfg.run_policy;
    if !["manual", "interval", "on_demand"].contains(&rp.mode.trim()) {
        return Err(format!("runPolicy.mode 非法: {}", rp.mode));
    }
    if !["fail_forward", "success_at_cost", "blocked", "hard_failure"].contains(&rp.failure_policy.trim()) {
        return Err(format!("failurePolicy 非法: {}", rp.failure_policy));
    }
    let pacing = rp.pacing_curve.trim();
    if !pacing.is_empty() && !["wave", "goal-pressure-payoff", "linear"].contains(&pacing) {
        return Err(format!("pacingCurve 非法: {pacing}"));
    }
    if !["off", "sparse", "balanced", "frequent"].contains(&rp.event_frequency.trim()) {
        return Err(format!("eventFrequency 非法: {}", rp.event_frequency));
    }
    if !["audit_only", "public_roll"].contains(&rp.rule_visibility_mode.trim()) {
        return Err(format!("ruleVisibilityMode 非法: {}", rp.rule_visibility_mode));
    }
    Ok(())
}

/// PUT /api/v1/story-tavern/sessions/{id}/director-config
/// 编辑导演策略：body 为完整 StageDirectorConfig（G8 含 failurePolicy/pacingCurve/eventFrequency/ruleVisibilityMode/branchPlanningTurns），
/// 校验枚举后写回 pack.stage_director。
pub(crate) async fn put_director_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<StageDirectorConfig>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if let Err(msg) = validate_stage_director_config(&body) {
        return unprocessable("ST_DIRECTOR_CONFIG", msg);
    }
    let mut pack = match state.packs.get(&sess.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    pack.stage_director = body;
    if let Err(e) = state.packs.save(pack) {
        return map_core_err(e);
    }
    let refreshed = match state.packs.get(&sess.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    Json(json!({
        "ok": true,
        "stageDirector": refreshed.stage_director,
    }))
    .into_response()
}

/// 事件卡包列表（含启用状态），供前端事件卡面板。
pub(crate) async fn get_event_packages(
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
    let pack = match state.packs.get(&sess.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    Json(json!({
        "packId": pack.id,
        "packages": pack.event_packages,
    }))
    .into_response()
}

/// 最近一次触发的事件（S5 event log）。
pub(crate) async fn get_last_event(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => Json(json!({ "lastEvent": s.last_event })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// 角色状态系统（actors + 归档索引）。
pub(crate) async fn get_actor_states(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => Json(json!({ "actorStates": s.actor_states })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// 手动更新角色状态（UI 属性面板编辑）。可改 fields / add_traits / remove_traits；
/// 只更新请求中出现的字段，未提及的保持不变。批量支持（数组）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActorStatesUpdateRequest {
    /// 单个角色更新，或数组批量更新。
    #[serde(flatten)]
    single: Option<ActorStateUpdate>,
    #[serde(default)]
    updates: Vec<ActorStateUpdate>,
}

pub(crate) async fn get_check_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => Json(json!({ "checks": s.check_history })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// [morphling Wave B3 2026-08-16] 章节剧情摘要账本：查看（每章总结列表 + 提炼配置，供 UI 展示）。
pub(crate) async fn get_chapter_summaries(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => {
            let cfg = s.diary_config.clone().unwrap_or_default();
            Json(json!({
                "chapterDiaries": s.chapter_diaries,
                "diaryConfig": { "turnInterval": cfg.turn_interval, "eventThreshold": cfg.event_threshold },
            }))
            .into_response()
        }
        Err(e) => map_core_err(e),
    }
}

/// [morphling Wave B3 2026-08-16] 章节剧情摘要账本：手动修改某章总结
/// （manual_edited=true → 自动提炼不再覆盖；无条目则新建）。
/// 可选携带 diaryConfig（回合数/事件数阈值）一起保存。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChapterDiaryPatch {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    diary_config: Option<kaleido_core::ChapterDiaryConfig>,
}

pub(crate) async fn put_chapter_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, chapter_id)): Path<(String, String)>,
    Json(body): Json<ChapterDiaryPatch>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if let Some(cfg) = body.diary_config {
        sess.diary_config = Some(cfg);
    }
    if !body.summary.is_empty() {
        let summary = body.summary.trim().chars().take(800).collect::<String>();
        match sess.chapter_diaries.iter_mut().find(|d| d.chapter_id == chapter_id) {
            Some(d) => {
                d.summary = summary.clone();
                d.manual_edited = true;
                d.updated_at_turn = sess.turn;
            }
            None => {
                sess.chapter_diaries.push(kaleido_core::ChapterDiaryEntry {
                    chapter_id: chapter_id.clone(),
                    title: String::new(),
                    summary: summary.clone(),
                    start_turn: 0,
                    end_turn: sess.turn,
                    updated_at_turn: sess.turn,
                    manual_edited: true,
                });
            }
        }
    }
    if let Err(e) = state.sessions_tavern.save(sess) {
        return map_core_err(e);
    }
    Json(json!({ "ok": true, "chapterId": chapter_id, "summary": body.summary })).into_response()
}

pub(crate) async fn put_actor_states(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::Json<serde_json::Value>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let mut sess = match state.sessions_tavern.get(&id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    // 兼容三种入参形态（通过 ActorStatesUpdateRequest 统一解析，serde flatten + updates 数组收集）：
    // 1) 单个对象 {characterId, fields, addTraits, removeTraits}
    // 2) 数组 [{...}, {...}]
    // 3) 包装对象 {updates: [{...}, ...]}
    let updates: Vec<ActorStateUpdate> = match body.0 {
        serde_json::Value::Array(arr) => arr
            .into_iter()
            .filter_map(|v| serde_json::from_value::<ActorStateUpdate>(v).ok())
            .collect(),
        other => {
            // 先按 Request 结构解析（收集 updates 数组）；失败再按单个 ActorStateUpdate 解析。
            if let Ok(req) = serde_json::from_value::<ActorStatesUpdateRequest>(other.clone()) {
                let mut v = req.updates;
                if let Some(u) = req.single {
                    if !u.character_id.is_empty() {
                        v.push(u);
                    }
                }
                v
            } else {
                let mut v = Vec::new();
                if let Ok(u) = serde_json::from_value::<ActorStateUpdate>(other) {
                    if !u.character_id.is_empty() {
                        v.push(u);
                    }
                }
                v
            }
        }
    };
    if updates.is_empty() {
        return bad_request("ST_NO_VALID_UPDATES", "no valid updates (need characterId + fields)");
    }
    let changed = sess.actor_states.apply_updates(&updates);
    match state.sessions_tavern.save(sess) {
        Ok(_) => Json(json!({ "ok": true, "updated": changed, "actorStates": state.sessions_tavern.get(&id).map(|s| s.actor_states).unwrap_or_default() }))
            .into_response(),
        Err(e) => map_core_err(e),
    }
}
