//! dual_agent/handlers — [2026-10-05 语义拆分 PH-B] 由 dual_agent/mod.rs 按设计文档
//! docs/plans/2026-10-05-p006-dual-agent-semantic-split.md §2.2 机械搬迁（字节保真）。

// 头部 use 块为拆分桥：符号经 `use super::*`（mod.rs 的 pub(crate) use 汇总）供给，
// 本文件不直接引用属预期；cargo fix 会误删（工程纪律见 docs/REFACTOR_20260927.md）。
#![allow(unused_imports)]

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use kaleido_core::LlmRuntime;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

use crate::llm_stream::{chat_completion_dispatch, extract_json_value};
use crate::harness_bridge::{auto_refine_gate, run_refine, LlmClientImpl};
use crate::{session_from, AppState};
use crate::error_codes::*;



use super::*;

pub(crate) async fn list_sessions_h(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match list_sessions(&state) {
        Ok(mut list) => {
            list.retain(|s| s.workspace_id == session.workspace_id || s.workspace_id.is_empty());
            list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            let states: Vec<Value> = list.iter().map(session_state_json).collect();
            ok_value(json!({
                "ok": true,
                "count": states.len(),
                "sessions": states,
            }))
        }
        Err(r) => r,
    }
}

pub(crate) async fn create_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateSessionBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let work_id = body.work_id.trim().to_string();
    if work_id.is_empty() {
        return bad_request("DUAL_WORKID_REQUIRED", "workId is required");
    }
    let ts = now();
    let title = body
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| format!("双Agent · {work_id}"));
    let stages: Vec<StageRecord> = STAGE_NAMES.iter().map(|n| StageRecord::new(n)).collect();
    let s = DualAgentSession {
        id: format!("dual-agent-{}", Uuid::new_v4()),
        work_id: work_id.clone(),
        book_id: body
            .book_id
            .map(|b| b.trim().to_string())
            .filter(|b| !b.is_empty()),
        title,
        active_role: AGENT_PLANNING.into(),
        stage: STAGE_NAMES[0].into(),
        plan: None,
        windows: vec![],
        stages,
        transcript: vec![AgentTurn {
            role: "assistant".into(),
            content: "Goethe 规划会话已创建。建议顺序：1) 执行规划 2) 交接 Dante 写作".into(),
            at: ts.clone(),
        }],
        chat_transcript: vec![],
        context_ledger: vec![],
        handoff_ok: false,
        llm_note: String::new(),
        created_at: ts.clone(),
        updated_at: ts,
        error: String::new(),
        workspace_id: session.workspace_id.clone(),
        review: vec![],
        summaries: vec![],
        styled_windows: vec![],
        auto_confirm: false,
    };
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    (
        StatusCode::CREATED,
        Json(json!({"ok": true, "session": s.clone(), "state": session_state_json(&s)})),
    )
        .into_response()
}

pub(crate) async fn get_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match load_session(&state, &id) {
        Ok(s) => {
            if let Err(r) = require_workspace(&s, &session) {
                return r;
            }
            ok_value(json!({"ok": true, "session": s.clone(), "state": session_state_json(&s)}))
        }
        Err(r) => r,
    }
}

// ── handler：plan ─────────────────────────────────────────────────────────

