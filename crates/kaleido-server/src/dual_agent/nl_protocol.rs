//! dual_agent/nl_protocol — [2026-10-05 语义拆分] 由 dual_agent.rs 按设计文档
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

pub(crate) fn ok_value(v: Value) -> Response {
    Json(v).into_response()
}

// ── U12-A 上下文治理（D2 token 预算拟合 / D3 ContextLedger 账本）─────────────

/// 记录一条多轮对话（U12-A1 chat_transcript）。
pub(crate) fn push_chat(s: &mut DualAgentSession, role: &str, content: &str, tag: &str) {
    s.chat_transcript.push(ChatTurn {
        role: role.into(),
        content: content.into(),
        tag: tag.into(),
        at: now(),
    });
}

/// 记录一条 ContextLedger 账本（U12-A4/D3）。
pub(crate) fn ledger_push(
    s: &mut DualAgentSession,
    stage: &str,
    outline_chars: usize,
    foreshadow_count: usize,
) {
    s.context_ledger.push(ContextLedgerEntry {
        stage: stage.to_string(),
        plan_hash: s
            .plan
            .as_ref()
            .map(plan_hash)
            .unwrap_or_else(|| "none".to_string()),
        outline_chars,
        foreshadow_count,
        timestamp: now(),
    });
}

// ── U12-A2 自然语言触发识别（参考 goethe.py _confirm_outline_if_explicit / _looks_like_handoff_request）

/// 输入是否构成显式确认（确认 / ok / 没问题 / 就按这个 …）。
pub(crate) fn looks_like_explicit_confirm(text: &str) -> bool {
    let normalized: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    if normalized.is_empty() {
        return false;
    }
    const OVERRIDE_NEGATIVE: &[&str] = &[
        "不要再确认",
        "不用确认",
        "无需确认",
        "别再确认",
        "不需要确认",
        "直接应用",
        "直接修改",
        "直接写入",
        "直接保存",
    ];
    const NEGATIVE: &[&str] = &[
        "不确认", "不同意", "先不", "暂不", "不要", "别改", "取消", "放弃",
    ];
    const SHORT: &[&str] = &[
        "确认", "同意", "可以", "可以的", "应用", "保存", "提交", "就这样", "改吧", "好", "好的",
        "行", "行的", "嗯", "yes", "y", "ok", "okay", "apply", "confirm", "lgtm",
    ];
    const EXPLICIT: &[&str] = &[
        "确认应用",
        "确认执行",
        "确认修改",
        "确认大纲",
        "确认写入",
        "确认",
        "同意修改",
        "同意应用",
        "应用这版",
        "应用修改",
        "写入大纲",
        "保存修改",
        "保存大纲",
        "采用这版",
        "就按这版",
        "就按这个",
        "没问题",
        "提交修改",
        "可以应用",
        "可以写入",
        "可以保存",
    ];
    if OVERRIDE_NEGATIVE.iter().any(|m| normalized.contains(m)) {
        return true;
    }
    if NEGATIVE.iter().any(|m| normalized.contains(m)) {
        return false;
    }
    let core = normalized.trim_matches(|c: char| {
        c.is_ascii_punctuation() || matches!(c, '。' | '，' | '！' | '？' | '、')
    });
    if SHORT.iter().any(|m| core == *m) {
        return true;
    }
    EXPLICIT.iter().any(|m| normalized.contains(m))
}

/// 输入是否构成 handoff 触发（交接 / 开始写 / 递给写作 / 交给Dante / dante来 …）。
pub(crate) fn looks_like_handoff_request(text: &str) -> bool {
    let lowered = text.trim().to_lowercase();
    if lowered.is_empty() {
        return false;
    }
    const TOKENS: &[&str] = &[
        "交接",
        "handoff",
        "开始写",
        "递给写作",
        "交给 dante",
        "交给dante",
        "dante 来",
        "dante来",
        "切到 dante",
        "切到dante",
        "切换到 dante",
        "切换到dante",
    ];
    TOKENS.iter().any(|t| lowered.contains(t))
}

// ── handler：sessions ─────────────────────────────────────────────────────

