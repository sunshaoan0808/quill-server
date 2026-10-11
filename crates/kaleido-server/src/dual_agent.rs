//! U12 双 Agent 分工与工作流（吞噬 Openwrite「Goethe 规划 → Dante 写作」+ 滚动规划 + 写作流程调度器）。
// [2026-10-05 语义拆分] 原单文件按 docs/plans/2026-10-05-p006-dual-agent-semantic-split.md §2.2
// 拆为目录模块族；D1-D3 已迁出 model/store/stage/budget/nl_protocol/goethe/dante/cot_strip。
//!
//! 角色模式：
//!   - planning（Goethe）：设定 / 大纲 / 伏笔规划，产出结构化 `PlanOutput`，规划完成后调用 handoff 交接。
//!   - writing（Dante）：消费写作窗口（`WritingWindow`）清单，接单撰写正文 / 审稿 / 运行态。
//!
//! 工作流状态机（持久化到 `data_root/dual-agent/{id}.json`，支持跨会话 resume）：
//!   STAGE_NAMES = [context_assembly, writing, review, user_confirm, styling, compression]
//!
//! Routes (all require session auth):
//!   GET  /api/v1/dual-agent/sessions                              list sessions
//!   POST /api/v1/dual-agent/sessions                              create planning session (work_id/book_id)
//!   GET  /api/v1/dual-agent/sessions/{id}                         get session
//!   POST /api/v1/dual-agent/sessions/{id}/plan                    run planning (Goethe: settings/outline/foreshadow)；携带 {instruction} 可多轮对话式迭代（U12-A1）
//!   GET  /api/v1/dual-agent/sessions/{id}/plan                    read planning output (+ pendingConfirmation)
//!   POST /api/v1/dual-agent/sessions/{id}/chat                    NL 会话：显式确认/交接触发，其余进入规划迭代（U12-A1/A2）
//!   POST /api/v1/dual-agent/sessions/{id}/confirm-plan            plan.state proposed → confirmed，确认后才允许 handoff（U12-A2）
//!   POST /api/v1/dual-agent/sessions/{id}/handoff                 planning done → writing windows → Dante takes over（U12-A3 完整协议）
//!   GET  /api/v1/dual-agent/sessions/{id}/windows                 writing window list
//!   POST /api/v1/dual-agent/sessions/{id}/windows/{wid}/write     Dante writes a window draft
//!   POST /api/v1/dual-agent/sessions/{id}/stage                   advance workflow stage (start/complete/fail/skip)
//!   POST /api/v1/dual-agent/sessions/{id}/resume                  resume interrupted session from persisted state
//!   GET  /api/v1/dual-agent/sessions/{id}/state                   current role/stage/progress
//!   GET  /api/v1/dual-agent/sessions/{id}/ledger                  ContextLedger 上下文账本（U12-A4/D3）

// [机械拆分产物 2026-10-05] 原头部 use 块现由子模块经 `use super::*` 共享，本文件
// 直接引用减少属预期；cargo fix 会误判未使用并删除（工程纪律见 REFACTOR_20260927.md）。
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


// [2026-10-05 语义拆分] 子模块声明 + pub(crate) use 汇总（tests 的 use super::* 经此解析）：
mod model;
mod store;
mod stage;
mod budget;
mod nl_protocol;
mod goethe;
mod dante;
mod cot_strip;
pub(crate) use model::*;
pub(crate) use store::*;
pub(crate) use stage::*;
pub(crate) use budget::*;
pub(crate) use nl_protocol::*;
pub(crate) use goethe::*;
pub(crate) use dante::*;
pub(crate) use cot_strip::*;
mod review;
mod handoff;
mod harness_hook;
mod handlers;
mod resume;
pub(crate) use review::*;
pub(crate) use handoff::*;
pub(crate) use harness_hook::*;
pub(crate) use handlers::*;
pub(crate) use resume::*;

/// 双 Agent 角色名（会话级切换）。
pub const AGENT_PLANNING: &str = "planning";
pub const AGENT_WRITING: &str = "writing";