pub(crate) async fn plan_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<PlanBody>>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    let instruction = body
        .and_then(|Json(b)| b.instruction)
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let iterating = instruction.is_some();

    // 已有规划且本轮无新增指示 → 幂等返回当前规划（U12 行为，避免重复消耗 LLM）。
    if s.plan.is_some() && !iterating {
        return ok_value(json!({
            "ok": true,
            "idempotent": true,
            "session": s.clone(),
            "plan": s.plan.clone(),
            "llmNote": s.llm_note,
            "pendingConfirmation": plan_pending_confirmation(&s),
        }));
    }

    if iterating {
        push_chat(&mut s, "user", instruction.as_deref().unwrap_or(""), "plan");
    }
    start_stage(&mut s, "context_assembly");
    if let Err(r) = save_session(&state, &s) {
        return r;
    }

    // ── context_assembly：装配真实上下文 ──
    let work_context = assemble_work_context(&state, &s);

    let llm = agent_llm(&state);
    let prov_kind = agent_provider_kind(&state);
    let (plan, note) = if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        (
            heuristic_plan_iter(&s, instruction.as_deref()),
            if iterating {
                "LLM 未配置，已生成启发式迭代规划（确认后即可交接写作）".to_string()
            } else {
                "LLM 未配置，已生成启发式占位规划（确认后即可交接写作）".to_string()
            },
        )
    } else {
        let inst = instruction.clone().unwrap_or_default();
        match run_goethe_plan_iter(&prov_kind, &llm, &s, &inst, &work_context).await {
            Ok(p) => (
                p,
                if iterating {
                    "Goethe 规划已按新增指示迭代（LLM 产出）".to_string()
                } else {
                    "Goethe 规划完成（LLM 产出）".to_string()
                },
            ),
            Err(e) => {
                tracing::warn!(session_id=%s.id, error=%e, "goethe plan LLM failed");
                (
                    heuristic_plan_iter(&s, instruction.as_deref()),
                    format!("Goethe LLM 失败，回退启发式规划：{e}"),
                )
            }
        }
    };

    let mut plan = plan;
    plan.state = "proposed".into();
    let step = if iterating { "plan_iteration" } else { "plan" };
    s.plan = Some(plan.clone());
    s.llm_note = note.clone();
    complete_stage(&mut s, "context_assembly", "规划输出：设定 / 大纲 / 伏笔清单（提案待确认）", None);
    let summary = format!(
        "Goethe 规划{}。方向：{}；大纲章数：{}；待写窗口：{}。提案待确认，确认后可交接 Dante。",
        if iterating { "已迭代" } else { "完成" },
        plan.direction,
        plan.outline.len(),
        plan.next_window.join("、")
    );
    s.transcript.push(AgentTurn {
        role: "assistant".into(),
        content: summary.clone(),
        at: now(),
    });
    push_chat(&mut s, "assistant", &summary, step);
    let o_chars = outline_chars(&s);
    let f_count = foreshadow_count(&s);
    ledger_push(&mut s, step, o_chars, f_count);
    // ── auto_confirm：自动确认规划提案 ──
    if s.auto_confirm {
        if let Some(p) = s.plan.as_mut() {
            if p.state != "confirmed" {
                p.state = "confirmed".into();
            }
        }
        push_chat(&mut s, "assistant", "规划提案已自动确认（auto_confirm=true）", "auto_confirm");
        s.llm_note.push_str("；已自动确认");
    }
    s.updated_at = now();
    let pending = plan_pending_confirmation(&s);
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "session": s.clone(),
        "plan": s.plan.clone(),
        "llmNote": s.llm_note.clone(),
        "pendingConfirmation": pending,
        "state": session_state_json(&s),
    }))
}

pub(crate) async fn get_plan_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    match s.plan {
        Some(p) => ok_value(json!({
            "ok": true,
            "sessionId": s.id,
            "plan": p,
            "llmNote": s.llm_note,
            "pendingConfirmation": p.state != "confirmed",
        })),
        None => not_found("DUAL_NO_PLAN", "plan not produced yet"),
    }
}

// ── handler：confirm-plan / chat（U12-A2 规划提案待确认 + 自然语言触发）────────

pub(crate) async fn confirm_plan_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    _body: Option<Json<Value>>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if s.plan.is_none() {
        return bad_request("DUAL_NO_PLAN", "尚无规划产出，请先 POST /plan");
    }
    let already = s.plan.as_ref().map(|p| p.state == "confirmed").unwrap_or(false);
    if !already {
        if let Some(p) = s.plan.as_mut() {
            p.state = "confirmed".into();
        }
        push_chat(&mut s, "assistant", "规划提案已确认（plan.state=confirmed），可执行交接写作。", "confirm");
        let o_chars = outline_chars(&s);
        let f_count = foreshadow_count(&s);
        ledger_push(&mut s, "confirm_plan", o_chars, f_count);
        s.updated_at = now();
        if let Err(r) = save_session(&state, &s) {
            return r;
        }
    }
    ok_value(json!({
        "ok": true,
        "confirmed": true,
        "idempotent": already,
        "session": s.clone(),
        "plan": s.plan.clone(),
        "pendingConfirmation": false,
        "state": session_state_json(&s),
    }))
}

