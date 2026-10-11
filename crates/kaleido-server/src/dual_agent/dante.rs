//! dual_agent/dante — [2026-10-05 语义拆分] 由 dual_agent.rs 按设计文档
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

/// Dante 写作指令（中文网文长篇风格，续写正文）。
pub(crate) const DANTE_SYS: &str = "你是 Kaleido 的 Dante，中文网文长篇写作 Agent。\n\
你的职责：根据 Goethe 的规划大纲、伏笔清单、作品设定和已有正文，续写当前章节正文。\n\
写作要求：\n\
1. 严格按大纲目标撰写，不偏离方向；\n\
2. 承接已有正文的剧情，不重复已有内容；\n\
3. 融入相关伏笔（伏笔描述中 plantChapter ≤ 当前章 ≤ payoffChapter 的伏笔）；\n\
4. 风格：中文网文长篇，叙事流畅，有场景感和人物对话；\n\
5. 输出纯正文，不带 Markdown 标题、不带\"好的\"自白、不带任何元注释；\n\
6. 字数约 1500-2000 字（目标字数以写作窗口 wordTarget 为准）。";

/// P0：构造 Dante 写稿 user prompt。
pub(crate) fn build_dante_user_prompt(s: &DualAgentSession, win: &WritingWindow) -> String {
    let mut parts = Vec::new();

    // 作品标题 + 方向
    parts.push(format!(
        "【作品信息】\n标题：{}\n全书方向：{}",
        s.title,
        s.plan.as_ref().map(|p| p.direction.as_str()).unwrap_or("未定")
    ));

    // 设定
    if let Some(p) = s.plan.as_ref() {
        if !p.settings.is_empty() {
            let settings_str: Vec<String> = p
                .settings
                .iter()
                .map(|sv| {
                    let key = sv.get("key").and_then(|v| v.as_str()).unwrap_or("?");
                    let val = sv.get("value").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("  - {key}：{val}")
                })
                .collect();
            parts.push(format!("【作品设定】\n{}", settings_str.join("\n")));
        }
    }

    // 当前窗口信息
    parts.push(format!(
        "【当前写作窗口】\n章节：{}\n标题：{}\n大纲目标：{}\n目标字数：{}字",
        win.chapter_id, win.title, win.outline, win.word_target
    ));

    // 相关伏笔：plantChapter ≤ 当前章 ≤ payoffChapter
    if let Some(p) = s.plan.as_ref() {
        let relevant_foreshadow: Vec<&Value> = p
            .foreshadow_items
            .iter()
            .filter(|f| {
                let plant = f.get("plantChapter").and_then(|v| v.as_str()).unwrap_or("");
                let payoff = f.get("payoffChapter").and_then(|v| v.as_str()).unwrap_or("");
                // 伏笔描述（无精确章号匹配时，只要非空即包含）
                !plant.is_empty() || !payoff.is_empty()
            })
            .collect();
        if !relevant_foreshadow.is_empty() {
            let fs_lines: Vec<String> = relevant_foreshadow
                .iter()
                .map(|f| {
                    let id = f.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                    let desc = f.get("desc").and_then(|v| v.as_str()).unwrap_or("");
                    let plant = f.get("plantChapter").and_then(|v| v.as_str()).unwrap_or("?");
                    let payoff = f.get("payoffChapter").and_then(|v| v.as_str()).unwrap_or("?");
                    format!("  - [{id}] {desc}（植入：{plant}，回收：{payoff}）")
                })
                .collect();
            parts.push(format!("【相关伏笔】\n{}", fs_lines.join("\n")));
        }
    }

    // 已写窗口摘要（token 预算裁剪）
    let written_drafts: Vec<String> = s
        .windows
        .iter()
        .filter(|w| w.status == "written" && w.id != win.id)
        .filter_map(|w| {
            w.draft.as_ref().map(|d| {
                format!("【{}】\n{}", w.title, d)
            })
        })
        .collect();
    if !written_drafts.is_empty() {
        let all_drafts = written_drafts.join("\n\n");
        let (fitted, dropped) = fit_text_to_token_budget(&all_drafts, 6000);
        let mut header = "【已有章节正文（已按 token 预算裁剪，用于续写衔接）】\n以下为已写章节，仅用于剧情衔接参考。严禁复用、模仿或重复以下任何句子的开头、句式或表达。每一章必须用全新的叙述开头。".to_string();
        if dropped > 0 {
            header.push_str(&format!("（已裁剪约 {dropped} 字符）"));
        }
        parts.push(format!("{header}\n{fitted}"));
    }

    parts.push(format!(
        "请撰写「{}」的正文。输出纯正文，不带标题和元注释。\n注意：不要重复已有章节的任何句子或段落，特别是开头句式。以全新的方式开启本章，与已有章节形成连续但不重复的叙事。",
        win.title
    ));
    parts.join("\n\n")
}

