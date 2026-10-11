//! dual_agent/handoff — [2026-10-05 语义拆分 PH-B] 由 dual_agent/mod.rs 按设计文档
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

pub(crate) fn str_get(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// 由规划大纲生成写作窗口清单（含 placeholder 兜底）。
pub(crate) fn build_windows(s: &DualAgentSession) -> Vec<WritingWindow> {
    let mut windows = Vec::new();
    if let Some(p) = s.plan.as_ref() {
        let mut idx = 0usize;
        for item in &p.outline {
            idx += 1;
            let raw_chapter = str_get(item, "chapter");
            let raw_title = str_get(item, "title");
            let chapter_id = if raw_chapter.is_empty() {
                format!("ch{:02}", idx)
            } else {
                raw_chapter
            };
            let title = if raw_title.is_empty() {
                format!("第{idx}章")
            } else {
                raw_title
            };
            let goal = str_get(item, "goal");
            windows.push(WritingWindow {
                id: format!("win-{idx:02}"),
                chapter_id,
                title,
                status: "pending".into(),
                outline: goal.clone(),
                prompt: format!("Dante 请依据大纲目标撰写本章正文：{goal}"),
                word_target: 2000,
                assigned_role: None,
                draft: None,
                written_at: None,
                sub_tasks: vec![],
            });
        }
    }
    if windows.is_empty() {
        for i in 1..=2 {
            windows.push(WritingWindow {
                id: format!("win-{i:02}"),
                chapter_id: format!("ch{i:02}"),
                title: format!("第{i}章"),
                status: "pending".into(),
                outline: "待规划补充大纲目标".into(),
                prompt: "Dante 请撰写本章正文。".into(),
                word_target: 2000,
                assigned_role: None,
                draft: None,
                written_at: None,
                sub_tasks: vec![],
            });
        }
    }
    windows
}

/// A3：handoff 完整性校验协议 —— {ok, blocked, error, next_action, missing_items}。
pub(crate) struct HandoffProtocol {
    pub(crate) ok: bool,
    pub(crate) blocked: bool,
    pub(crate) error: String,
    pub(crate) next_action: String,
    pub(crate) missing_items: Vec<String>,
}

/// 校验是否满足交接条件：规划已产出 + 已确认 + 大纲/伏笔非空。
/// blocked=missing_items 非空（大纲 outline 或 foreshadowItems 为空）→
/// next_action="fill_outline" / "fill_foreshadow"；ok → next_action="start_writing"。
pub(crate) fn handoff_check(s: &DualAgentSession) -> HandoffProtocol {
    match s.plan {
        None => HandoffProtocol {
            ok: false,
            blocked: true,
            error: "plan not produced yet".into(),
            next_action: "fill_outline".into(),
            missing_items: vec!["outline".into(), "foreshadowItems".into()],
        },
        Some(ref p) if p.state != "confirmed" => HandoffProtocol {
            ok: false,
            blocked: true,
            error: "plan pending confirmation".into(),
            next_action: "confirm_plan".into(),
            missing_items: vec!["plan_confirmation".into()],
        },
        Some(ref p) => {
            let mut missing = Vec::new();
            if p.outline.is_empty() {
                missing.push("outline".into());
            }
            if p.foreshadow_items.is_empty() {
                missing.push("foreshadowItems".into());
            }
            if !missing.is_empty() {
                let next_action = if missing.iter().any(|m| m == "outline") {
                    "fill_outline"
                } else {
                    "fill_foreshadow"
                };
                HandoffProtocol {
                    ok: false,
                    blocked: true,
                    error: "handoff blocked: 必要规划章节缺失".into(),
                    next_action: next_action.into(),
                    missing_items: missing,
                }
            } else {
                HandoffProtocol {
                    ok: true,
                    blocked: false,
                    error: String::new(),
                    next_action: "start_writing".into(),
                    missing_items: vec![],
                }
            }
        }
    }
}

pub(crate) fn handoff_protocol_response(p: &HandoffProtocol, s: &DualAgentSession) -> Response {
    ok_value(json!({
        "ok": p.ok,
        "blocked": p.blocked,
        "error": p.error,
        "nextAction": p.next_action,
        "missingItems": p.missing_items,
        "session": s.clone(),
        "state": session_state_json(s),
    }))
}

/// 执行交接（已交接则幂等返回既有窗口；不满足条件返回 blocked 协议）。
pub(crate) fn run_handoff_common(state: &AppState, s: &mut DualAgentSession) -> Response {
    if s.handoff_ok {
        return ok_value(json!({
            "ok": true,
            "blocked": false,
            "error": "",
            "nextAction": "start_writing",
            "missingItems": [],
            "idempotent": true,
            "session": s.clone(),
            "windows": s.windows.clone(),
            "state": session_state_json(s),
        }));
    }
    let proto = handoff_check(s);
    if !proto.ok {
        return handoff_protocol_response(&proto, s);
    }

    s.windows = build_windows(s);
    s.handoff_ok = true;
    s.active_role = AGENT_WRITING.into();
    start_stage(s, "writing");
    let summary = format!(
        "Goethe 已完成交接：Dante 接管写作，共 {} 个写作窗口待接单。",
        s.windows.len()
    );
    s.transcript.push(AgentTurn {
        role: "assistant".into(),
        content: summary.clone(),
        at: now(),
    });
    push_chat(s, "assistant", &summary, "handoff");
    let o_chars = outline_chars(s);
    let f_count = foreshadow_count(s);
    ledger_push(s, "handoff", o_chars, f_count);
    s.updated_at = now();
    if let Err(r) = save_session(state, s) {
        return r;
    }
    ok_value(json!({
        "ok": true,
        "blocked": false,
        "error": "",
        "nextAction": "start_writing",
        "missingItems": [],
        "handoff": true,
        "session": s.clone(),
        "windows": s.windows.clone(),
        "state": session_state_json(s),
    }))
}