// ── handler：auto-confirm ────────────────────────────────────────────────

pub(crate) async fn set_auto_confirm_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<AutoConfirmBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    s.auto_confirm = body.enabled;
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "autoConfirm": s.auto_confirm,
        "state": session_state_json(&s),
    }))
}

// ── handler：chat（自然语言会话：显式确认 / handoff 触发，其余进入对话式规划迭代）──

/// 自然语言会话：显式确认 / handoff 触发，其余进入对话式规划迭代（A1）。
pub(crate) async fn chat_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ChatBody>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let message = body.message.trim().to_string();
    if message.is_empty() {
        return bad_request("DUAL_EMPTY_MESSAGE", "message is empty");
    }
    push_chat(&mut s, "user", &message, "chat");

    let explicit_confirm = looks_like_explicit_confirm(&message);
    let handoff_request = looks_like_handoff_request(&message);

    if explicit_confirm {
        let confirmed = s.plan.is_some();
        if let Some(p) = s.plan.as_mut() {
            if p.state != "confirmed" {
                p.state = "confirmed".into();
            }
        }
        push_chat(
            &mut s,
            "assistant",
            if confirmed {
                "已收到确认：规划提案置为 confirmed，可交接写作。"
            } else {
                "尚未生成规划提案，无可确认内容；请先描述规划需求。"
            },
            "confirm",
        );
        if confirmed {
            let o_chars = outline_chars(&s);
            let f_count = foreshadow_count(&s);
            ledger_push(&mut s, "confirm_plan", o_chars, f_count);
        }
        s.updated_at = now();
        if let Err(r) = save_session(&state, &s) {
            return r;
        }
        // 同句同时表达交接意图时，确认后继续交接（确认优先级参考 goethe.py）。
        if handoff_request {
            return run_handoff_common(&state, &mut s);
        }
        return ok_value(json!({
            "ok": true,
            "confirmed": confirmed,
            "pendingConfirmation": plan_pending_confirmation(&s),
            "session": s.clone(),
            "plan": s.plan.clone(),
            "chatTranscript": s.chat_transcript,
            "state": session_state_json(&s),
        }));
    }

    if handoff_request {
        s.updated_at = now();
        if let Err(r) = save_session(&state, &s) {
            return r;
        }
        return run_handoff_common(&state, &mut s);
    }

    // 其余输入 → 对话式规划迭代：喂回上一轮 PlanOutput + 本条用户消息。
    start_stage(&mut s, "context_assembly");
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    // ── context_assembly：装配真实上下文 ──
    let work_context = assemble_work_context(&state, &s);

    let llm = agent_llm(&state);
    let prov_kind = agent_provider_kind(&state);
    let (plan, note) = if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        (
            heuristic_plan_iter(&s, Some(&message)),
            "LLM 未配置，已生成启发式迭代规划".to_string(),
        )
    } else {
        match run_goethe_plan_iter(&prov_kind, &llm, &s, &message, &work_context).await {
            Ok(p) => (p, "Goethe 规划已按对话迭代（LLM 产出）".to_string()),
            Err(e) => {
                tracing::warn!(session_id=%s.id, error=%e, "goethe chat plan LLM failed");
                (
                    heuristic_plan_iter(&s, Some(&message)),
                    format!("Goethe LLM 失败，回退启发式规划：{e}"),
                )
            }
        }
    };
    let mut plan = plan;
    plan.state = "proposed".into();
    s.plan = Some(plan.clone());
    s.llm_note = note.clone();
    complete_stage(&mut s, "context_assembly", "规划迭代完成：设定 / 大纲 / 伏笔清单已更新", None);
    let summary = format!(
        "Goethe 规划已迭代。方向：{}；大纲章数：{}。当前提案为待确认状态，确认后即可交接。",
        plan.direction,
        plan.outline.len()
    );
    s.transcript.push(AgentTurn {
        role: "assistant".into(),
        content: summary.clone(),
        at: now(),
    });
    push_chat(&mut s, "assistant", &summary, "plan_iteration");
    let o_chars = outline_chars(&s);
    let f_count = foreshadow_count(&s);
    ledger_push(&mut s, "plan_iteration", o_chars, f_count);
    // ── auto_confirm：自动确认规划提案 ──
    if s.auto_confirm {
        if let Some(p) = s.plan.as_mut() {
            if p.state != "confirmed" {
                p.state = "confirmed".into();
            }
        }
        push_chat(&mut s, "assistant", "规划提案已自动确认（auto_confirm=true）", "auto_confirm");
        s.llm_note.push_str("；已自动确认");
    }
    s.updated_at = now();
    let pending = plan_pending_confirmation(&s);
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "session": s.clone(),
        "plan": s.plan.clone(),
        "llmNote": s.llm_note.clone(),
        "pendingConfirmation": pending,
        "chatTranscript": s.chat_transcript,
        "state": session_state_json(&s),
    }))
}