/// [morphling WriteHERE P1 2026-08-19] 从 LLM 回复中稳健地提取 JSON 数组：
/// 从第一个 `[` 开始做括号配对（正确处理嵌套数组与字符串内括号），忽略前后说明文字。
/// 找不到配对 `]` 时返回 None。
pub(crate) fn extract_json_array(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text.find('[')?;
    let mut depth: i32 = 0;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return text.get(start..=i);
                }
            }
            _ => {}
        }
    }
    None
}

/// [morphling WriteHERE P1 2026-08-19] 惰性 LLM 分解：把窗口的本章分解成语义原子化的
/// think/write 子任务 DAG。任何失败（LLM 错误 / JSON 解析 / 无 write 任务）返回 None，
/// 上层回退旧单次写稿（零回归）。仅当 `KALEIDO_DUAL_DECOMPOSE` 环境变量非 0/1 时才触发。
pub(crate) async fn try_decompose_window(
    prov_kind: &str,
    llm: &LlmRuntime,
    s: &DualAgentSession,
    win: &WritingWindow,
) -> Option<Vec<WindowTask>> {
    if !std::env::var("KALEIDO_DUAL_DECOMPOSE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return None;
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(60))
        .build()
        .ok()?;
    let mut book = format!("书名《{}》\n", s.title);
    if let Some(ref p) = s.plan {
        if !p.direction.is_empty() {
            book.push_str(&format!("【创作方向】{}\n", p.direction));
        }
        if !p.current_arc.is_empty() {
            book.push_str(&format!("【当前篇章】{}\n", p.current_arc));
        }
    }
    let user = format!(
        "为以下写作窗口设计一组原子化子任务（DAG），用于把本章从「设计决策」到「正文落地」逐级完成。\n\
{book}\
【本章标题】{title}\n\
【本章大纲目标】{outline}\n\
【目标篇幅】约 {word} 字\n\
要求：\n\
1. 产出 {n} 个子任务，至少 1 个 kind=\"write\"，其余可为 kind=\"think\"（设计/节拍分析，供 write 承接）。\n\
2. 用 deps 表达依赖：write 任务应 depend 于若干 think 或更早的 write 任务（其产出会注入 prompt）。\n\
3. 首个子任务 deps 为空。避免循环依赖。\n\
4. 用 JSON 数组输出即可（可输出一段话，把 JSON 数组放中间）。元素形如 {{\"id\":\"t1\",\"kind\":\"think\",\"goal\":\"...\",\"deps\":[],\"wordTarget\":50}}，kind 为 \"think\" 或 \"write\"，wordTarget 为字数，deps 是前置任务 id 数组，首任务 deps 为空。除 JSON 数组外其它文字忽略。",
        n = 4,
        title = win.title,
        outline = win.outline,
        word = win.word_target,
    );
    let resp = chat_completion_dispatch(
        &llm.base_url, &llm.api_key, &llm.model, prov_kind, DANTE_SYS, &user,
        0.1, 16384, 60, &client,
    )
    .await
    .ok()?;
    let trimmed = resp.trim();
    let json_str = extract_json_array(trimmed);
    let arr = serde_json::from_str::<Vec<serde_json::Value>>(json_str?).ok()?;
    let mut tasks: Vec<WindowTask> = Vec::new();
    for v in arr {
        let id = v["id"].as_str()?.to_string();
        let kind = v["kind"].as_str().unwrap_or("write").to_string();
        if kind != "think" && kind != "write" {
            continue;
        }
        let goal = v["goal"].as_str().unwrap_or("").to_string();
        if goal.trim().is_empty() {
            continue;
        }
        let deps = v["deps"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
            .unwrap_or_default();
        let word = v["wordTarget"].as_u64().unwrap_or(0) as u32;
        tasks.push(WindowTask {
            id,
            kind,
            goal,
            deps,
            word_target: word,
            status: "pending".into(),
            draft: None,
        });
    }
    if tasks.iter().any(|t| t.kind == "write") {
        tracing::info!(session_id=%s.id, window_id=%win.id, n=%tasks.len(), "dante window decomposed into DAG");
        Some(tasks)
    } else {
        None
    }
}

pub(crate) async fn run_dante_write_iter(
    prov_kind: &str,
    llm: &LlmRuntime,
    s: &DualAgentSession,
    win: &WritingWindow,
) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;

    // [morphling WriteHERE P1 2026-08-19] 递归原子化写稿：窗口带 sub_tasks DAG 时，
    // 按拓扑序逐任务生成（think 产出注入 write 的 prompt，write 产出拼接为正文）。
    // sub_tasks 为空 → 退化为旧单次写稿（零回归）。
    let dag_tasks = if !win.sub_tasks.is_empty() {
        win.sub_tasks.clone()
    } else {
        match try_decompose_window(prov_kind, llm, s, win).await {
            Some(t) => t,
            None => Vec::new(),
        }
    };
    if !dag_tasks.is_empty() {
        let ordered = topo_sort_tasks(&dag_tasks)?;
        let mut outputs: Vec<(String, String)> = Vec::new(); // (task_id, draft)
        let mut prose = String::new();
        for task in &ordered {
            let deps_ctx = deps_context(task, &ordered, &outputs);
            let user = build_task_user_prompt(s, win, task, &deps_ctx);
            let text = match chat_completion_dispatch(
                &llm.base_url,
                &llm.api_key,
                &llm.model,
                prov_kind,
                DANTE_SYS,
                &user,
                0.1, 16384, 90,
                &client,
            )
            .await
            {
                Ok(t) => strip_dante_cot(&t),
                Err(e) => {
                    return Err(format!(
                        "dante sub-task {} ({}) failed: {e}",
                        task.id, task.kind
                    ))
                }
            };
            outputs.push((task.id.clone(), text.clone()));
            if task.kind == "write" && !text.trim().is_empty() {
                prose.push_str(text.trim());
                prose.push_str("\n\n");
            }
        }
        let assembled = prose.trim_end().to_string();
        if assembled.is_empty() {
            tracing::warn!(session_id=%s.id, window_id=%win.id, "dante DAG produced empty prose");
        }
        return Ok(assembled);
    }

    let user = build_dante_user_prompt(s, win);
    let text = chat_completion_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        prov_kind,
        DANTE_SYS,
        &user,
        0.1, 16384, 90,
        &client,
    )
    .await?;
    Ok(text)
}

