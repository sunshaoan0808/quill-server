//! dual_agent/goethe — [2026-10-05 语义拆分] 由 dual_agent.rs 按设计文档
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

/// Goethe 规划指令（产出 JSON 契约，参考 goethe.py 的规划职责）。
pub(crate) const GOETHE_SYS: &str = "你是 Kaleido 的 Goethe，长期会话规划 Agent。\
你的职责：汇总设定、收敛人物/设定/大纲、埋设伏笔，产出结构化规划资产。\
只输出 JSON，不输出 Markdown、不输出解释文字。\
JSON 结构必须为：{\"settings\":[{\"key\":\"...\",\"value\":\"...\"}],\
\"direction\":\"全书一句话方向\",\
\"outline\":[{\"chapter\":\"第1章\",\"title\":\"...\",\"goal\":\"本章目标/事件\",\"characters\":[\"角色名\"]}],\
\"foreshadowItems\":[{\"id\":\"f1\",\"desc\":\"...\",\"plantChapter\":\"第1章\",\"payoffChapter\":\"第3章\"}],\
\"currentArc\":\"当前弧\",\
\"currentWindow\":[\"已写章节\"],\
\"nextWindow\":[\"待规划章节\"],\
\"nextArcGoals\":[\"下一弧候选目标\"],\
\"arcSummary\":\"当前弧摘要\"}";

// ── context_assembly：真实上下文装配 ──────────────────────────────────────

/// 从作品目录装配真实上下文：已写章节 .md + outlines/ 下的大纲文件。
/// work_id 不安全或目录不存在时安全降级（返回空串），不 panic。
pub(crate) fn assemble_work_context(state: &AppState, s: &DualAgentSession) -> String {
    let ws_root = match state.works.workspace_root(&s.workspace_id) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };

    let safe_work_id = sanitize_work_id(&s.work_id);
    let work_dir = ws_root.join(&safe_work_id);
    // 如果 work_id 对应目录不存在，回退到 workspace_root 扫描
    let scan_dir = if work_dir.is_dir() { work_dir.clone() } else { ws_root.clone() };

    // ── a. 已写章节 .md 文件 ──
    let mut md_files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&scan_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() && p.extension().map(|e| e == "md").unwrap_or(false) {
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                if !is_backup_file(&name) {
                    md_files.push(p);
                }
            }
        }
    }
    md_files.sort();
    md_files.truncate(3); // 最多 3 个文件

    let mut chapters_text = String::new();
    for (i, path) in md_files.iter().enumerate() {
        if let Ok(content) = std::fs::read_to_string(path) {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let prefix = take_utf8_prefix(&content, 3000); // 每个文件前 3000 字符
            chapters_text.push_str(&format!(
                "【文稿 {}：{}】\n{}\n\n",
                i + 1,
                name,
                prefix
            ));
        }
    }

    // ── b. outlines 目录 ──
    let outline_dir = ws_root.join(&safe_work_id).join("outlines");
    let mut outline_text = String::new();
    if outline_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&outline_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    if let Ok(content) = std::fs::read_to_string(&p) {
                        let prefix = take_utf8_prefix(&content, 2000);
                        outline_text.push_str(&format!(
                            "【大纲文件：{}】\n{}\n\n",
                            p.file_name().unwrap_or_default().to_string_lossy(),
                            prefix
                        ));
                    }
                }
            }
        }
    }

    // ── c. 组装并裁剪 ──
    let mut combined = String::new();
    if !chapters_text.is_empty() {
        combined.push_str(&chapters_text);
    }
    if !outline_text.is_empty() {
        combined.push_str(&outline_text);
    }
    if combined.is_empty() {
        return String::new();
    }
    let (fitted, _) = fit_text_to_token_budget(&combined, 8000);
    fitted
}

pub(crate) fn goethe_brief(s: &DualAgentSession) -> String {
    let book = s.book_id.clone().unwrap_or_default();
    format!(
        "作品：{}（work_id={}，book_id={}）\n\
         请按 JSON 契约输出：基础设定（settings）、全书方向（direction）、章级大纲（outline，至少 2 章）、\
         伏笔清单（foreshadowItems）、当前弧（currentArc）、下一弧候选目标（nextArcGoals）、\
         以及已写/待写章节窗口（currentWindow / nextWindow）。\
         大纲草案只进入待确认区，后续由 Dante 按写作窗口接单撰写正文。",
        s.title, s.work_id, book
    )
}