// ── handler：handoff / windows ────────────────────────────────────────────

pub(crate) async fn handoff_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    run_handoff_common(&state, &mut s)
}

pub(crate) async fn get_windows_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    let count = s.windows.len();
    ok_value(json!({
        "ok": true,
        "sessionId": s.id,
        "handoffDone": s.handoff_ok,
        "windows": s.windows,
        "count": count,
    }))
}

pub(crate) async fn write_window_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, window_id)): Path<(String, String)>,
    Json(body): Json<WriteWindowBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let content = body.content.trim().to_string();
    if content.is_empty() {
        return bad_request("DUAL_EMPTY_CONTENT", "content is empty");
    }
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    if !s.handoff_ok {
        return bad_request("DUAL_NO_HANDOFF", "尚未交接，请先 POST /handoff");
    }
    let wid = window_id.clone();
    let idx = match s.windows.iter().position(|w| w.id == wid) {
        Some(i) => i,
        None => {
            return not_found("DUAL_WINDOW_NOT_FOUND", format!("window not found: {window_id}"))
        }
    };
    {
        let win = &mut s.windows[idx];
        win.status = "written".into();
        win.draft = Some(content);
        win.assigned_role = Some(AGENT_WRITING.into());
        win.written_at = Some(now());
    }
    let win = s.windows[idx].clone();
    if s.windows.iter().all(|w| w.status == "written") {
        complete_stage(&mut s, "writing", "全部写作窗口已完成，进入审稿", None);
    }
    let f_count = foreshadow_count(&s);
    ledger_push(
        &mut s,
        "write_window",
        win.draft.as_ref().map(|d| d.chars().count()).unwrap_or(0),
        f_count,
    );
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    let state = session_state_json(&s);
    ok_value(json!({
        "ok": true,
        "window": win,
        "sessionId": s.id,
        "stage": s.stage,
        "state": state,
    }))
}

// ── P0 handler：AI 写稿（generate）────────────────────────────────────────

pub(crate) async fn generate_window_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, window_id)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    if !s.handoff_ok {
        return bad_request("DUAL_NO_HANDOFF", "尚未交接，请先 POST /handoff");
    }
    let wid = window_id.clone();
    let idx = match s.windows.iter().position(|w| w.id == wid) {
        Some(i) => i,
        None => {
            return not_found("DUAL_WINDOW_NOT_FOUND", format!("window not found: {window_id}"))
        }
    };
    // 标记窗口为 running
    {
        let win = &mut s.windows[idx];
        win.status = "running".into();
        win.assigned_role = Some(AGENT_WRITING.into());
    }
    s.llm_note = "Dante 正在写作…".into();
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }

    let llm = agent_llm(&state);
    let prov_kind = agent_provider_kind(&state);
    let win_snapshot = s.windows[idx].clone();
    let (draft, note) = dante_write_with_retry(&prov_kind, &llm, &s, &win_snapshot).await;

    // [fix 2026-08-16] 占位稿（note 以 ERROR 前缀标记）不再静默标 written：
    // 标为 placeholder，且不推进 writing 阶段完成，避免用户误以为已产出正式正文。
    let is_placeholder = note.starts_with("ERROR:");
    // 写入窗口
    {
        let win = &mut s.windows[idx];
        win.draft = Some(draft);
        win.status = if is_placeholder { "placeholder".into() } else { "written".into() };
        win.written_at = Some(now());
    }
    s.llm_note = note.clone();
    let draft_chars = s.windows[idx].draft.as_ref().map(|d| d.chars().count()).unwrap_or(0);
    let f_count = foreshadow_count(&s);
    ledger_push(&mut s, "write_window", draft_chars, f_count);
    // 全部窗口写完且无占位 → 自动完成 writing 阶段
    let all_written = s.windows.iter().all(|w| w.status == "written");
    if all_written && !is_placeholder {
        complete_stage(&mut s, "writing", "全部写作窗口已完成，进入审稿", None);
    }
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    let state_json = session_state_json(&s);
    ok_value(json!({
        "ok": true,
        "window": s.windows[idx],
        "llmNote": note,
        "sessionId": s.id,
        "stage": s.stage,
        "state": state_json,
    }))
}