/// [morphling WriteHERE P1 2026-08-19] 拓扑排序（Kahn）。返回依赖先行的任务序列；
/// 二环依赖 / 未声明依赖引用返回 Err（此时上层回退单次写稿）。
pub(crate) fn topo_sort_tasks(tasks: &[WindowTask]) -> Result<Vec<WindowTask>, String> {
    let ids: std::collections::HashSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
    for t in tasks {
        for d in &t.deps {
            if !ids.contains(d) {
                return Err(format!(
                    "sub-task {} declares unknown dependency {}",
                    t.id, d
                ));
            }
        }
    }
    // Kahn：反复取出「所有依赖已完成」的任务
    let mut remaining: Vec<WindowTask> = tasks.to_vec();
    let mut done: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut order: Vec<WindowTask> = Vec::with_capacity(tasks.len());
    while !remaining.is_empty() {
        let before = remaining.len();
        let mut next_round: Vec<WindowTask> = Vec::with_capacity(remaining.len());
        let mut progressed = false;
        for t in remaining {
            if t.deps.iter().all(|d| done.contains(d)) {
                done.insert(t.id.clone());
                order.push(t);
                progressed = true;
            } else {
                next_round.push(t);
            }
        }
        remaining = next_round;
        if remaining.len() == before && !progressed {
            return Err("sub-task DAG contains a cycle".to_string());
        }
    }
    Ok(order)
}

