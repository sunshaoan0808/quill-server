//! dual_agent/budget — [2026-10-05 语义拆分] 由 dual_agent.rs 按设计文档
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

/// 反馈给 Goethe 的上一轮规划上下文 token 预算上限（A1 迭代输入裁剪）。
pub(crate) const PLAN_FEEDBACK_MAX_TOKENS: usize = 12000;

/// 粗略 token 估算（CJK 为主：非 ASCII 约 3 字节/词元，ASCII 约 4 字节/词元）。
pub(crate) fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let bytes = text.len();
    let ascii = text.bytes().filter(|b| b.is_ascii()).count();
    (bytes - ascii) / 3 + ascii / 4 + 1
}

/// 截取不超过 max_bytes 字节的 UTF-8 前缀（保证落在字符边界）。
pub(crate) fn take_utf8_prefix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A4/D2：token 预算内拟合长上下文 —— 前缀保留 + 尾部摘要标记。
/// 返回 (fitted, dropped_chars)。超预算时对长上下文（plan outline / 窗口 draft）
/// 做前缀保留 + 尾部摘要裁剪，避免 serde/写入时全量溢出报错。
pub fn fit_text_to_token_budget(text: &str, max_tokens: usize) -> (String, usize) {
    let original_chars = text.chars().count();
    if max_tokens == 0 || text.is_empty() {
        return (String::new(), original_chars);
    }
    if estimate_tokens(text) <= max_tokens {
        return (text.to_string(), 0);
    }
    const TAIL: &str = "\n…（已省略尾部 N 字符，按 token 预算裁剪）";
    let tail_tokens = estimate_tokens(TAIL);
    let mut low = 0usize;
    let mut high = text.len();
    while low < high {
        let mid = low + (high - low + 1) / 2;
        let prefix = take_utf8_prefix(text, mid);
        if estimate_tokens(prefix) + tail_tokens <= max_tokens {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let prefix = take_utf8_prefix(text, low);
    let dropped = original_chars - prefix.chars().count();
    let mut fitted = prefix.to_string();
    fitted.push_str(&TAIL.replace('N', &dropped.to_string()));
    (fitted, dropped)
}

/// 规划内容指纹（账本用，短哈希）。
pub(crate) fn plan_hash(p: &PlanOutput) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(p)
        .unwrap_or_else(|_| "{}".to_string())
        .hash(&mut h);
    format!("{:016x}", h.finish())
}

/// 当前规划 outline 序列化的字符数（审计注入上下文量）。
pub(crate) fn outline_chars(s: &DualAgentSession) -> usize {
    s.plan
        .as_ref()
        .and_then(|p| serde_json::to_string(&p.outline).ok())
        .map(|v| v.chars().count())
        .unwrap_or(0)
}

pub(crate) fn foreshadow_count(s: &DualAgentSession) -> usize {
    s.plan.as_ref().map(|p| p.foreshadow_items.len()).unwrap_or(0)
}

pub(crate) fn plan_pending_confirmation(s: &DualAgentSession) -> bool {
    s.plan.as_ref().map(|p| p.state != "confirmed").unwrap_or(true)
}