// ── P1 handler：写稿落盘（draft → 作品章节 md）─────────────────────────────

/// 落盘文件名净化：只保留安全字符，防止路径逃逸。
pub(crate) fn sanitize_filename(name: &str) -> String {
    name.chars()
        .filter(|c| {
            matches!(c,
                'A'..='Z' | 'a'..='z' | '0'..='9'
                | '_' | '-' | '.'
                | '\u{4E00}'..='\u{9FFF}'
            )
        })
        .collect::<String>()
        .trim_matches('.')
        .to_string()
}

/// 把 written 窗口的 draft 落盘为作品章节 md 文件。
pub(crate) async fn publish_window_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, window_id)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    let idx = match s.windows.iter().position(|w| w.id == window_id) {
        Some(i) => i,
        None => {
            return not_found("DUAL_WINDOW_NOT_FOUND", format!("window not found: {window_id}"))
        }
    };
    let win = &s.windows[idx];
    if win.status != "written" || win.draft.is_none() {
        return bad_request("DUAL_NOT_WRITTEN", "窗口未完成写作，无法落盘");
    }
    let draft = win.draft.clone().unwrap_or_default();

    // ── 落盘路径：data/works/{workspace_id}/{safe_work_id}/{safe_filename}.md ──
    let ws_root = match state.works.workspace_root(&s.workspace_id) {
        Ok(r) => r,
        Err(e) => {
            return internal("DUAL_WORKSPACE_ROOT", format!("workspace root: {e}"))
        }
    };
    let safe_work_id = sanitize_work_id(&s.work_id);
    let work_dir = if safe_work_id.is_empty() {
        ws_root.clone()
    } else {
        ws_root.join(&safe_work_id)
    };

    // 文件名：chapter_id 优先，其次 title，再其次 ch{序号}
    let raw_name = if !win.chapter_id.trim().is_empty() {
        win.chapter_id.clone()
    } else if !win.title.trim().is_empty() {
        win.title.clone()
    } else {
        format!("ch{}", idx + 1)
    };
    let mut file_name = sanitize_filename(&raw_name.trim());
    if file_name.is_empty() {
        file_name = format!("ch{}", idx + 1);
    }
    if !file_name.ends_with(".md") {
        file_name.push_str(".md");
    }

    // 创建 work_dir（不存在则建）
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        return internal("DUAL_CREATE_WORK_DIR", format!("create work dir: {e}"));
    }
    let target = work_dir.join(&file_name);
    // 双保险：确认 target 在 ws_root 内（sanitize 后 join 不应逃逸，但防御性检查）
    let canonical_ws = match std::fs::canonicalize(&ws_root) {
        Ok(p) => p,
        Err(_) => ws_root.clone(),
    };
    let target_abs = if target.exists() {
        std::fs::canonicalize(&target).unwrap_or_else(|_| target.clone())
    } else {
        target.clone()
    };
    if !target_abs.starts_with(&canonical_ws) {
        return bad_request("DUAL_PATH_ESCAPE", "落盘路径越界，已拒绝");
    }

    // 内容：标题行 + draft
    let mut content = String::new();
    if !win.title.trim().is_empty() {
        content.push_str(&format!("# {}\n\n", win.title));
    }
    content.push_str(&draft);
    if let Err(e) = std::fs::write(&target, &content) {
        return internal("DUAL_WRITE_FILE", format!("write file: {e}"));
    }

    // 记账 + 返回
    let rel_path = format!("{}/{}", work_dir.file_name().unwrap_or_default().to_string_lossy(), file_name);
    ledger_push(&mut s, "publish", draft.chars().count(), 0);
    s.llm_note = format!("已落盘章节：{rel_path}");
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "path": rel_path,
        "fileName": file_name,
        "windowId": window_id,
        "llmNote": s.llm_note,
        "sessionId": s.id,
        "state": session_state_json(&s),
    }))
}