/// [morphling WriteHERE P1 2026-08-19] 构造某任务的依赖上下文：已完成依赖产出的拼接。
pub(crate) fn deps_context(
    task: &WindowTask,
    _ordered: &[WindowTask],
    outputs: &[(String, String)],
) -> String {
    if task.deps.is_empty() {
        return String::new();
    }
    let mut ctx = String::from("【依赖先行产出（本任务写作时须承接）】\n");
    for d in &task.deps {
        if let Some((_, txt)) = outputs.iter().rev().find(|(id, _)| id == d) {
            if !txt.trim().is_empty() {
                ctx.push_str(&format!("—— 依赖 {d} ——\n{txt}\n\n"));
            }
        }
    }
    ctx
}

/// [morphling WriteHERE P1 2026-08-19] 构造原子子任务的 user prompt。
/// think 任务产出分析；write 任务产出正文片段。均注入依赖产出与本章大纲目标。
pub(crate) fn build_task_user_prompt(
    s: &DualAgentSession,
    win: &WritingWindow,
    task: &WindowTask,
    deps_ctx: &str,
) -> String {
    let mut book = format!("书名《{}》\n", s.title);
    if let Some(ref p) = s.plan {
        if !p.direction.is_empty() {
            book.push_str(&format!("【创作方向】{}\n", p.direction));
        }
        if !p.current_arc.is_empty() {
            book.push_str(&format!("【当前篇章】{}\n", p.current_arc));
        }
    }
    let role = match task.kind.as_str() {
        "think" => "你是创作设计师。为【本章】产出精确的设计决策/节拍分析，不写正文。",
        _ => "你是正文执笔。承接依赖产出，按大纲目标写出连贯的叙事正文。",
    };
    format!(
        "请完成以下原子子任务「{id}」（类型：{kind}）。\n\
{book}\
【本章标题】{title}\n\
【本章大纲目标】{outline}\n\
{deps_ctx}\
【任务目标】{goal}\n\
目标篇幅约 {word} 字。{role}\n\
直接输出成果本身，不要复述任务，不要加思考过程、不要用 markdown 标题包裹。",
        id = task.id,
        kind = task.kind,
        title = win.title,
        outline = win.outline,
        goal = task.goal,
        word = task.word_target,
    )
}

