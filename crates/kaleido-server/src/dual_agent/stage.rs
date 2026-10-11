//! dual_agent/stage — [2026-10-05 语义拆分] 由 dual_agent.rs 按设计文档
//! docs/plans/2026-10-05-p006-dual-agent-semantic-split.md §2.2 机械搬迁（字节保真，serde 契约未动）。

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

pub(crate) fn next_stage(name: &str) -> Option<&'static str> {
    STAGE_NAMES
        .iter()
        .position(|n| *n == name)
        .and_then(|i| STAGE_NAMES.get(i + 1).copied())
}

pub(crate) fn start_stage(s: &mut DualAgentSession, name: &str) {
    if let Some(st) = s.stages.iter_mut().find(|st| st.name == name) {
        st.status = "running".into();
        if st.started_at.is_empty() {
            st.started_at = now();
        }
        st.completed_at.clear();
        st.message.clear();
    }
    s.stage = name.to_string();
}

pub(crate) fn complete_stage(s: &mut DualAgentSession, name: &str, message: &str, data: Option<Value>) {
    if let Some(st) = s.stages.iter_mut().find(|st| st.name == name) {
        st.status = "completed".into();
        st.completed_at = now();
        st.message = message.to_string();
        if let Some(d) = data {
            st.data = d;
        }
    }
    if let Some(next) = next_stage(name) {
        s.stage = next.to_string();
    }
}

pub(crate) fn fail_stage(s: &mut DualAgentSession, name: &str, message: &str) {
    if let Some(st) = s.stages.iter_mut().find(|st| st.name == name) {
        st.status = "failed".into();
        st.completed_at = now();
        st.message = message.to_string();
    }
    s.error = format!("{name}: {message}");
}

pub(crate) fn skip_stage(s: &mut DualAgentSession, name: &str, message: &str) {
    if let Some(st) = s.stages.iter_mut().find(|st| st.name == name) {
        st.status = "skipped".into();
        st.message = if message.is_empty() { "跳过".into() } else { message.to_string() };
    }
    if let Some(next) = next_stage(name) {
        s.stage = next.to_string();
    }
}

pub(crate) fn role_label(role: &str) -> &'static str {
    match role {
        AGENT_WRITING => "Dante · 写作",
        _ => "Goethe · 规划",
    }
}

pub(crate) fn next_action(s: &DualAgentSession) -> &'static str {
    if s.plan.is_none() {
        return "run_plan";
    }
    if plan_pending_confirmation(s) {
        return "confirm_plan";
    }
    if !s.handoff_ok {
        return "run_handoff";
    }
    if s.windows.iter().any(|w| w.status != "written") {
        return "write_windows";
    }
    if s.stage != "compression" {
        return "advance_stage";
    }
    "done"
}

/// 截断辅助：将字符串截断到 max_chars，超出时加省略号。
pub(crate) fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

pub(crate) fn session_state_json(s: &DualAgentSession) -> Value {
    let stage_idx = STAGE_NAMES
        .iter()
        .position(|n| *n == s.stage)
        .unwrap_or(0);
    let done = s
        .stages
        .iter()
        .filter(|st| st.status == "completed" || st.status == "skipped")
        .count();
    let written = s.windows.iter().filter(|w| w.status == "written").count();
    // ── 审稿明细（截断防大）──
    let review_details: Vec<Value> = s
        .review
        .iter()
        .map(|r| {
            json!({
                "severity": r.severity,
                "issue": truncate_str(&r.issue, 200),
                "windowId": r.window_id,
            })
        })
        .collect();
    // ── 章节摘要明细（截断防大）──
    let summary_details: Vec<Value> = s
        .summaries
        .iter()
        .map(|sm| {
            json!({
                "windowId": sm.window_id,
                "chapterId": sm.chapter_id,
                "summary": truncate_str(&sm.summary, 300),
            })
        })
        .collect();
    // ── 风格化窗口明细（截断防大）──
    let styled_details: Vec<Value> = s
        .styled_windows
        .iter()
        .map(|sw| {
            json!({
                "windowId": sw.window_id,
                "styledDraft": truncate_str(&sw.styled_draft, 300),
            })
        })
        .collect();
    json!({
        "sessionId": s.id,
        "workId": s.work_id,
        "title": s.title,
        "activeRole": s.active_role,
        "activeRoleLabel": role_label(&s.active_role),
        "stage": s.stage,
        "stageIndex": stage_idx,
        "stageCount": STAGE_NAMES.len(),
        "stageProgress": done,
        "windowsTotal": s.windows.len(),
        "windowsWritten": written,
        "planReady": s.plan.is_some(),
        "planConfirmed": s.plan.as_ref().map(|p| p.state == "confirmed").unwrap_or(false),
        "pendingConfirmation": plan_pending_confirmation(s),
        "handoffDone": s.handoff_ok,
        "llmNote": s.llm_note,
        "error": s.error,
        "nextAction": next_action(s),
        "createdAt": s.created_at,
        "updatedAt": s.updated_at,
        "stages": s.stages,
        "reviewCount": s.review.len(),
        "summariesCount": s.summaries.len(),
        "styledWindowsCount": s.styled_windows.len(),
        "review": review_details,
        "summaries": summary_details,
        "styledWindows": styled_details,
        "autoConfirm": s.auto_confirm,
    })
}

/// 判断指定阶段是否已完成（status == "completed"）。
pub(crate) fn stage_completed(s: &DualAgentSession, name: &str) -> bool {
    s.stages.iter().any(|st| st.name == name && st.status == "completed")
}