// ── P1 handler：AI 审稿 ──────────────────────────────────────────────────

pub(crate) async fn review_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    if !s.handoff_ok {
        return bad_request("DUAL_NO_HANDOFF", "尚未交接");
    }
    // 校验全部窗口已 written
    let pending: Vec<&str> = s.windows.iter().filter(|w| w.status != "written").map(|w| w.id.as_str()).collect();
    if !pending.is_empty() {
        return crate::error_codes::err_with_code(
            axum::http::StatusCode::BAD_REQUEST,
            "DUAL_NOT_WRITTEN",
            "尚有窗口未完成写作",
            serde_json::json!({ "pendingWindows": pending }),
        );
    }
    start_stage(&mut s, "review");
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }

    let llm = agent_llm(&state);
    let prov_kind = agent_provider_kind(&state);
    let (review_items, note) = if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        (
            vec![],
            "LLM 未配置，跳过审稿".to_string(),
        )
    } else {
        match run_review_iter(&prov_kind, &llm, &s).await {
            Ok(items) => (items, "AI 审稿完成（LLM 产出）".to_string()),
            Err(e) => {
                tracing::warn!(session_id=%s.id, error=%e, "review LLM failed");
                (vec![], format!("审稿 LLM 失败：{e}"))
            }
        }
    };

    let major_count = review_items.iter().filter(|r| r.severity == "major").count();
    let minor_count = review_items.iter().filter(|r| r.severity == "minor").count();
    s.review = review_items.clone();
    s.llm_note = note.clone();
    let msg = format!("审稿完成：{major_count} 个 major 问题，{minor_count} 个 minor 问题");
    complete_stage(&mut s, "review", &msg, Some(json!({"major": major_count, "minor": minor_count})));
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "review": review_items,
        "majorCount": major_count,
        "minorCount": minor_count,
        "llmNote": note,
        "sessionId": s.id,
        "stage": s.stage,
        "state": session_state_json(&s),
    }))
}

// ── P1 handler：风格统一 ─────────────────────────────────────────────────

pub(crate) async fn styling_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    start_stage(&mut s, "styling");
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }

    let llm = agent_llm(&state);
    let prov_kind = agent_provider_kind(&state);
    let mut styled = Vec::new();
    let mut note = String::new();
    let has_llm = !llm.base_url.trim().is_empty() && !llm.api_key.trim().is_empty();

    for win in &s.windows {
        if win.status != "written" || win.draft.is_none() {
            continue;
        }
        if has_llm {
            match run_styling_iter(&prov_kind, &llm, &s, win).await {
                Ok(styled_text) => {
                    styled.push(StyledWindow {
                        window_id: win.id.clone(),
                        styled_draft: styled_text,
                    });
                    note = "风格统一完成（LLM 产出）".into();
                }
                Err(e) => {
                    tracing::warn!(session_id=%s.id, window_id=%win.id, error=%e, "styling LLM failed");
                    // 兜底：原样返回
                    styled.push(StyledWindow {
                        window_id: win.id.clone(),
                        styled_draft: win.draft.clone().unwrap_or_default(),
                    });
                    note = format!("风格统一 LLM 失败（{e}），部分窗口原样保留");
                }
            }
        } else {
            // 无 LLM 时直接原样
            styled.push(StyledWindow {
                window_id: win.id.clone(),
                styled_draft: win.draft.clone().unwrap_or_default(),
            });
            note = "LLM 未配置，风格统一跳过（原样保留）".into();
        }
    }

    s.styled_windows = styled;
    s.llm_note = note.clone();
    complete_stage(&mut s, "styling", "风格统一完成", None);
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "styledWindows": s.styled_windows.len(),
        "llmNote": note,
        "sessionId": s.id,
        "stage": s.stage,
        "state": session_state_json(&s),
    }))
}

// ── P1 handler：章节压缩 ─────────────────────────────────────────────────