/// P0：Dante 写稿带重试（LLM 失败自动重试 1 次，再失败回退启发式）。
/// 返回 (draft, note)。LLM 不可用时直接启发式（note 标记 ERROR 前缀，供上层区分占位）。
/// [fix 2026-08-16] 产出后做两道质量闸：
///   1) 剥离 CoT / 规划尾巴（strip_dante_cot）
///   2) 与既有已写窗口查重，重叠率 > 0.5 视为复制，触发一次「去重重写」；
///      重写后仍重复则保留并显式标注。
pub(crate) async fn dante_write_with_retry(
    prov_kind: &str,
    llm: &LlmRuntime,
    s: &DualAgentSession,
    win: &WritingWindow,
) -> (String, String) {
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return (
            heuristic_dante_draft(s, win),
            "ERROR: LLM 未配置，已生成启发式占位草稿（非正式正文，请先配置 LLM 后重试）".to_string(),
        );
    }
    // 首次写稿（失败重试 1 次）
    let first = match run_dante_write_iter(prov_kind, llm, s, win).await {
        Ok(text) => {
            let cleaned = strip_dante_cot(&text);
            if cleaned.is_empty() {
                (heuristic_dante_draft(s, win), "Dante 产出为空，已回退占位草稿".to_string())
            } else {
                (cleaned, String::new())
            }
        }
        Err(e1) => {
            tracing::warn!(session_id=%s.id, window_id=%win.id, error=%e1, "dante write LLM failed, retrying once");
            match run_dante_write_iter(prov_kind, llm, s, win).await {
                Ok(text) => {
                    let cleaned = strip_dante_cot(&text);
                    if cleaned.is_empty() {
                        (heuristic_dante_draft(s, win), "Dante 产出为空，已回退占位草稿".to_string())
                    } else {
                        (cleaned, String::new())
                    }
                }
                Err(e2) => (
                    heuristic_dante_draft(s, win),
                    format!("ERROR: Dante LLM 失败（已重试 1 次仍失败），回退启发式草稿：{e2}"),
                ),
            }
        }
    };
    let (mut draft, mut note) = first;

    // 查重闸：与既有已写窗口比对开头重叠率
    if let Some((rate, dup_id)) = dante_max_overlap(s, win, &draft) {
        if rate > 0.5 {
            tracing::warn!(
                session_id=%s.id,
                window_id=%win.id,
                dup_window=%dup_id,
                rate=%format!("{:.2}", rate),
                "dante draft duplicates existing window, triggering dedup rewrite"
            );
            // 去重重写：携带查重上下文，要求全新开头
            let ctx = build_dante_dedup_prompt(s, win, &dup_id, rate);
            let rewrite = run_dante_dedup_rewrite(prov_kind, llm, &ctx).await;
            match rewrite {
                Some(mut r2) => {
                    r2 = strip_dante_cot(&r2);
                    let rate2 = dante_max_overlap(s, win, &r2)
                        .map(|(r, _)| r)
                        .unwrap_or(0.0);
                    if rate2 > 0.5 {
                        draft = r2;
                        note = format!(
                            "Dante 写稿完成，但与「{}」重复（重叠 {:.0}%），去重重写后仍重复，请人工检查",
                            dup_id,
                            rate2 * 100.0
                        );
                    } else {
                        draft = r2;
                        note = format!(
                            "Dante 写稿完成（去重重写成功：原稿与「{}」重叠 {:.0}%）",
                            dup_id,
                            rate * 100.0
                        );
                    }
                }
                None => {
                    note = format!(
                        "Dante 写稿完成，但与「{}」重复（重叠 {:.0}%），去重重写失败，保留原稿",
                        dup_id,
                        rate * 100.0
                    );
                }
            }
        } else {
            note = if note.is_empty() {
                "Dante AI 写稿完成（LLM 产出）".to_string()
            } else {
                note
            };
        }
    } else {
        note = if note.is_empty() {
            "Dante AI 写稿完成（LLM 产出）".to_string()
        } else {
            note
        };
    }
    (draft, note)
}

/// 构造去重重写的 user prompt：明确告知模型与哪个窗口重复，要求用全新开头重写。
pub(crate) fn build_dante_dedup_prompt(
    s: &DualAgentSession,
    win: &WritingWindow,
    dup_id: &str,
    rate: f64,
) -> String {
    let base = build_dante_user_prompt(s, win);
    format!(
        "{}\n\n【去重警告】上一稿与已写窗口「{}」的开头重叠率高达 {:.0}%，属于内容复制，不合格。\
          \n请用与已写章节完全不同的全新场景、全新开头句式重写本窗口「{}」的正文，\
          \n严禁复用、模仿或重复任何已有章节的句子、段落或开头表达。直接输出全新正文，不要解释。",
        base, dup_id, rate * 100.0, win.title
    )
}

/// 去重重写调用（LLM 失败返回 None，不回退启发式）。
pub(crate) async fn run_dante_dedup_rewrite(
    prov_kind: &str,
    llm: &LlmRuntime,
    ctx: &str,
) -> Option<String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(300))
        .build()
        .ok()?;
    chat_completion_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        prov_kind,
        DANTE_SYS,
        ctx,
        0.1, 16384, 2000,
        &client,
    )
    .await
    .ok()
}

/// P0：LLM 不可用时的启发式占位草稿（保证链路可走通）。
pub(crate) fn heuristic_dante_draft(s: &DualAgentSession, win: &WritingWindow) -> String {
    let direction = s
        .plan
        .as_ref()
        .map(|p| p.direction.as_str())
        .unwrap_or("故事待展开");
    format!(
        "（启发式占位草稿 · LLM 未配置）\n\n章节「{}」—— {}。\n\n大纲目标：{}。\n\n全书方向：{}。\n\n正文待 Dante（LLM）填充。",
        win.title, win.chapter_id, win.outline, direction
    )
}

