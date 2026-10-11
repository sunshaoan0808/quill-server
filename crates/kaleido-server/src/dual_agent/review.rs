//! dual_agent/review — [2026-10-05 语义拆分 PH-B] 由 dual_agent/mod.rs 按设计文档
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

/// 审稿指令（REVIEWER_SYS）。
pub(crate) const REVIEWER_SYS: &str = "你是 Kaleido 的审稿编辑 Agent。\n\
你的职责：审阅已写章节，找出写作问题（情节矛盾、人物不一致、节奏问题、语言问题等）。\n\
输出格式：JSON 数组，每个元素为 {\"severity\":\"major\"|\"minor\",\"issue\":\"问题描述\",\"windowId\":\"对应窗口ID\"}。\n\
只输出 JSON 数组，不输出任何其他文字。\n\
severity 标准：\n\
- major：情节硬伤、人物性格前后矛盾、逻辑不通（必须修改）；\n\
- minor：语言可改进、节奏可优化、细节可丰富（建议修改）。";

/// P1：构造审稿 user prompt。
pub(crate) fn build_reviewer_user_prompt(s: &DualAgentSession) -> String {
    let mut parts = Vec::new();
    parts.push(format!("【作品】{}\n全书方向：{}", s.title,
        s.plan.as_ref().map(|p| p.direction.as_str()).unwrap_or("未定")));

    // 已写窗口的 draft（token 预算裁剪）
    let drafts: Vec<String> = s
        .windows
        .iter()
        .filter(|w| w.status == "written")
        .filter_map(|w| {
            w.draft.as_ref().map(|d| {
                format!("【{}（{}）】\n{}", w.title, w.id, d)
            })
        })
        .collect();
    if drafts.is_empty() {
        parts.push("（尚无已写章节，请输出空数组 []）".to_string());
    } else {
        let all = drafts.join("\n\n");
        let (fitted, dropped) = fit_text_to_token_budget(&all, 8000);
        let mut header = "【已写章节正文】".to_string();
        if dropped > 0 {
            header.push_str(&format!("（已裁剪约 {dropped} 字符）"));
        }
        parts.push(format!("{header}\n{fitted}"));
    }

    parts.push("请输出审稿结果 JSON 数组。".to_string());
    parts.join("\n\n")
}

/// P1：AI 审稿（调用 LLM，返回审稿条目列表）。
pub(crate) async fn run_review_iter(
    prov_kind: &str,
    llm: &LlmRuntime,
    s: &DualAgentSession,
) -> Result<Vec<ReviewItem>, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let user = build_reviewer_user_prompt(s);
    let text = chat_completion_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        prov_kind,
        REVIEWER_SYS,
        &user,
        0.1, 16384, 2000,
        &client,
    )
    .await?;
    let v = extract_json_value(&text).ok_or_else(|| "review response contains no JSON".to_string())?;
    let items: Vec<ReviewItem> =
        serde_json::from_value(v).map_err(|e| format!("review json mismatch: {e}"))?;
    Ok(items)
}

// ── P1 风格统一引擎 ─────────────────────────────────────────────────────

/// 风格统一指令。
pub(crate) const STYLING_SYS: &str = "你是 Kaleido 的风格润色 Agent。\n\
你的职责：对已写章节进行风格统一，使全书文风一致。\n\
要求：\n\
1. 保持原有剧情和人物不变；\n\
2. 统一叙事视角、用词风格、句式节奏；\n\
3. 输出完整润色后的章节正文（纯正文，不带标题和元注释）；\n\
4. 保持原有长度，不要大幅删减。";

/// P1：构造风格化 user prompt。
pub(crate) fn build_styling_user_prompt(s: &DualAgentSession, win: &WritingWindow) -> String {
    let mut parts = Vec::new();

    // 风格指令（从 settings 取）
    let mut style_directives = Vec::new();
    if let Some(p) = s.plan.as_ref() {
        for sv in &p.settings {
            let key = sv.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let val = sv.get("value").and_then(|v| v.as_str()).unwrap_or("");
            if key.contains("文风") || key.contains("风格") || key.contains("叙事") || key.contains("视角") {
                style_directives.push(format!("  - {key}：{val}"));
            }
        }
    }
    if style_directives.is_empty() {
        parts.push("【默认风格指令】\n网文长篇风格，叙事流畅，对话自然，节奏明快。".to_string());
    } else {
        parts.push(format!("【风格指令】\n{}", style_directives.join("\n")));
    }

    // 窗口 draft
    if let Some(draft) = &win.draft {
        parts.push(format!("【原文】\n章节：{}\n\n{}", win.title, draft));
    }

    parts.push("请输出风格统一后的完整正文。".to_string());
    parts.join("\n\n")
}

/// P1：LLM 不可用时的兜底（原样返回）。
#[allow(dead_code)] // [P7] 启发式样式兜底预留（LLM styling 主路径）
pub(crate) fn heuristic_styling(draft: &str) -> String {
    draft.to_string()
}

// ── P1 章节压缩引擎 ─────────────────────────────────────────────────────

/// 压缩指令。
pub(crate) const COMPRESS_SYS: &str = "你是 Kaleido 的摘要 Agent。\n\
你的职责：对章节正文生成简洁摘要（约 100-200 字），保留核心情节、人物行为和关键信息。\n\
输出格式：JSON {\"summary\":\"摘要内容\"}。\n\
只输出 JSON，不输出其他文字。";

/// P1：构造压缩 user prompt。
pub(crate) fn build_compress_user_prompt(_s: &DualAgentSession, win: &WritingWindow) -> String {
    let draft = win.draft.as_deref().unwrap_or("（无正文）");
    let (fitted, dropped) = fit_text_to_token_budget(draft, 4000);
    let mut text = format!("【章节】{}（{}）\n\n{}", win.title, win.chapter_id, fitted);
    if dropped > 0 {
        text.push_str(&format!("\n（已裁剪约 {dropped} 字符）"));
    }
    text.push_str("\n\n请生成简洁摘要 JSON。");
    text
}

/// P1：LLM 不可用时的兜底（截断前 200 字节，安全按字符边界，不 panic）。
pub(crate) fn heuristic_compress(draft: &str) -> String {
    let trimmed = draft.trim();
    if trimmed.len() <= 200 {
        return trimmed.to_string();
    }
    // 从 200 字节处回退到最近的字符边界（中文 3 字节，避免切半个字）
    let mut end = 200;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &trimmed[..end])
}

/// P1：风格统一（LLM 调用）。
pub(crate) async fn run_styling_iter(
    prov_kind: &str,
    llm: &LlmRuntime,
    s: &DualAgentSession,
    win: &WritingWindow,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let user = build_styling_user_prompt(s, win);
    chat_completion_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        prov_kind,
        STYLING_SYS,
        &user,
        0.1, 16384, 2000,
        &client,
    )
    .await
}

/// P1：章节压缩（LLM 调用，返回摘要文本）。
pub(crate) async fn run_compress_iter(
    prov_kind: &str,
    llm: &LlmRuntime,
    s: &DualAgentSession,
    win: &WritingWindow,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let user = build_compress_user_prompt(s, win);
    let text = chat_completion_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        prov_kind,
        COMPRESS_SYS,
        &user,
        0.1, 16384, 500,
        &client,
    )
    .await?;
    // 尝试从 JSON 提取 summary
    if let Some(v) = extract_json_value(&text) {
        if let Some(summary) = v.get("summary").and_then(|s| s.as_str()) {
            return Ok(summary.to_string());
        }
    }
    // fallback: 直接返回原文
    Ok(text)
}

