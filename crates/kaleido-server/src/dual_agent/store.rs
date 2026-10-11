//! dual_agent/store — [2026-10-05 语义拆分] 由 dual_agent.rs 按设计文档
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

pub(crate) fn dual_agent_dir(state: &AppState) -> PathBuf {
    state.auth.data_root().root().join("dual-agent")
}

pub(crate) fn session_path(state: &AppState, id: &str) -> PathBuf {
    dual_agent_dir(state).join(format!("{id}.json"))
}

pub(crate) fn safe_session_id(id: &str) -> Result<String, Response> {
    let s = id.trim();
    if s.is_empty() || s.contains('/') || s.contains('\\') || s.contains("..") {
        return Err(bad_request("DUAL_BAD_ID", "invalid dual-agent session id"));
    }
    Ok(s.to_string())
}

pub(crate) fn save_session(state: &AppState, s: &DualAgentSession) -> Result<(), Response> {
    let dir = dual_agent_dir(state);
    std::fs::create_dir_all(&dir).map_err(|e| {
        internal("DUAL_CREATE_DIR", format!("create dual-agent dir: {e}"))
    })?;
    let body = serde_json::to_string_pretty(s).map_err(|e| {
        internal("DUAL_SERIALIZE", format!("serialize session: {e}"))
    })?;
    let path = session_path(state, &s.id);
    let tmp = dir.join(format!("{}.tmp", s.id));
    std::fs::write(&tmp, &body).map_err(|e| {
        internal("DUAL_WRITE", format!("write session: {e}"))
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        internal("DUAL_COMMIT", format!("commit session: {e}"))
    })
}

pub(crate) fn load_session(state: &AppState, id: &str) -> Result<DualAgentSession, Response> {
    let id = safe_session_id(id)?;
    let body = std::fs::read_to_string(session_path(state, &id)).map_err(|_| {
        not_found("DUAL_NOT_FOUND", format!("dual-agent session not found: {id}"))
    })?;
    serde_json::from_str::<DualAgentSession>(&body).map_err(|e| {
        internal("DUAL_CORRUPT", format!("corrupt session {id}: {e}"))
    })
}

/// audit P1 IDOR: 会话归属校验——非本 workspace 的 dual-agent 会话一律 403。
/// 兼容历史空 workspace_id（旧数据）视为可见，但写操作仍受限。
pub(crate) fn require_workspace(
    s: &DualAgentSession,
    session: &kaleido_core::SessionRecord,
) -> Result<(), Response> {
    if !s.workspace_id.is_empty() && s.workspace_id != session.workspace_id {
        return Err(forbidden("DUAL_FORBIDDEN", "dual-agent session not in your workspace"));
    }
    Ok(())
}

pub(crate) fn list_sessions(state: &AppState) -> Result<Vec<DualAgentSession>, Response> {
    let dir = dual_agent_dir(state);
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().map(|x| x == "json").unwrap_or(false) {
                if let Ok(body) = std::fs::read_to_string(&path) {
                    if let Ok(s) = serde_json::from_str::<DualAgentSession>(&body) {
                        out.push(s);
                    }
                }
            }
        }
    }
    Ok(out)
}

// ── 状态机工具 ─────────────────────────────────────────────────────────────

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339()
}

/// 过滤掉备份/隐藏文件（如 `_recover_ver.md`）。
pub(crate) fn is_backup_file(name: &str) -> bool {
    name.starts_with('_') || name.starts_with('.') || name.ends_with('~')
}

/// sanitize work_id：只保留安全字符，防止路径逃逸。
pub(crate) fn sanitize_work_id(work_id: &str) -> String {
    work_id
        .chars()
        .filter(|c| {
            matches!(c,
                'A'..='Z' | 'a'..='z' | '0'..='9'
                | '_' | '-' | '.'
                | '\u{4E00}'..='\u{9FFF}'
            )
        })
        .collect()
}

