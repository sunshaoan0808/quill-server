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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoryAssistantRequest {
    message: String,
    /// 可选：前端本地保存的助手对话历史（[{role, content}]，不含本次 message）。
    #[serde(default)]
    history: Vec<AssistHistoryMsg>,
    /// 冒险/跑团剧情消息（[{role, content}]，最近 N 条，由前端传入）。
    #[serde(default)]
    messages: Vec<AssistHistoryMsg>,
    /// 剧本标题（可选，空则省略）。
    #[serde(default)]
    title: String,
    /// 会话类型: story(冒险/跑团, 默认) / chat(对话模式)。仅影响上下文角色标签。
    #[serde(default)]
    kind: String,
    /// 世界书 ids（冒险/跑团/对话模式：前端从 wb/cc 选择解析传入，用于设定检索）。
    #[serde(default)]
    world_book_ids: Vec<String>,
}

/// 剧情助手（story/冒险/跑团版）：无 TavernSession，剧情上下文来自客户端传入的
/// messages（冒险/跑团是 localStorage 会话，不走 sessions_tavern 存储）。
/// 与 assistant_chat 同语义：独立 LLM 会话、全局视角、绝不代写剧情。
pub(crate) async fn story_assistant_chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<StoryAssistantRequest>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let msg = body.message.trim().to_string();
    if msg.is_empty() {
        return bad_request("ST_EMPTY_MESSAGE", "empty message");
    }
    let mut ctx = String::new();
    if !body.title.trim().is_empty() {
        ctx.push_str(&format!("剧本：{}\n\n", body.title.trim()));
    }
    // 世界书设定检索（wb ids 来自前端 wb/cc 选择；索引未建/embed 失败静默降级）
    if !body.world_book_ids.is_empty() {
        let wctx = wb_vector_ctx(&state, &body.world_book_ids, &msg);
        if !wctx.is_empty() {
            ctx.push_str(&wctx);
        }
    }
    ctx.push_str("【最近剧情】\n");
    for m in body.messages.iter().rev().take(10).rev() {
        let role = if m.role == "user" {
            "你"
        } else if body.kind == "chat" {
            "对方"
        } else {
            "旁白"
        };
        // 前后各取 150 字符（消息长时保留尾部信息）
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
    let mut msgs: Vec<serde_json::Value> = Vec::new();
    msgs.push(json!({"role": "system", "content": ASSISTANT_SYSTEM_PROMPT}));
    for h in body.history.iter().rev().take(12).rev() {
        let role = if h.role == "user" { "user" } else { "assistant" };
        let content = h.content.trim();
        if content.is_empty() {
            continue;
        }
        msgs.push(json!({"role": role, "content": content}));
    }
    msgs.push(json!({"role": "user", "content": user}));
    match crate::llm_stream::stream_chat_completions_msgs(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        msgs,
        0.1,
        8192,
        90,
        |_| true,
    )
    .await
    {
        Ok(reply) => {
            // 面板块同样提取剥离（冒险页无 panels 展示容器，正文只回干净文本）
            let (clean_reply, panels) = split_panels_from_narrative(&reply.trim());
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

/// 剧情助手「全本视野」辅助：章节目录（全本骨架，含章节目标；超长截断）。
pub(crate) fn pack_toc_text(pack: &StoryPack) -> String {
    let mut out = String::from("【章节目录】\n");
    for ch in &pack.chapters {
        out.push_str(&format!("{}. {}", ch.order, ch.title));
        if !ch.goals.is_empty() {
            let g: Vec<&str> = ch.goals.iter().take(2).map(|s| s.as_str()).collect();
            out.push_str(&format!("（目标：{}）", g.join("/")));
        }
        out.push('\n');
        if out.chars().count() > 6000 {
            out.push_str("……（目录过长已截断）\n");
            break;
        }
    }
    out.push('\n');
    out
}

/// 世界线摘要（玩家分支路径：主线/分叉 + 各线存档回合）。
pub(crate) fn format_worldline_text(wv: &kaleido_core::WorldlineView) -> String {
    let mut out = String::from("【世界线】当前分支：");
    out.push_str(&wv.current_worldline_id);
    out.push('\n');
    for line in &wv.lines {
        if line.saves.is_empty() {
            continue;
        }
        let turns: Vec<String> = line.saves.iter().map(|s| s.turn.to_string()).collect();
        out.push_str(&format!(
            "分支 {}：{} 个存档（回合 {}）\n",
            line.id,
            line.saves.len(),
            turns.join("→")
        ));
    }
    out.push('\n');
    out
}

/// pack 正文向量索引 id（VectorIndexStore 按 id 分文件，中文自动 safe 化）。
pub(crate) fn pack_index_id(pack_id: &str) -> String {
    format!("pack-{pack_id}")
}

/// 读 pack 全部章节正文，按 ~500 字符切块（不重叠）。返回 (uid, 文本)。
pub(crate) fn pack_body_blocks(pack: &StoryPack, pack_root: &std::path::Path) -> Vec<(String, String)> {
    const CHUNK: usize = 500;
    let mut blocks = Vec::new();
    for ch in &pack.chapters {
        if ch.body_path.is_empty() {
            continue;
        }
        let path = pack_root.join(&ch.body_path);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = raw.trim();
        if text.is_empty() {
            continue;
        }
        let chars: Vec<char> = text.chars().collect();
        for (i, chunk) in chars.chunks(CHUNK).enumerate() {
            let block: String = chunk.iter().collect();
            blocks.push((format!("ch{}-{}", ch.order, i), block));
        }
    }
    blocks
}

/// 懒加载建 pack 正文向量索引（已有索引直接跳过；embed 不可用/无正文静默返回 Err）。
pub(crate) fn ensure_pack_vector_index(state: &AppState, pack: &StoryPack) -> Result<(), String> {
    let index_id = pack_index_id(&pack.id);
    let existing = state.vector_index.load(&index_id);
    if !existing.entries.is_empty() {
        return Ok(());
    }
    if !crate::embed_local::inline_enabled() {
        return Err("embed not enabled".into());
    }
    crate::embed_local::ensure_local().map_err(|e| e)?;
    let pack_root = state.packs.pack_dir(&pack.id).map_err(|e| e.to_string())?;
    let blocks = pack_body_blocks(pack, &pack_root);
    if blocks.is_empty() {
        return Err("no chapter bodies".into());
    }
    let texts: Vec<String> = blocks.iter().map(|(_, t)| t.clone()).collect();
    let vecs = crate::embed_local::embed_many(&texts).map_err(|e| e)?;
    let entries: Vec<kaleido_core::VectorIndexEntry> = blocks
        .iter()
        .zip(vecs.iter())
        .map(|((uid, text), v)| kaleido_core::VectorIndexEntry {
            uid: uid.clone(),
            world: index_id.clone(),
            text: text.clone(),
            text_hash: String::new(),
            vector: v.clone(),
        })
        .collect();
    let file = kaleido_core::VectorIndexFile {
        world_book_id: index_id.clone(),
        model: crate::embed_local::MODEL_NAME.into(),
        dim: vecs.first().map(|v| v.len()).unwrap_or(0),
        entries,
        updated_at: None,
    };
    state
        .vector_index
        .save(file)
        .map_err(|e| e.to_string())?;
    tracing::info!(pack = %pack.id, blocks = blocks.len(), "pack vector index built");
    Ok(())
}

/// 全书向量检索：问题 embed → pack 索引 top 3 命中块 → 注入助手上下文。
/// 索引未建则懒建（首次可能慢几秒）；embed/检索失败静默降级（不阻塞助手）。
pub(crate) fn pack_vector_ctx(state: &AppState, pack: &StoryPack, query: &str) -> String {
    let query = query.trim();
    if query.is_empty() {
        return String::new();
    }
    if let Err(e) = ensure_pack_vector_index(state, pack) {
        tracing::debug!(pack = %pack.id, err = %e, "pack vector ctx skipped");
        return String::new();
    }
    let index_id = pack_index_id(&pack.id);
    let idx = state.vector_index.load(&index_id);
    if idx.entries.is_empty() {
        return String::new();
    }
    let qv = match crate::embed_local::embed_one(query) {
        Ok(v) => v,
        Err(_) => return String::new(),
    };
    // 正文长块检索：阈值比世界书（0.42）大幅放宽——泛化问题（"前三章讲什么"）
    // embed 后与叙事块相似度偏低，0.35 会空命中（实测 Q3 教训）
    let settings = kaleido_core::VectorActivationSettings {
        enabled: true,
        score_threshold: 0.28,
        top_k: 4,
    };
    let hits = if kaleido_core::vector_model_mismatch(&idx.model, crate::embed_local::MODEL_NAME) {
        tracing::warn!(index_model = %idx.model, pack = %pack.id, "pack vector ctx: index model mismatch, skip");
        Vec::new()
    } else {
        kaleido_core::rank_hits(&idx, crate::embed_local::MODEL_NAME, &qv, &settings)
    };
    if hits.is_empty() {
        return String::new();
    }
    let mut out = String::from("【全书检索命中】\n");
    for h in hits {
        if let Some(e) = idx.entries.iter().find(|e| e.uid == h.uid) {
            let text: String = e.text.chars().take(400).collect();
            out.push_str(&format!("[{}]\n{}\n\n", e.uid, text));
        }
    }
    out
}

/// 手动重建 pack 正文向量索引（正文改动后调用；删除旧索引强制重算）。
pub(crate) async fn rebuild_pack_vector_index(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let pack = match state.packs.get(&id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let index_id = pack_index_id(&id);
    let _ = state.vector_index.delete(&index_id);
    match ensure_pack_vector_index(&state, &pack) {
        Ok(()) => {
            let idx = state.vector_index.load(&index_id);
            Json(json!({ "ok": true, "entryCount": idx.entries.len() })).into_response()
        }
        Err(e) => Json(json!({ "ok": false, "error": e })).into_response(),
    }
}

/// 世界书设定检索（story/冒险/跑团/对话版）：复用 W5 collect_vector_hits，
/// 命中条目按 uid 回索引取 text 注入。索引未建/embed 失败静默降级。
pub(crate) fn wb_vector_ctx(state: &AppState, wb_ids: &[String], query: &str) -> String {
    let query = query.trim();
    if query.is_empty() || wb_ids.is_empty() {
        return String::new();
    }
    // 世界书条目短：阈值用 0.35（介于世界书默认 0.42 与 pack 正文 0.28 之间）
    let settings = kaleido_core::VectorActivationSettings {
        enabled: true,
        score_threshold: 0.35,
        top_k: 4,
    };
    let (hits, verr) = crate::collect_vector_hits(state, wb_ids, query, &settings);
    if let Some(e) = verr {
        tracing::debug!(err = %e, "wb vector ctx skipped");
        return String::new();
    }
    if hits.is_empty() {
        return String::new();
    }
    let mut out = String::from("【设定检索命中】\n");
    for h in hits {
        for wid in wb_ids {
            let idx = state.vector_index.load(wid);
            if let Some(e) = idx.entries.iter().find(|e| e.uid == h.uid) {
                let text: String = e.text.chars().take(400).collect();
                out.push_str(&format!("[{}]\n{}\n\n", e.uid, text));
                break;
            }
        }
    }
    out
}

pub(crate) async fn create_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<CreateSaveRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // F1: ownership check.
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    match state.sessions_tavern.create_save(&id, body.label) {
        Ok(s) => Json(s).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn delete_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, save_id)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // F1: ownership check.
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    match state.sessions_tavern.delete_save(&id, &save_id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn restore_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, save_id)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // F1: ownership check.
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    // Map "turn in progress" to 409
    match state.sessions_tavern.restore_save(&id, &save_id) {
        Ok(s) => Json(s).into_response(),
        Err(kaleido_core::CoreError::BadRequest(msg)) if msg.contains("turn in progress") => return conflict("ST_CONFLICT", msg),
        Err(e) => map_core_err(e),
    }
}

/// [跨会话分叉] 存档分叉到新会话（旧会话不动，新会话独立跑新剧情）。
pub(crate) async fn fork_save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, save_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // ownership: source session must belong to caller
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    let label = body.get("label").and_then(|v| v.as_str()).map(|s| s.to_string());
    match state.sessions_tavern.fork_save_to_session(&id, &save_id, label) {
        Ok(s) => Json(json!({"ok": true, "session": s})).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// [吸收 DEEIX service_fork.go] 从任意消息分叉：POST body {messageIndex, label?}
pub(crate) async fn fork_at_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // ownership: source session must belong to caller
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    let idx = match body.get("messageIndex").and_then(|v| v.as_u64()) {
        Some(n) => n as usize,
        None => return map_core_err(kaleido_core::CoreError::BadRequest("messageIndex required".into())),
    };
    let label = body.get("label").and_then(|v| v.as_str()).map(|s| s.to_string());
    match state.sessions_tavern.fork_at_message(&id, idx, label) {
        Ok(s) => Json(json!({"ok": true, "session": s})).into_response(),
        Err(e) => map_core_err(e),
    }
}

// ─── P0-1 story_command（/rewind N 回退、/reroll 重生成）────────────────────

/// 解析 "/rewind" / "/rewind 3" / "/rewind3"；无数字或缺省 → 1。
pub(crate) fn parse_rewind_steps(msg: &str) -> Option<usize> {
    if !msg.starts_with("/rewind") {
        return None;
    }
    let rest = msg.trim_start_matches("/rewind").trim_start();
    if rest.is_empty() {
        return Some(1);
    }
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        return Some(1);
    }
    Some(digits.parse::<usize>().unwrap_or(1).max(1))
}

/// 共享回退逻辑：校验会话/进行中回合 → restore_checkpoint(steps) → save。
/// 返回 (turn, 实际回退步数)。
pub(crate) async fn do_rewind(
    state: &AppState,
    id: &str,
    steps: usize,
) -> kaleido_core::CoreResult<(u32, usize)> {
    let mut sess = state.sessions_tavern.get(id)?;
    if sess.pack_missing {
        return Err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    if sess.active_run_id.is_some() {
        return Err(kaleido_core::CoreError::BadRequest(
            "turn in progress; stop or wait before rewind".into(),
        ));
    }
    let rewound = sess.restore_checkpoint(steps)?;
    let turn = sess.turn;
    if rewound > 0 {
        let _ = state.sessions_tavern.save(sess)?;
    }
    Ok((turn, rewound))
}

/// 共享重生成逻辑：先取当前最后一条 user 消息，再回退 1 回合，save。
/// 返回 (turn, lastUserMessage)。
pub(crate) async fn do_reroll(state: &AppState, id: &str) -> kaleido_core::CoreResult<(u32, String)> {
    let mut sess = state.sessions_tavern.get(id)?;
    if sess.pack_missing {
        return Err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    if sess.active_run_id.is_some() {
        return Err(kaleido_core::CoreError::BadRequest(
            "turn in progress; stop or wait before reroll".into(),
        ));
    }
    // [Swipe 多备选] 旧正文不丢：最后一条 assistant 内容暂存 pending_swipes，下条消息继承
    if let Some(last_asst) = sess.messages.iter().rev().find(|m| m.role == "assistant") {
        if !last_asst.content.is_empty() {
            // 去重：已在 swipes/pending 里的不重复存
            let already = last_asst.swipes.iter().any(|s| s == &last_asst.content) || sess.pending_swipes.iter().any(|s| s == &last_asst.content);
            if !already {
                sess.pending_swipes.push(last_asst.content.clone());
                // cap 10
                if sess.pending_swipes.len() > 10 { sess.pending_swipes.remove(0); }
            }
        }
    }
    let last_user = sess
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let rewound = sess.restore_checkpoint(1)?;
    if rewound == 0 {
        return Err(kaleido_core::CoreError::BadRequest("no checkpoint".into()));
    }
    let turn = sess.turn;
    let _ = state.sessions_tavern.save(sess)?;
    Ok((turn, last_user))
}

// ─── P0-2/P0-3 story_command（/config 配置生效、/remember /event 记忆生效）──────

pub(crate) const SCENE_SUMMARY_MAX: usize = 500;
pub(crate) const EVENT_SUMMARY_MAX: usize = 500;

/// /config key=value —— 白名单配置写入 sess.player.flags（保留已有 keys），保存后生效。
pub(crate) async fn handle_config_cmd(state: &AppState, id: &str, msg: &str) -> Response {
    let rest = msg.trim_start_matches("/config").trim();
    if rest.is_empty() {
        return Json(json!({
            "reply": "用法：/config key=value（可选 strict_mode_boost(0-1) / pacing(0-1) / style_guidance(短句)）",
            "action": "error",
        }))
        .into_response();
    }
    let pairs = match parse_config_args(rest) {
        Some(p) => p,
        None => {
            return Json(json!({
                "reply": "参数不合法：key 须为 strict_mode_boost / pacing / style_guidance，数值须在 0~1，style_guidance 为短句",
                "action": "error",
            }))
            .into_response();
        }
    };
    let mut sess = match state.sessions_tavern.get(id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    // [fix 2026-08-15 文风外挂] /config style_source=<pack标题> 读取指定 pack 蒸馏出的
    // narrative_style，合成 style_guidance 注入回合（用户场景：宿醉 18+ 清淡 →
    // 外挂智取美母的浓烈 18+ 描写风格）。外挂覆盖 tone/style/prose_guidance；
    // style_guidance 显式设置时优先级最高（覆盖外挂）。
    let mut has_style_source = false;
    for (k, v) in pairs.iter() {
        if k == "style_source" {
            has_style_source = true;
            if let Some(sv) = v.as_str() {
                match resolve_external_style(&state, sv) {
                    Ok(guidance) => {
                        sess.player.flags
                            .as_object_mut()
                            .map(|m| m.insert("style_guidance".into(), json!(guidance)));
                    }
                    Err(e) => {
                        return Json(json!({
                            "reply": format!("外挂文风失败：{e}（可用 pack 见 /packs）"),
                            "action": "error",
                        }))
                        .into_response();
                    }
                }
            }
        }
    }
    if has_style_source {
        // 外挂已写入 style_guidance；跳过 apply_config_flags 避免 style_source 本身落 flags
        if let Err(e) = state.sessions_tavern.save(sess) {
            return map_core_err(e);
        }
        let applied = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Json(json!({
            "reply": format!("已生效：{applied}（已外挂对应作品文风）"),
            "action": "config",
            "keys": pairs.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
        }))
        .into_response();
    }
    apply_config_flags(&mut sess, &pairs);
    // [fix 2026-08-15 叙述视角] pov=default → 从 flags 移除 pov，回退蒸馏文风人称
    if pairs.iter().any(|(k, v)| k == "pov" && v.as_str() == Some("default")) {
        if let Some(map) = sess.player.flags.as_object_mut() {
            map.remove("pov");
        }
    }
    if let Err(e) = state.sessions_tavern.save(sess) {
        return map_core_err(e);
    }
    let applied = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(", ");
    Json(json!({
        "reply": format!("已生效：{applied}"),
        "action": "config",
        "keys": pairs.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
    }))
    .into_response()
}

/// /remember 场景摘要：<text> —— 更新 L1 场景摘要（限 500 字），保存后生效。
pub(crate) async fn handle_remember_cmd(state: &AppState, id: &str, msg: &str) -> Response {
    let rest = msg.trim_start_matches("/remember").trim();
    let text = if let Some(stripped) = rest.strip_prefix("场景摘要：") {
        stripped.trim()
    } else if let Some(stripped) = rest.strip_prefix("场景摘要:") {
        stripped.trim()
    } else {
        rest
    };
    if text.is_empty() {
        return Json(json!({
            "reply": "用法：/remember 场景摘要：<场景摘要文本>",
            "action": "error",
        }))
        .into_response();
    }
    if text.chars().count() > SCENE_SUMMARY_MAX {
        return Json(json!({
            "reply": format!("场景摘要过长（最多 {} 字）", SCENE_SUMMARY_MAX),
            "action": "error",
        }))
        .into_response();
    }
    let mut sess = match state.sessions_tavern.get(id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    sess.memory_l1.scene_summary = text.to_string();
    sess.memory_l1.updated_at_turn = sess.turn;
    if let Err(e) = state.sessions_tavern.save(sess) {
        return map_core_err(e);
    }
    Json(json!({
        "reply": "已生效：场景摘要已更新",
        "action": "remember",
    }))
    .into_response()
}

/// /event add <简述> / /event drop <id> —— 维护 L2 事件账本，保存后生效。
pub(crate) async fn handle_event_cmd(state: &AppState, id: &str, msg: &str) -> Response {
    let rest = msg.trim_start_matches("/event").trim();
    if rest == "add" || rest.starts_with("add ") {
        let brief = rest["add".len()..].trim();
        if brief.is_empty() {
            return Json(json!({
                "reply": "用法：/event add <事件简述>",
                "action": "error",
            }))
            .into_response();
        }
        if brief.chars().count() > EVENT_SUMMARY_MAX {
            return Json(json!({
                "reply": format!("事件简述过长（最多 {} 字）", EVENT_SUMMARY_MAX),
                "action": "error",
            }))
            .into_response();
        }
        let mut sess = match state.sessions_tavern.get(id) {
            Ok(s) => s,
            Err(e) => return map_core_err(e),
        };
        sess.memory_l2.events.push(kaleido_core::MemoryL2Event {
            id: format!("ev-{}", Uuid::new_v4()),
            turn: sess.turn,
            kind: "event".into(),
            summary: brief.to_string(),
            actors: vec![],
            node_id: sess.node_id.clone(),
            embedding: vec![],
        });
        sess.memory_l2.updated_at_turn = sess.turn;
        if let Err(e) = state.sessions_tavern.save(sess) {
            return map_core_err(e);
        }
        Json(json!({
            "reply": "已生效：事件已记录",
            "action": "event_add",
        }))
        .into_response()
    } else if rest == "drop" || rest.starts_with("drop ") {
        let eid = rest["drop".len()..].trim();
        if eid.is_empty() {
            return Json(json!({
                "reply": "用法：/event drop <事件id>",
                "action": "error",
            }))
            .into_response();
        }
        let mut sess = match state.sessions_tavern.get(id) {
            Ok(s) => s,
            Err(e) => return map_core_err(e),
        };
        let before = sess.memory_l2.events.len();
        sess.memory_l2.events.retain(|e| e.id != eid);
        if sess.memory_l2.events.len() == before {
            return Json(json!({
                "reply": format!("未找到事件：{eid}"),
                "action": "error",
            }))
            .into_response();
        }
        sess.memory_l2.updated_at_turn = sess.turn;
        if let Err(e) = state.sessions_tavern.save(sess) {
            return map_core_err(e);
        }
        Json(json!({
            "reply": "已生效：事件已删除",
            "action": "event_drop",
        }))
        .into_response()
    } else {
        Json(json!({
            "reply": "用法：/event add <简述> 或 /event drop <id>",
            "action": "error",
        }))
        .into_response()
    }
}

/// /time 指令 —— 显式推进游戏时钟（正序约束）。
/// 用法：/time 深夜 /time 次日清晨 /time 三天后
pub(crate) async fn handle_time_cmd(state: &AppState, id: &str, msg: &str) -> Response {
    let rest = msg.trim_start_matches("/time").trim();
    if rest.is_empty() {
        let s = match state.sessions_tavern.get(id) {
            Ok(s) => s,
            Err(e) => return map_core_err(e),
        };
        return Json(json!({
            "reply": format!("当前：{}（用 /time <时段|次日|N天后> 推进）", s.game_clock.state_line()),
            "action": "time_status",
            "gameClock": s.game_clock,
        }))
        .into_response();
    }
    let mut sess = match state.sessions_tavern.get(id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    match sess.game_clock.jump(rest, sess.turn) {
        Ok((_, desc)) => {
            if let Err(e) = state.sessions_tavern.save(sess.clone()) {
                return map_core_err(e);
            }
            Json(json!({
                "reply": format!("已推进 → {desc}"),
                "action": "time_jump",
                "gameClock": sess.game_clock,
            }))
            .into_response()
        }
        Err(e) => Json(json!({
            "reply": format!("{e}"),
            "action": "error",
        }))
        .into_response(),
    }
}

/// /weather 指令 —— 显式改天气。**用户指令第一原则**：直接设置（允许跳变），
/// 不再强制邻接渐进（此前晴→暴雨需逐级 5 次，用户指定被拒）。
/// 用法：/weather 小雨 /weather 暴雨
pub(crate) async fn handle_weather_cmd(state: &AppState, id: &str, msg: &str) -> Response {
    let rest = msg.trim_start_matches("/weather").trim();
    if rest.is_empty() {
        let s = match state.sessions_tavern.get(id) {
            Ok(s) => s,
            Err(e) => return map_core_err(e),
        };
        return Json(json!({
            "reply": format!("当前天气：{}（用 /weather <{}> 改，可直接设置任意天气）", s.game_clock.weather, "晴/多云/阴/小雨/大雨/暴雨/雨雪/雪/大雪/雾/大风"),
            "action": "weather_status",
            "gameClock": s.game_clock,
        }))
        .into_response();
    }
    let mut sess = match state.sessions_tavern.get(id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    match sess.game_clock.force_weather(rest) {
        Ok(w) => {
            if let Err(e) = state.sessions_tavern.save(sess.clone()) {
                return map_core_err(e);
            }
            Json(json!({
                "reply": format!("天气已改 → {w}"),
                "action": "weather_set",
                "gameClock": sess.game_clock,
            }))
            .into_response()
        }
        Err(e) => Json(json!({
            "reply": format!("{e}"),
            "action": "error",
        }))
        .into_response(),
    }
}

/// /season 指令 —— 显式强制季节（剧情设定/推进季节用）。
/// 用法：/season 冬 /season 夏天
pub(crate) async fn handle_season_cmd(state: &AppState, id: &str, msg: &str) -> Response {
    let rest = msg.trim_start_matches("/season").trim();
    if rest.is_empty() {
        let s = match state.sessions_tavern.get(id) {
            Ok(s) => s,
            Err(e) => return map_core_err(e),
        };
        return Json(json!({
            "reply": format!("当前季节：{}（用 /season <春|夏|秋|冬> 强制设置）", s.game_clock.season()),
            "action": "season_status",
            "gameClock": s.game_clock,
        }))
        .into_response();
    }
    let mut sess = match state.sessions_tavern.get(id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    match sess.game_clock.set_season(rest) {
        Ok(se) => {
            if let Err(e) = state.sessions_tavern.save(sess.clone()) {
                return map_core_err(e);
            }
            Json(json!({
                "reply": format!("季节已设为 → {se}（{}）", sess.game_clock.state_line()),
                "action": "season_set",
                "gameClock": sess.game_clock,
            }))
            .into_response()
        }
        Err(e) => Json(json!({
            "reply": format!("{e}"),
            "action": "error",
        }))
        .into_response(),
    }
}

pub(crate) async fn rewind_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RewindRequest>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let steps = body.steps.unwrap_or(1).max(1);
    match do_rewind(&state, &id, steps).await {
        Ok((turn, rewound)) => {
            let remaining = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
                Ok(s) => s.checkpoints.len().saturating_sub(1),
                Err(_) => 0,
            };
            Json(json!({
                "ok": true,
                "rewound": rewound,
                "turn": turn,
                "remaining": remaining,
            }))
            .into_response()
        }
        Err(kaleido_core::CoreError::BadRequest(msg)) if msg.contains("turn in progress") => return conflict("ST_CONFLICT", msg),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn reroll_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match do_reroll(&state, &id).await {
        Ok((turn, last_user)) => {
            Json(json!({
                "ok": true,
                "lastUserMessage": last_user,
                "turn": turn,
            }))
            .into_response()
        }
        Err(kaleido_core::CoreError::BadRequest(msg)) if msg.contains("turn in progress") => return conflict("ST_CONFLICT", msg),
        Err(e) => map_core_err(e),
    }
}

// ─── Message management (ST 核心交互：删除/编辑消息) ─────────────────────────

/// DELETE /api/v1/story-tavern/sessions/{id}/messages/{mid}
/// 删除单条消息。返回更新后的 session。
pub(crate) async fn delete_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, mid)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let result = (|| -> kaleido_core::CoreResult<TavernSession> {
        let mut s = state.sessions_tavern.get_for_owner(&id, &session.user_id)?;
        let before = s.messages.len();
        s.messages.retain(|m| m.id != mid);
        if s.messages.len() == before {
            return Err(kaleido_core::CoreError::NotFound(format!("message not found: {mid}")));
        }
        state.sessions_tavern.save(s.clone())?;
        Ok(s)
    })();
    match result {
        Ok(s) => Json(json!({ "ok": true, "session": s })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// PUT /api/v1/story-tavern/sessions/{id}/messages/{mid}
/// 编辑单条消息正文。body: { "content": "..." }。返回更新后的 session。
pub(crate) async fn edit_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, mid)): Path<(String, String)>,
    body: axum::Json<serde_json::Value>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let new_content = body
        .get("content")
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    if new_content.trim().is_empty() {
        return bad_request("ST_CONTENT_REQUIRED", "content required");
    }
    let result = (|| -> kaleido_core::CoreResult<TavernSession> {
        let mut s = state.sessions_tavern.get_for_owner(&id, &session.user_id)?;
        let mut found = false;
        for m in s.messages.iter_mut() {
            if m.id == mid {
                m.content = new_content.clone();
                found = true;
                break;
            }
        }
        if !found {
            return Err(kaleido_core::CoreError::NotFound(format!("message not found: {mid}")));
        }
        state.sessions_tavern.save(s.clone())?;
        Ok(s)
    })();
    match result {
        Ok(s) => Json(json!({ "ok": true, "session": s })).into_response(),
        Err(e) => map_core_err(e),
    }
}

// ─── Persona handlers ────────────────────────────────────────────────────────

pub(crate) async fn get_persona(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(character_id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.personas.get(&character_id) {
        Ok(p) => Json(p).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn save_persona(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(character_id): Path<String>,
    Json(mut persona): Json<TavernPersona>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    if persona.character_id.is_empty() {
        persona.character_id = character_id;
    } else if persona.character_id != character_id {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "path characterId mismatch".into(),
        ));
    }
    match state.personas.save(persona) {
        Ok(p) => Json(p).into_response(),
        Err(e) => map_core_err(e),
    }
}

// ─── T2: 创作罗盘（author_intent + current_focus）──────────────────────────

pub(crate) async fn get_compass(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(work_id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let store = CompassStore::new(state.auth.data_root().clone());
    match store.load(&work_id) {
        Ok(c) => Json(c).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn put_compass(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(work_id): Path<String>,
    Json(compass): Json<Compass>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let store = CompassStore::new(state.auth.data_root().clone());
    let saved = match store.save(&work_id, &compass) {
        Ok(c) => c,
        Err(e) => return map_core_err(e),
    };
    // 挂载到当前全部 TavernSession 的系统状态，令注入下回合即生效。
    // 单用户本地；未来若引入 session→work 归属，可只更新匹配 work 的会话。
    let mut sessions_mounted = 0usize;
    if let Ok(list) = state.sessions_tavern.list() {
        for row in list {
            let Some(sid) = row.get("sessionId").and_then(|v| v.as_str()) else {
                continue;
            };
            if let Ok(mut sess) = state.sessions_tavern.get(sid) {
                sess.actor_states.mount_compass(saved.clone());
                if state.sessions_tavern.save(sess).is_ok() {
                    sessions_mounted += 1;
                }
            }
        }
    }
    Json(json!({
        "version": saved.version,
        "authorIntent": saved.author_intent,
        "currentFocus": saved.current_focus,
        "sessionsMounted": sessions_mounted,
    }))
    .into_response()
}

// ─── ST-1: Turn handlers ────────────────────────────────────────────────────


/// Filter lore entries matching current chapter/node context.
/// Entries need: permanent=true, or chapterRange contains chapter_cursor, or nodeIds includes node_id.
/// T 层 (2026-08-19): 解析章节 id 数值（"ch06" → 6）；解析失败返回 0（保守，不参与过滤）。
pub(crate) fn worldline_chapter_num(ch: &str) -> i64 {
    ch.trim()
        .strip_prefix("ch")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0)
}

/// T 层 (2026-08-19): 构建「原著时间线」注入块——从 pack.worldline（旁挂 worldline.json
/// 蒸馏产物）取 chapter <= 当前章的事件，按章升序格式化。只注入「已推进」的历史事件，
/// 防后期事件剧透/首回合劫持（D2 同源）；无章节标注（解析失败）保守不注入。旧 pack / 未蒸馏
/// worldline 为空 → 返回 None 零注入。
pub(crate) fn build_worldline_block(pack: &StoryPack, chapter_cursor: &str) -> Option<String> {
    if pack.worldline.is_empty() {
        return None;
    }
    let cur = worldline_chapter_num(chapter_cursor);
    if cur <= 0 {
        return None; // 无当前章进度时不注入（保守）
    }
    let mut shown: Vec<&Value> = Vec::new();
    for ev in &pack.worldline {
        let ch = ev.get("chapter").and_then(|v| v.as_str()).unwrap_or("");
        let chn = worldline_chapter_num(ch);
        if chn > 0 && chn <= cur {
            shown.push(ev);
        }
    }
    if shown.is_empty() {
        return None;
    }
    shown.sort_by_key(|ev| worldline_chapter_num(ev.get("chapter").and_then(|v| v.as_str()).unwrap_or("")));
    let mut out = String::from("\n## 原著时间线（已推进部分）");
    for ev in shown.iter().take(20) {
        let event = ev.get("event").and_then(|v| v.as_str()).unwrap_or("");
        if event.trim().is_empty() {
            continue;
        }
        let tp = ev.get("time_point").and_then(|v| v.as_str()).unwrap_or("");
        let ch = ev.get("chapter").and_then(|v| v.as_str()).unwrap_or("");
        let imp = ev.get("importance").and_then(|v| v.as_str()).unwrap_or("");
        out.push_str(&format!(
            "\n- {ch}·{tp} [{}] {event}",
            if imp.is_empty() { "–" } else { imp }
        ));
    }
    out.push_str("\n以上为原著已发生剧情的时间线锚点，用于保持剧情勾稽；当前进度之后的情节尚未发生，不得提前使用。");
    if out.trim_end_matches(|c: char| c != '】' && c != '\n').ends_with("原著时间线（已推进部分）") {
        return None; // 无有效事件行（全空摘要）
    }
    Some(out)
}

/// Hook 条件触发（吞噬 loreweaver hook_runtime 口径）：entry 可带 trigger 条件，
/// 任一不满足即跳过（与 sticky/cooldown 时间维正交，这是条件维）。
/// triggerPresent: 需在场角色 id/名其一；triggerItem: 需任一口袋持有物品名其一；
/// triggerBondMin: {cid: 分数} 需羁绊分达标其一（bond 分）。
pub(crate) fn lore_trigger_ok(entry: &Value, session: &kaleido_core::TavernSession) -> bool {
    // triggerPresent
    if let Some(arr) = entry.get("triggerPresent").and_then(|v| v.as_array()) {
        if arr.is_empty() { return true; }
        let present: std::collections::HashSet<&str> = session.present_character_ids.iter().map(|s| s.as_str()).collect();
        let hit = arr.iter().filter_map(|v| v.as_str()).any(|w| present.contains(w));
        if !hit { return false; }
    }
    // triggerItem（任一口袋持有其一）
    if let Some(arr) = entry.get("triggerItem").and_then(|v| v.as_array()) {
        if arr.is_empty() { return true; }
        let wants: Vec<String> = arr.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect();
        let mut hit = false;
        for pk in session.pockets.values() {
            for it in pk.carrying.iter().chain(pk.worn.iter()) {
                if wants.iter().any(|w| kaleido_core::pockets::same_item(&it.name, w)) { hit = true; break; }
            }
            if hit { break; }
        }
        if !hit { return false; }
    }
    // triggerBondMin {cid: min} 任一达标
    if let Some(obj) = entry.get("triggerBondMin").and_then(|v| v.as_object()) {
        if obj.is_empty() { return true; }
        let mut hit = false;
        for (k, vv) in obj {
            let min = vv.as_i64().unwrap_or(0) as i32;
            if let Some(b) = session.relationships.get(k) {
                if b.score >= min { hit = true; break; }
            }
        }
        if !hit { return false; }
    }
    true
}
pub(crate) fn filter_lore_entries<'a>(entries: &'a [Value], chapter_cursor: &str, node_id: &str) -> Vec<&'a Value> {
    let mut out = Vec::new();
    for entry in entries {
        let perm = entry.get("permanent").and_then(|v| v.as_bool()).unwrap_or(false);
        if perm {
            out.push(entry);
            continue;
        }
        let range = entry.get("chapterRange").and_then(|v| v.as_str()).unwrap_or("");
        let node_ids = entry.get("nodeIds").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>()).unwrap_or_default();
        let mut matched = false;
        if !range.is_empty() {
            // "ch01-ch02" style range check
            let ch_label = chapter_cursor.trim().to_lowercase();
            let parts: Vec<&str> = range.splitn(2, '-').map(|s| s.trim()).collect();
            if parts.len() == 1 {
                matched = ch_label == parts[0].to_lowercase();
            } else if parts.len() >= 2 {
                matched = ch_label >= parts[0].to_lowercase() && ch_label <= parts[1].to_lowercase();
            }
        }
        if !matched && !node_ids.is_empty() {
            matched = node_ids.iter().any(|nid| *nid == node_id);
        }
        if matched {
            out.push(entry);
        }
    }
    out
}

/// P0 闭环: 预载伏笔注入块（调用点调用，保持 build_tavern_system_prompt 纯函数）。
/// 取 weight 前 15 条 active/planted 伏笔，总长 ≤1200 字符，每条 ≤120 字符。
/// 已回收（recalled）的不注入 —— 回收后下一轮自动从 prompt 消失（可感知闭环）。
pub(crate) fn preload_foreshadow_block(state: &AppState, pack: &StoryPack, session: &TavernSession) -> Option<String> {
    use kaleido_core::foreshadow_store::ForeshadowStore;
    // work_id 解析: pack.source.refs 中 36 字符 uuid 形态的条目即 work 关联
    // （pack 由作者区 work 生成时写入），否则用 pack_id 兜底（fail-open，查不到即 None）。
    let work_id: String = pack
        .source
        .refs
        .iter()
        .find(|r| r.len() == 36 && r.chars().filter(|c| *c == '-').count() == 4)
        .cloned()
        .unwrap_or_else(|| session.pack_id.clone());
    let fs = ForeshadowStore::open(&state.auth.data_root().root().join("plot.sqlite")).ok()?;
    let mut fores: Vec<kaleido_core::foreshadow_store::Foreshadow> = Vec::new();
    for status in ["planted", "active"] {
        if let Ok(mut v) = fs.list_foreshadows(&work_id, Some(status), None) {
            fores.append(&mut v);
        }
    }
    if fores.is_empty() {
        return None;
    }
    fores.sort_by(|a, b| b.weight.cmp(&a.weight));
    let mut lines: Vec<String> = Vec::new();
    let mut budget = 0usize;
    for f in fores.iter().take(15) {
        let desc: String = f.description.chars().take(120).collect();
        let line = format!("- [{}]: {}", f.title, desc);
        budget += line.chars().count();
        if budget > 1200 {
            break;
        }
        lines.push(line);
    }
    if lines.is_empty() {
        return None;
    }
    Some(format!(
        "## 伏笔（作者预埋，未到揭示时机请勿提前揭晓；回收后自然消失）\n{}",
        lines.join("\n")
    ))
}
