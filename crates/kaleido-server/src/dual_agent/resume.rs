//! dual_agent/resume — [2026-10-05 语义拆分 PH-B] 由 dual_agent/mod.rs 按设计文档
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

/// resume 时每个阶段最多连续执行的窗口数（避免一次 resume 消耗过多 LLM 额度）。
pub(crate) const RESUME_MAX_WINDOWS: usize = 2;

/// 执行真正的 resume 续跑逻辑，返回 (actions, llm_note)。
/// 按当前阶段分派：writing → review → styling → compression。
pub(crate) async fn run_resume_logic(state: &AppState, s: &mut DualAgentSession) -> (Vec<String>, String) {
    let mut actions = Vec::new();
    let mut note = String::new();

    // ── writing 阶段 ──
    if s.stage == "writing" && s.handoff_ok {
        // 先重置所有 running 卡住的窗口为 pending
        for win in s.windows.iter_mut() {
            if win.status == "running" {
                tracing::warn!(session_id=%s.id, window_id=%win.id, "resume: resetting stuck running window to pending");
                win.status = "pending".into();
            }
        }
        let llm = agent_llm(state);
        let prov_kind = agent_provider_kind(&state);
        let mut written_count = 0usize;
        for i in 0..s.windows.len() {
            if s.windows[i].status != "pending" {
                continue;
            }
            if written_count >= RESUME_MAX_WINDOWS {
                break; // 最多连续写 RESUME_MAX_WINDOWS 个窗口
            }
            // 标记为 running
            s.windows[i].status = "running".into();
            s.windows[i].assigned_role = Some(AGENT_WRITING.into());
            let win_snapshot = s.windows[i].clone();
            // 调用 Dante 写稿（带重试，LLM 失败回退启发式）
            let (draft, win_note) = dante_write_with_retry(&prov_kind, &llm, s, &win_snapshot).await;
            // 写入窗口
            s.windows[i].draft = Some(draft);
            s.windows[i].status = "written".into();
            s.windows[i].written_at = Some(now());
            note = win_note;
            actions.push("write_window".to_string());
            written_count += 1;
        }
        // 全部窗口写完自动完成 writing 阶段
        if s.windows.iter().all(|w| w.status == "written") {
            complete_stage(s, "writing", "全部写作窗口已完成，进入审稿", None);
            actions.push("complete_writing".to_string());
        }
        return (actions, note);
    }

    // ── review 阶段：所有窗口已 written 但 review 未完成 ──
    if s.windows.iter().all(|w| w.status == "written") && !stage_completed(s, "review") {
        start_stage(s, "review");
        let llm = agent_llm(state);
        let prov_kind = agent_provider_kind(&state);
        let has_llm = !llm.base_url.trim().is_empty() && !llm.api_key.trim().is_empty();
        let (review_items, review_note) = if !has_llm {
            (vec![], "LLM 未配置，跳过审稿".to_string())
        } else {
            match run_review_iter(&prov_kind, &llm, s).await {
                Ok(items) => (items, "AI 审稿完成（LLM 产出）".to_string()),
                Err(e) => {
                    tracing::warn!(session_id=%s.id, error=%e, "resume: review LLM failed");
                    (vec![], format!("审稿 LLM 失败：{e}"))
                }
            }
        };
        let major_count = review_items.iter().filter(|r| r.severity == "major").count();
        let minor_count = review_items.iter().filter(|r| r.severity == "minor").count();
        s.review = review_items;
        note = review_note;
        let msg = format!("审稿完成：{major_count} 个 major 问题，{minor_count} 个 minor 问题");
        complete_stage(s, "review", &msg, Some(json!({"major": major_count, "minor": minor_count})));
        actions.push("review".to_string());
        return (actions, note);
    }

    // ── styling 阶段：review 已完成但 styling 未完成 ──
    if stage_completed(s, "review") && !stage_completed(s, "styling") {
        start_stage(s, "styling");
        let llm = agent_llm(state);
        let prov_kind = agent_provider_kind(&state);
        let has_llm = !llm.base_url.trim().is_empty() && !llm.api_key.trim().is_empty();
        let mut styled = Vec::new();
        for win in &s.windows {
            if win.status != "written" || win.draft.is_none() {
                continue;
            }
            if has_llm {
                match run_styling_iter(&prov_kind, &llm, s, win).await {
                    Ok(styled_text) => {
                        styled.push(StyledWindow {
                            window_id: win.id.clone(),
                            styled_draft: styled_text,
                        });
                        note = "风格统一完成（LLM 产出）".into();
                    }
                    Err(e) => {
                        tracing::warn!(session_id=%s.id, window_id=%win.id, error=%e, "resume: styling LLM failed");
                        styled.push(StyledWindow {
                            window_id: win.id.clone(),
                            styled_draft: win.draft.clone().unwrap_or_default(),
                        });
                        note = format!("风格统一 LLM 失败（{e}），部分窗口原样保留");
                    }
                }
            } else {
                styled.push(StyledWindow {
                    window_id: win.id.clone(),
                    styled_draft: win.draft.clone().unwrap_or_default(),
                });
                note = "LLM 未配置，风格统一跳过（原样保留）".into();
            }
        }
        s.styled_windows = styled;
        complete_stage(s, "styling", "风格统一完成", None);
        actions.push("styling".to_string());
        return (actions, note);
    }

    // ── compression 阶段：styling 已完成但 compression 未完成 ──
    if stage_completed(s, "styling") && !stage_completed(s, "compression") {
        start_stage(s, "compression");
        let llm = agent_llm(state);
        let prov_kind = agent_provider_kind(&state);
        let has_llm = !llm.base_url.trim().is_empty() && !llm.api_key.trim().is_empty();
        let mut summaries = Vec::new();
        for win in &s.windows {
            if win.status != "written" || win.draft.is_none() {
                continue;
            }
            let summary_text = if has_llm {
                match run_compress_iter(&prov_kind, &llm, s, win).await {
                    Ok(text) => {
                        note = "章节摘要完成（LLM 产出）".into();
                        text
                    }
                    Err(e) => {
                        tracing::warn!(session_id=%s.id, window_id=%win.id, error=%e, "resume: compress LLM failed");
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
        complete_stage(s, "compression", "章节压缩完成", None);
        actions.push("compression".to_string());
        return (actions, note);
    }

    // ── context_assembly 阶段（规划中断）──
    if s.stage == "context_assembly" && s.plan.is_none() && !s.auto_confirm {
        // 规划需要用户输入，不自动重跑，仅清 error 返回 nextAction=run_plan
        note = "规划阶段中断，需要用户手动执行规划".into();
        return (actions, note);
    }

    (actions, note)
}