/// 写作流程调度阶段（对应 workflow_scheduler.py 的 STAGE_NAMES）。
pub const STAGE_NAMES: &[&str] = &[
    "context_assembly",
    "writing",
    "review",
    "user_confirm",
    "styling",
    "compression",
];

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/dual-agent/sessions",
            get(list_sessions_h).post(create_session_h),
        )
        .route("/api/v1/dual-agent/sessions/{id}", get(get_session_h))
        .route(
            "/api/v1/dual-agent/sessions/{id}/plan",
            post(plan_session_h).get(get_plan_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/chat",
            post(chat_session_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/confirm-plan",
            post(confirm_plan_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/handoff",
            post(handoff_session_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/windows",
            get(get_windows_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/windows/{window_id}/write",
            post(write_window_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/windows/{window_id}/generate",
            post(generate_window_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/windows/{window_id}/publish",
            post(publish_window_h),
        )
        .route("/api/v1/dual-agent/sessions/{id}/stage", post(advance_stage_h))
        .route(
            "/api/v1/dual-agent/sessions/{id}/review",
            post(review_session_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/styling",
            post(styling_session_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/compress",
            post(compress_session_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/resume",
            post(resume_session_h),
        )
        .route(
            "/api/v1/dual-agent/sessions/{id}/auto-confirm",
            post(set_auto_confirm_h),
        )
        .route("/api/v1/dual-agent/sessions/{id}/state", get(get_state_h))
        .route(
            "/api/v1/dual-agent/sessions/{id}/ledger",
            get(get_ledger_h),
        )
}

// ── 数据结构 ─────────────────────────────────────────────────────────────

fn agent_llm(state: &AppState) -> LlmRuntime {
    state.app_state.resolve_llm(
        state.llm_base.as_deref(),
        state.llm_key.as_deref(),
        &state.llm_model,
    )
}

/// F4: dual_agent 各迭代函数共用的 provider kind（managed protocol > env 默认）。
/// 迭代函数只拿 `&LlmRuntime`，kind 随调用点解析后透传 dispatch。
fn agent_provider_kind(state: &AppState) -> String {
    crate::llm_stream::runtime_provider_kind(&agent_llm(state), &state.provider_kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_session() -> DualAgentSession {
        DualAgentSession {
            id: "dual-agent-test".into(),
            work_id: "w".into(),
            book_id: None,
            title: "t".into(),
            active_role: AGENT_PLANNING.into(),
            stage: STAGE_NAMES[0].into(),
            plan: None,
            windows: vec![],
            stages: STAGE_NAMES.iter().map(|n| StageRecord::new(n)).collect(),
            transcript: vec![],
            chat_transcript: vec![],
            context_ledger: vec![],
            handoff_ok: false,
            llm_note: String::new(),
            created_at: "x".into(),
            updated_at: "x".into(),
            error: String::new(),
            workspace_id: "ws".into(),
            review: vec![],
            summaries: vec![],
            styled_windows: vec![],
            auto_confirm: false,
        }
    }

    #[test]
    fn u12a_fit_text_under_budget_is_passthrough() {
        let (fitted, dropped) = fit_text_to_token_budget("short text", 1000);
        assert_eq!(fitted, "short text");
        assert_eq!(dropped, 0);
    }

    #[test]
    fn stage_completed_detects_completed() {
        let mut s = test_session();
        assert!(!stage_completed(&s, "writing"));
        complete_stage(&mut s, "writing", "done", None);
        assert!(stage_completed(&s, "writing"));
    }

    #[test]
    fn truncate_str_handles_utf8_and_short() {
        assert_eq!(truncate_str("短", 5), "短");
        let long = "字".repeat(300);
        let out = truncate_str(&long, 100);
        assert!(out.chars().count() <= 101); // 100 + 省略号
        assert!(out.ends_with('…'));
    }

    #[test]
    fn sanitize_filename_blocks_traversal_and_spaces() {
        assert_eq!(sanitize_filename("../etc/passwd"), "etcpasswd");
        assert_eq!(sanitize_filename("第1章 醒来"), "第1章醒来");
        assert_eq!(sanitize_filename("a b/c"), "abc");
        assert_eq!(sanitize_filename("..."), "");
        assert_eq!(sanitize_filename("普通章节"), "普通章节");
    }

    #[test]
    fn dante_prompt_forbids_repeating_previous_drafts() {
        // 构造一个已写窗口 + 目标窗口，验证 prompt 含「不要重复」指令
        let mut s = test_session();
        s.plan = Some(PlanOutput {
            direction: "测试方向".into(),
            ..Default::default()
        });
        s.windows.push(WritingWindow {
            id: "win-01".into(),
            chapter_id: "第1章".into(),
            title: "已写章".into(),
            status: "written".into(),
            outline: String::new(),
            prompt: String::new(),
            word_target: 2000,
            assigned_role: Some(AGENT_WRITING.into()),
            draft: Some("这是已写章节的正文。".into()),
            written_at: Some(now()),
            sub_tasks: vec![],
        });
        s.windows.push(WritingWindow {
            id: "win-02".into(),
            chapter_id: "第2章".into(),
            title: "目标章".into(),
            status: "pending".into(),
            outline: String::new(),
            prompt: String::new(),
            word_target: 2000,
            assigned_role: None,
            draft: None,
            written_at: None,
            sub_tasks: vec![],
        });
        let prompt = build_dante_user_prompt(&s, &s.windows[1]);
        assert!(prompt.contains("不要重复已有章节"), "prompt 应禁止重复: {prompt}");
        assert!(prompt.contains("严禁复用"), "prompt 应含严禁复用: {prompt}");
        assert!(prompt.contains("已写章节的正文"), "prompt 应含已有章节内容");
    }

    #[test]
    fn u12a_fit_text_truncates_long_text_with_tail_summary() {
        let long = "第".repeat(5000) + "章";
        let (fitted, dropped) = fit_text_to_token_budget(&long, 100);
        assert!(dropped > 0, "expected chars dropped");
        assert!(fitted.len() < long.len());
        assert!(fitted.contains("已省略"));
        assert!(fitted.starts_with("第"));
    }

    #[test]
    fn u12a_fit_text_zero_budget_drops_all() {
        let (fitted, dropped) = fit_text_to_token_budget("abc", 0);
        assert_eq!(fitted, "");
        assert_eq!(dropped, 3);
    }

    #[test]
    fn u12a_confirm_detection_markers() {
        assert!(looks_like_explicit_confirm("没问题，就按这个吧"));
        assert!(looks_like_explicit_confirm("确认"));
        assert!(looks_like_explicit_confirm("OK"));
        assert!(looks_like_explicit_confirm("好的，确认应用这版"));
        assert!(!looks_like_explicit_confirm("先不确认，再讨论一下"));
        assert!(!looks_like_explicit_confirm("帮我看下大纲"));
    }

    #[test]
    fn u12a_handoff_detection_markers() {
        assert!(looks_like_handoff_request("交接给 Dante 吧"));
        assert!(looks_like_handoff_request("开始写正文"));
        assert!(looks_like_handoff_request("交给 Dante 来写"));
        assert!(!looks_like_handoff_request("确认大纲没问题"));
    }

    #[test]
    fn u12a_handoff_blocked_without_confirmation() {
        let mut s = test_session();
        s.plan = Some(PlanOutput {
            outline: vec![json!({"chapter": "第1章"})],
            ..Default::default()
        });
        let proto = handoff_check(&s);
        assert!(proto.blocked);
        assert_eq!(proto.next_action, "confirm_plan");
        assert!(proto.missing_items.contains(&"plan_confirmation".to_string()));
    }

    #[test]
    fn u12a_handoff_blocked_missing_outline_and_foreshadow() {
        let mut s = test_session();
        let mut p = PlanOutput::default();
        p.state = "confirmed".into();
        s.plan = Some(p);
        let proto = handoff_check(&s);
        assert!(proto.blocked);
        assert!(proto.missing_items.contains(&"outline".to_string()));
        assert!(proto.missing_items.contains(&"foreshadowItems".to_string()));
        assert_eq!(proto.next_action, "fill_outline");
    }

    #[test]
    fn u12a_handoff_blocked_no_plan() {
        let s = test_session();
        let proto = handoff_check(&s);
        assert!(proto.blocked);
        assert!(proto.missing_items.contains(&"outline".to_string()));
        assert_eq!(proto.next_action, "fill_outline");
    }

    #[test]
    fn u12a_handoff_ok_when_complete_and_confirmed() {
        let mut s = test_session();
        let mut p = PlanOutput::default();
        p.state = "confirmed".into();
        p.outline = vec![json!({"chapter": "第1章", "title": "开端", "goal": "g"})];
        p.foreshadow_items = vec![json!({"id": "f1", "desc": "d"})];
        s.plan = Some(p);
        let proto = handoff_check(&s);
        assert!(!proto.blocked);
        assert!(proto.ok);
        assert_eq!(proto.next_action, "start_writing");
        assert!(proto.missing_items.is_empty());
    }

    #[test]
    fn u12a_next_action_requires_confirmation() {
        let mut s = test_session();
        s.plan = Some(PlanOutput::default());
        assert_eq!(next_action(&s), "confirm_plan");
    }

    #[test]
    fn u12a_heuristic_plan_keeps_handoff_walkable() {
        // U12 承诺：LLM 未配置时启发式规划仍可端到端走通（confirm→handoff→windows）。
        let mut s = test_session();
        let mut p = heuristic_plan(&s);
        assert_eq!(p.foreshadow_items.len(), 1, "启发式规划需含占位伏笔");
        p.state = "confirmed".into();
        s.plan = Some(p);
        let proto = handoff_check(&s);
        assert!(!proto.blocked);
        assert_eq!(proto.next_action, "start_writing");
    }

    #[test]
    fn u12a_ledger_records_entries() {
        let mut s = test_session();
        s.plan = Some(PlanOutput {
            outline: vec![json!({"chapter": "第1章"})],
            foreshadow_items: vec![json!({"id": "f1"})],
            ..Default::default()
        });
        let o_chars = outline_chars(&s);
        let f_count = foreshadow_count(&s);
        ledger_push(&mut s, "plan", o_chars, f_count);
        ledger_push(&mut s, "handoff", o_chars, f_count);
        assert_eq!(s.context_ledger.len(), 2);
        assert_eq!(s.context_ledger[0].stage, "plan");
        assert_eq!(s.context_ledger[1].plan_hash, s.context_ledger[0].plan_hash);
        assert!(s.context_ledger[0].outline_chars > 0);
        assert_eq!(s.context_ledger[0].foreshadow_count, 1);
    }

    #[test]
    fn u12a_plan_proposed_by_default_on_load() {
        // 旧存档无 review/summaries/styled_windows 字段时 serde default 兼容
        let raw = serde_json::json!({
            "id": "dual-agent-test",
            "workId": "w",
            "title": "t",
            "activeRole": "planning",
            "stage": "context_assembly",
            "handoffOk": false,
            "createdAt": "x",
            "updatedAt": "x",
            "workspaceId": "ws",
        });
        let s: DualAgentSession = serde_json::from_value(raw).unwrap();
        assert!(s.review.is_empty());
        assert!(s.summaries.is_empty());
        assert!(s.styled_windows.is_empty());
    }

    #[test]
    fn dante_prompt_contains_outline_and_foreshadow() {
        let mut s = test_session();
        let mut p = PlanOutput::default();
        p.direction = "全书方向测试".into();
        p.settings = vec![serde_json::json!({"key": "世界观", "value": "奇幻大陆"})];
        p.outline = vec![serde_json::json!({"chapter": "第1章", "title": "开端", "goal": "引出主角", "characters": ["李明"]})];
        p.foreshadow_items = vec![serde_json::json!({"id": "f1", "desc": "神秘符文", "plantChapter": "第1章", "payoffChapter": "第5章"})];
        s.plan = Some(p);
        s.windows = vec![WritingWindow {
            id: "win-01".into(),
            chapter_id: "第1章".into(),
            title: "开端".into(),
            status: "pending".into(),
            outline: "引出主角与核心冲突".into(),
            prompt: String::new(),
            word_target: 2000,
            assigned_role: None,
            draft: None,
            written_at: None,
            sub_tasks: vec![],
        }];
        let prompt = build_dante_user_prompt(&s, &s.windows[0]);
        assert!(prompt.contains("全书方向测试"));
        assert!(prompt.contains("奇幻大陆"));
        assert!(prompt.contains("神秘符文"));
        assert!(prompt.contains("开端"));
    }

    #[test]
    fn review_item_serde_roundtrip() {
        let item = ReviewItem {
            severity: "major".into(),
            issue: "情节矛盾".into(),
            window_id: "win-01".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        let back: ReviewItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.severity, "major");
        assert_eq!(back.issue, "情节矛盾");
    }

    #[test]
    fn chapter_summary_serde_roundtrip() {
        let s = ChapterSummary {
            window_id: "win-01".into(),
            chapter_id: "ch01".into(),
            summary: "主角出场".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ChapterSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.summary, "主角出场");
    }

    #[test]
    fn fit_text_to_token_budget_does_not_panic() {
        // 确保各种边界条件不 panic
        let (_, _) = fit_text_to_token_budget("", 100);
        let (_, _) = fit_text_to_token_budget("短文本", 0);
        let long = "章".repeat(10000);
        let (fitted, dropped) = fit_text_to_token_budget(&long, 100);
        assert!(dropped > 0);
        assert!(fitted.contains("已省略"));
    }

    #[test]
    fn heuristic_compress_basic() {
        assert_eq!(heuristic_compress(""), "");
        assert_eq!(heuristic_compress("短"), "短");
        let long = "字".repeat(500);
        let result = heuristic_compress(&long);
        assert!(result.len() <= 202); // 200 chars + ellipsis
        assert!(result.ends_with('…'));
    }

    #[test]
    fn heuristic_dante_draft_contains_context() {
        let mut s = test_session();
        let mut p = PlanOutput::default();
        p.direction = "测试方向".into();
        s.plan = Some(p);
        let win = WritingWindow {
            id: "win-01".into(),
            chapter_id: "ch01".into(),
            title: "第一章".into(),
            status: "pending".into(),
            outline: "测试大纲".into(),
            prompt: String::new(),
            word_target: 2000,
            assigned_role: None,
            draft: None,
            written_at: None,
            sub_tasks: vec![],
        };
        let draft = heuristic_dante_draft(&s, &win);
        assert!(draft.contains("第一章"));
        assert!(draft.contains("测试方向"));
    }

    #[test]
    fn serde_compatibility_old_archive_no_new_fields() {
        // 模拟旧存档（无 review/summaries/styled_windows）
        let raw = serde_json::json!({
            "id": "dual-agent-old",
            "workId": "w",
            "title": "old",
            "activeRole": "planning",
            "stage": "context_assembly",
            "handoffOk": false,
            "createdAt": "x",
            "updatedAt": "x",
            "workspaceId": "ws",
            "plan": {"direction": "d", "state": "confirmed", "outline": [{"chapter": "1"}], "foreshadowItems": [{"id": "f1"}]}
        });
        let s: DualAgentSession = serde_json::from_value(raw).unwrap();
        assert!(s.review.is_empty());
        assert!(s.summaries.is_empty());
        assert!(s.styled_windows.is_empty());
        // 确保 handoff check 仍然通过
        let proto = handoff_check(&s);
        assert!(proto.ok);
    }

    // ── 2026-08-16 修复回归测试：CoT 剥离 + 查重 gate ─────────────────────

    #[test]
    fn strip_dante_cot_removes_tail_planning() {
        // 复现 afb win-03：正文末尾混入 LLM 自我规划（CoT 泄漏）
        let input = "病房里的白炽灯管嗡嗡作响，父亲坐在床边，母亲端着一壶热水走进来。\n\
                     她总觉得，她在哭。\n\
                     好的，我需要写第3章「跌倒与尿尿」，先规划结构：开头写病房场景，\
                     中间写李昊跌倒，结尾写尿床。字数控制在2000字左右。要写出场景感、对话、人物情绪。开始写正文。";
        let out = strip_dante_cot(input);
        // 剥离后不应残留规划文本
        assert!(!out.contains("好的，我需要写"), "planning tail not stripped: {out}");
        assert!(!out.contains("结构规划"), "planning tail not stripped: {out}");
        assert!(!out.contains("字数控制"), "planning tail not stripped: {out}");
        assert!(!out.contains("开始写正文"), "planning tail not stripped: {out}");
        // 正文开头应保留
        assert!(out.contains("病房里的白炽灯管"), "body head lost: {out}");
        assert!(out.contains("她在哭"), "body tail lost: {out}");
    }

    #[test]
    fn strip_dante_cot_removes_head_planning() {
        // 复现 cba 第1章：正文开头混入思考（"好的，用户需要我写第1章…让我仔细分析"）
        let input = "好的，用户需要我写第1章「异常简历」，让我仔细分析一下要求。\
                     这是一篇关于程序员在体检中心发现异常的悬疑网文。\
                     开始写正文。\n\
                     窗外的雨敲打着玻璃，李默盯着屏幕上的体检报告，手指微微发颤。";
        let out = strip_dante_cot(input);
        assert!(!out.contains("好的，用户需要"), "head CoT not stripped: {out}");
        assert!(!out.contains("让我仔细分析"), "head CoT not stripped: {out}");
        assert!(out.contains("窗外的雨敲打"), "body lost: {out}");
    }

    #[test]
    fn strip_dante_cot_keeps_clean_text() {
        // 无 CoT 的干净正文应原样保留
        let clean = "夜色渐深，她关掉台灯，把手机放到枕边。今天的一切都像梦一样。";
        assert_eq!(strip_dante_cot(clean), clean);
    }

    #[test]
    fn extract_json_array_handles_fenced_and_nested() {
        // 纯 JSON 数组
        assert_eq!(extract_json_array("[{\"id\":1}]").unwrap(), "[{\"id\":1}]");
        // 前后有说明文字
        assert_eq!(
            extract_json_array("好的，分解如下：[{\"id\":\"t1\"}] 请接续。").unwrap(),
            "[{\"id\":\"t1\"}]"
        );
        // ```json 围栏 + 嵌套数组
        let fenced = "```json\n[{\"id\":\"t1\",\"deps\":[]},{\"id\":\"t2\",\"deps\":[\"t1\"]}]\n```";
        assert_eq!(
            extract_json_array(fenced).unwrap(),
            "[{\"id\":\"t1\",\"deps\":[]},{\"id\":\"t2\",\"deps\":[\"t1\"]}]"
        );
        // 字符串内出现 ] 不应提前截断
        let inner_bracket = "[{\"goal\":\"他（完档）」结束\"}]";
        assert_eq!(extract_json_array(inner_bracket).unwrap(), inner_bracket);
        // 无数组 → None
        assert!(extract_json_array("没有数组的回复").is_none());
    }

    #[test]
    fn extract_json_array_unbalanced_returns_none() {
        // 只有 `[` 没有配对 `]`
        assert!(extract_json_array("[{\"id\":1}").is_none());
    }

    #[test]
    fn overlap_rate_detects_copy_and_distinct() {
        // 复现 afb win-04 整段复制 win-03 开头
        let win3 = "病房里的白炽灯管嗡嗡作响，父亲坐在床边，母亲端着一壶热水走进来。\n她总觉得，她在哭。";
        let win4_copy = "病房里的白炽灯管嗡嗡作响，父亲坐在床边，母亲端着一壶热水走进来。\n但她总觉得，她在哭。\n门被推开。";
        let win4_orig = "清晨的走廊里，护士推着药车，铁轮碾过水磨石地面，发出刺耳的声响。";
        let rate_copy = dante_overlap_rate(win4_copy, win3);
        let rate_orig = dante_overlap_rate(win4_orig, win3);
        assert!(rate_copy > 0.5, "copy should be flagged, got {rate_copy}");
        assert!(rate_orig < 0.2, "distinct should pass, got {rate_orig}");
    }

    #[test]
    fn max_overlap_picks_written_window_only() {
        let mut s = test_session();
        s.windows = vec![
            WritingWindow {
                id: "w1".into(),
                chapter_id: "1".into(),
                title: "一".into(),
                status: "written".into(),
                outline: "".into(),
                prompt: String::new(),
                word_target: 2000,
                assigned_role: None,
                draft: Some("病房里的白炽灯管嗡嗡作响，父亲坐在床边。".into()),
                written_at: None,
                sub_tasks: vec![],
            },
            WritingWindow {
                id: "w2".into(),
                chapter_id: "2".into(),
                title: "二".into(),
                status: "running".into(), // 未写完的不参与查重
                outline: "".into(),
                prompt: String::new(),
                word_target: 2000,
                assigned_role: None,
                draft: Some("病房里的白炽灯管嗡嗡作响，父亲坐在床边。".into()),
                written_at: None,
                sub_tasks: vec![],
            },
            WritingWindow {
                id: "w3".into(),
                chapter_id: "3".into(),
                title: "三".into(),
                status: "pending".into(),
                outline: "".into(),
                prompt: String::new(),
                word_target: 2000,
                assigned_role: None,
                draft: None,
                written_at: None,
                sub_tasks: vec![],
            },
        ];
        let cur = WritingWindow {
            id: "w3".into(),
            chapter_id: "3".into(),
            title: "三".into(),
            status: "running".into(),
            outline: "".into(),
            prompt: String::new(),
            word_target: 2000,
            assigned_role: None,
            draft: Some("病房里的白炽灯管嗡嗡作响，父亲坐在床边。".into()),
            written_at: None,
            sub_tasks: vec![],
        };
        let res = dante_max_overlap(&s, &cur, "病房里的白炽灯管嗡嗡作响，父亲坐在床边。");
        let (rate, dup) = res.expect("should find a dup against written w1");
        assert!(rate > 0.5, "overlap rate too low: {rate}");
        assert_eq!(dup, "w1", "should only match the written window, got {dup}");
    }
}