pub(crate) fn parse_plan(text: &str) -> Result<PlanOutput, String> {
    let v = extract_json_value(text).ok_or_else(|| "plan response contains no JSON".to_string())?;
    serde_json::from_value::<PlanOutput>(v).map_err(|e| format!("plan json mismatch: {e}"))
}

/// A1：构造 Goethe 规划 prompt —— 首轮用作品简报；迭代轮把上一轮 PlanOutput
/// （token 预算内裁剪）与用户新增指示一起喂回。
/// `work_context`：context_assembly 装配的真实文稿上下文（可为空）。
pub(crate) fn build_goethe_user_prompt(s: &DualAgentSession, instruction: &str, work_context: &str) -> String {
    let brief = goethe_brief(s);
    let inst = instruction.trim();
    let mut parts = vec![format!("【作品信息】\n{brief}\n")];
    // ── 注入真实上下文 ──
    if !work_context.is_empty() {
        parts.push(format!("【作品真实内容】（来自作者区文稿，用于规划参考）\n{work_context}\n"));
    }
    if let Some(p) = s.plan.as_ref() {
        parts.push("【上一轮规划】（本次迭代输入，已按 token 预算裁剪，超预算部分省略）".to_string());
        let raw = serde_json::to_string_pretty(p).unwrap_or_else(|_| "{}".to_string());
        let (fitted, dropped) = fit_text_to_token_budget(&raw, PLAN_FEEDBACK_MAX_TOKENS);
        parts.push(fitted);
        if dropped > 0 {
            parts.push(format!("（上一轮规划裁剪省略约 {dropped} 字符）"));
        }
    }
    if !inst.is_empty() {
        parts.push(format!("【用户新增指示】\n{inst}\n"));
    }
    parts.push(
        "请基于以上信息迭代更新规划，并输出**完整**规划 JSON（契约字段不变：\
         settings/direction/outline/foreshadowItems/currentArc/currentWindow/nextWindow/\
         nextArcGoals/arcSummary）。"
            .to_string(),
    );
    parts.join("\n")
}

pub(crate) async fn run_goethe_plan_iter(
    prov_kind: &str,
    llm: &LlmRuntime,
    s: &DualAgentSession,
    instruction: &str,
    work_context: &str,
) -> Result<PlanOutput, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let user = build_goethe_user_prompt(s, instruction, work_context);
    let text = chat_completion_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        prov_kind,
        GOETHE_SYS,
        &user,
        0.1, 16384, 300,
        &client,
    )
    .await?;
    parse_plan(&text)
}

/// LLM 不可用时的启发式占位规划：保证状态机可端到端走通（handoff → 写作窗口）。
pub(crate) fn heuristic_plan(s: &DualAgentSession) -> PlanOutput {
    PlanOutput {
        settings: vec![
            json!({"key": "世界观", "value": format!("{} 的基础设定（待补充）", s.title)}),
            json!({"key": "主角", "value": "主角设定（待补充）"}),
        ],
        direction: format!("{} 的故事方向（待 Goethe 收敛）", s.title),
        outline: vec![
            json!({"chapter": "第1章", "title": "开端", "goal": "引出主角与核心冲突", "characters": []}),
            json!({"chapter": "第2章", "title": "推进", "goal": "展开主要情节线", "characters": []}),
        ],
        foreshadow_items: vec![json!({
            "id": "f1",
            "desc": "伏笔占位（待 Goethe 细化）",
            "plantChapter": "第1章",
            "payoffChapter": "第3章"
        })],
        current_arc: "arc_001".into(),
        current_window: vec![],
        next_window: vec!["第1章".into(), "第2章".into()],
        next_arc_goals: vec!["由 Goethe 基于全书方向提出下一弧目标".into()],
        arc_summary: "当前弧尚无可用章节摘要".into(),
        note: "启发式占位规划（LLM 未配置或不可用）".into(),
        state: "proposed".into(),
    }
}

pub(crate) fn heuristic_plan_iter(s: &DualAgentSession, instruction: Option<&str>) -> PlanOutput {
    let mut p = heuristic_plan(s);
    if let Some(inst) = instruction.filter(|i| !i.trim().is_empty()) {
        p.note = format!(
            "启发式占位规划（LLM 未配置或不可用；新增指示未精调）：{}",
            inst.trim()
        );
    }
    p
}

// ── P0 Dante AI 写稿引擎 ────────────────────────────────────────────────

