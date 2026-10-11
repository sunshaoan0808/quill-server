//! dual_agent/harness_hook — [2026-10-05 语义拆分 PH-B] 由 dual_agent/mod.rs 按设计文档
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

// ── Harness P3 auto-refine 可选钩子 ─────────────────────────────────────
//
// 默认关闭。通过 env `HARNESS_AUTO_REFINE`（=1/true）开启。在章节压缩完成等
// 干净的阶段完成点触发：gate → should_refine 才 run_refine。任何 LLM/IO 错误
// 一律吞掉只打日志，绝不影响写作主流程。手动路径见 `harness_api::refine`。
//
// P4：gate/refine 的 guidance 对齐由 `harness_bridge`（load_state 后调
// guidance_summary）在构造 PlanContext/ReviewContext 时自动注入，此钩子无需
// 额外改动；默认关闭，零行为变化。
pub(crate) fn harness_auto_refine_enabled() -> bool {
    match std::env::var("HARNESS_AUTO_REFINE") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            v == "1" || v == "true" || v == "yes"
        }
        Err(_) => false,
    }
}

/// 从会话摘要近似构建 auto-refine 的 conversation_tail（无摘要则回退会话标题）。
pub(crate) fn session_tail(s: &DualAgentSession) -> String {
    if s.summaries.is_empty() {
        return s.title.clone();
    }
    let mut tail = String::new();
    for sum in s.summaries.iter().rev().take(5) {
        tail.push_str(&sum.chapter_id);
        tail.push_str(": ");
        tail.push_str(&sum.summary);
        tail.push('\n');
    }
    tail
}

/// 触发 auto-refine：env 开启 + provider 已配置 + gate 通过才真正 run_refine。
/// 防御性：任何错误只 log，返回不可见（后台跑，不阻塞/不改变主响应）。
pub(crate) async fn maybe_auto_refine(state: AppState, s: &DualAgentSession) {
    if !harness_auto_refine_enabled() {
        return;
    }
    let session = s.clone();
    let root = state.auth.data_root().root().to_path_buf();
    let Some(llm) = LlmClientImpl::from_env() else {
        tracing::warn!(session_id=%session.id, "HARNESS_AUTO_REFINE enabled but LLM provider not configured; skipping auto-refine");
        return;
    };
    let tail = session_tail(&session);
    match auto_refine_gate(&root, &llm, "compact", &tail).await {
        Ok(true) => {
            let _ = run_refine(&root, &llm, &tail, None, None).await.map_err(|e| {
                tracing::warn!(session_id=%session.id, error=%e, "harness auto-refine failed");
            });
        }
        Ok(false) => tracing::debug!(session_id=%session.id, "harness auto-refine gate declined"),
        Err(e) => tracing::warn!(session_id=%session.id, error=%e, "harness auto-refine gate error"),
    }
}

// ── handler：stage / resume / state ───────────────────────────────────────

