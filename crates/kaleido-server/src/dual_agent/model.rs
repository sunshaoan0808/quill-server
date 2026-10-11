//! dual_agent/model — [2026-10-05 语义拆分] 由 dual_agent.rs 按设计文档
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

/// 阶段执行记录（status: pending / running / completed / failed / skipped）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageRecord {
    pub name: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub started_at: String,
    #[serde(default)]
    pub completed_at: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub data: Value,
}

/// Goethe 规划产出（设定 / 大纲 / 伏笔 / 滚动窗口）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanOutput {
    #[serde(default)]
    pub settings: Vec<Value>,
    #[serde(default)]
    pub direction: String,
    #[serde(default)]
    pub outline: Vec<Value>,
    #[serde(default)]
    pub foreshadow_items: Vec<Value>,
    #[serde(default)]
    pub current_arc: String,
    #[serde(default)]
    pub current_window: Vec<String>,
    #[serde(default)]
    pub next_window: Vec<String>,
    #[serde(default)]
    pub next_arc_goals: Vec<String>,
    #[serde(default)]
    pub arc_summary: String,
    #[serde(default)]
    pub note: String,
    /// 提案状态：proposed（待确认） / confirmed（已确认，确认后才允许 handoff）。
    #[serde(default = "default_plan_state")]
    pub state: String,
}

pub(crate) fn default_plan_state() -> String {
    "proposed".into()
}

/// 写作窗口：Dante 接单的最小正文单元。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WritingWindow {
    pub id: String,
    pub chapter_id: String,
    pub title: String,
    /// pending | assigned | written | reviewed
    pub status: String,
    #[serde(default)]
    pub outline: String,
    #[serde(default)]
    pub prompt: String,
    pub word_target: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written_at: Option<String>,
    /// [morphling WriteHERE P1 2026-08-19] 原子写作子任务 DAG。
    /// 空 = 旧单次写稿（零回归）；非空 = Dante 按拓扑序逐任务生成，依赖产出注入。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_tasks: Vec<WindowTask>,
}

/// [morphling WriteHERE P1 2026-08-19] 写作窗口的原子化子任务（递归 think/write DAG）。
/// - kind="think"：产出设计/分析/节拍决策（不直接进正文，仅作为依赖上下文）。
/// - kind="write"：产出正文片段，依赖的 think/write 产出会注入其 prompt。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowTask {
    pub id: String,
    /// think | write
    pub kind: String,
    pub goal: String,
    /// 依赖的其他任务 id（先行产出注入本任务 prompt）
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub word_target: u32,
    /// pending | done
    #[serde(default)]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
}

/// 会话内对话记录（规划讨论 / 交接提示）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTurn {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub at: String,
}

// ── P1 审稿 / 风格 / 压缩 数据结构 ─────────────────────────────────────

/// 审稿条目（REVIEWER_SYS 输出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    /// "major" | "minor"
    pub severity: String,
    pub issue: String,
    #[serde(default)]
    pub window_id: String,
}

/// 章节压缩摘要条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterSummary {
    pub window_id: String,
    pub chapter_id: String,
    pub summary: String,
}

/// P1: 风格化窗口快照（styling 阶段产出）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyledWindow {
    pub window_id: String,
    pub styled_draft: String,
}

/// 多轮对话式规划记录（U12-A1）：记录每轮的 user/assistant 消息与业务标签。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub at: String,
}

/// ContextLedger 上下文账本条目（U12-A4/D3）：审计每步真实注入上下文量。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLedgerEntry {
    pub stage: String,
    #[serde(default)]
    pub plan_hash: String,
    #[serde(default)]
    pub outline_chars: usize,
    #[serde(default)]
    pub foreshadow_count: usize,
    #[serde(default)]
    pub timestamp: String,
}

/// 双 Agent 会话（一等持久化资产，独立前缀 `dual-agent-*`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DualAgentSession {
    pub id: String,
    pub work_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_id: Option<String>,
    pub title: String,
    /// active_role: planning | writing
    pub active_role: String,
    /// 当前工作流阶段（STAGE_NAMES 之一）。
    pub stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanOutput>,
    #[serde(default)]
    pub windows: Vec<WritingWindow>,
    #[serde(default)]
    pub stages: Vec<StageRecord>,
    #[serde(default)]
    pub transcript: Vec<AgentTurn>,
    /// 多轮对话式规划记录（U12-A1）。
    #[serde(default)]
    pub chat_transcript: Vec<ChatTurn>,
    /// ContextLedger 上下文账本（U12-A4/D3）。
    #[serde(default)]
    pub context_ledger: Vec<ContextLedgerEntry>,
    pub handoff_ok: bool,
    #[serde(default)]
    pub llm_note: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub error: String,
    pub workspace_id: String,
    /// P1: 审稿结果（review 阶段产出）。
    #[serde(default)]
    pub review: Vec<ReviewItem>,
    /// P1: 章节压缩摘要（compression 阶段产出）。
    #[serde(default)]
    pub summaries: Vec<ChapterSummary>,
    /// P1: 风格化后的窗口 drafts（styling 阶段产出，None 表示未风格化）。
    #[serde(default)]
    pub styled_windows: Vec<StyledWindow>,
    /// P1: 自动确认规划提案（开启后 plan 产出自动 confirmed，跳过手动确认）。
    #[serde(default)]
    pub auto_confirm: bool,
}

// ── 请求体 ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateSessionBody {
    pub(crate) work_id: String,
    #[serde(default)]
    pub(crate) book_id: Option<String>,
    #[serde(default)]
    pub(crate) title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WriteWindowBody {
    pub(crate) content: String,
}

/// P0: AI 写稿请求体（可选字段，若 generate=true 则调用 LLM）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // [P7] generate=true 写稿请求体预留
pub(crate) struct GenerateWindowBody {
    #[serde(default)]
    pub generate: bool,
}

/// 多轮规划请求体（U12-A1）：instruction 为空且已有规划时幂等返回。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlanBody {
    #[serde(default)]
    pub(crate) instruction: Option<String>,
}

/// 自然语言会话请求体（U12-A1/A2）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatBody {
    pub(crate) message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StageBody {
    pub(crate) name: String,
    /// start | complete | fail | skip
    #[serde(default = "default_stage_status")]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) data: Option<Value>,
}

pub(crate) fn default_stage_status() -> String {
    "complete".into()
}

/// 自动确认开关请求体。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoConfirmBody {
    pub(crate) enabled: bool,
}

// ── 持久化（data_root/dual-agent/{id}.json）───────────────────────────────


impl StageRecord {
    pub(crate) fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            status: "pending".into(),
            started_at: String::new(),
            completed_at: String::new(),
            message: String::new(),
            data: Value::Null,
        }
    }
}