pub(crate) async fn compress_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    start_stage(&mut s, "compression");
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }

    let llm = agent_llm(&state);
    let prov_kind = agent_provider_kind(&state);
    let mut summaries = Vec::new();
    let mut note = String::new();
    let has_llm = !llm.base_url.trim().is_empty() && !llm.api_key.trim().is_empty();

    for win in &s.windows {
        if win.status != "written" || win.draft.is_none() {
            continue;
        }
        let summary_text = if has_llm {
            match run_compress_iter(&prov_kind, &llm, &s, win).await {
                Ok(text) => {
                    note = "章节摘要完成（LLM 产出）".into();
                    text
                }
                Err(e) => {
                    tracing::warn!(session_id=%s.id, window_id=%win.id, error=%e, "compress LLM failed");
                    note = format!("摘要 LLM 失败（{e}），使用截断兜底");
                    heuristic_compress(win.draft.as_deref().unwrap_or(""))
                }
            }
        } else {
            note = "LLM 未配置，使用截断兜底".into();
            heuristic_compress(win.draft.as_deref().unwrap_or(""))
        };
        summaries.push(ChapterSummary {
            window_id: win.id.clone(),
            chapter_id: win.chapter_id.clone(),
            summary: summary_text,
        });
    }

    s.summaries = summaries;
    s.llm_note = note.clone();
    complete_stage(&mut s, "compression", "章节压缩完成", None);
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }

    // Harness P3：章节压缩完成是干净的 auto-refine 触发点（默认关闭、后台、防御性）。
    {
        let st = state.clone();
        let sess = s.clone();
        tokio::spawn(async move {
            maybe_auto_refine(st, &sess).await;
        });
    }

    ok_value(json!({
        "ok": true,
        "summariesCount": s.summaries.len(),
        "llmNote": note,
        "sessionId": s.id,
        "stage": s.stage,
        "state": session_state_json(&s),
    }))
}

pub(crate) async fn advance_stage_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<StageBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    let name = body.name.trim().to_string();
    if !STAGE_NAMES.contains(&name.as_str()) {
        return crate::error_codes::err_with_code(
            axum::http::StatusCode::BAD_REQUEST,
            "DUAL_BAD_STAGE",
            format!("invalid stage: {name}"),
            serde_json::json!({ "validStages": STAGE_NAMES }),
        );
    }
    match body.status.to_ascii_lowercase().as_str() {
        "start" => start_stage(&mut s, &name),
        "complete" | "completed" | "done" => {
            complete_stage(&mut s, &name, &body.message, body.data.clone())
        }
        "fail" | "failed" => fail_stage(&mut s, &name, &body.message),
        "skip" | "skipped" => skip_stage(&mut s, &name, &body.message),
        other => {
            return crate::error_codes::err_with_code(
                axum::http::StatusCode::BAD_REQUEST,
                "DUAL_BAD_STATUS",
                format!("invalid status: {other}"),
                serde_json::json!({ "valid": ["start", "complete", "fail", "skip"] }),
            );
        }
    }
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "session": s.clone(),
        "state": session_state_json(&s),
    }))
}

pub(crate) async fn resume_session_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    s.error = String::new();
    let o_chars = outline_chars(&s);
    let f_count = foreshadow_count(&s);
    ledger_push(&mut s, "resume", o_chars, f_count);
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    // ── 执行真正的 resume 续跑逻辑 ──
    let (actions, note) = run_resume_logic(&state, &mut s).await;
    if !note.is_empty() {
        s.llm_note = note;
    }
    s.updated_at = now();
    if let Err(r) = save_session(&state, &s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "resumed": true,
        "actions": actions,
        "session": s.clone(),
        "state": session_state_json(&s),
        "nextAction": next_action(&s),
    }))
}

// ── resume 核心逻辑：按阶段分派续跑 ───────────────────────────────────

pub(crate) async fn get_state_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(r) = require_workspace(&s, &session) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "session": s.clone(),
        "state": session_state_json(&s),
    }))
}

pub(crate) async fn get_ledger_h(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let s = match load_session(&state, &id) {
        Ok(s) => s,
        Err(r) => return r,
    };
    ok_value(json!({
        "ok": true,
        "sessionId": s.id,
        "count": s.context_ledger.len(),
        "ledger": s.context_ledger,
    }))
}

