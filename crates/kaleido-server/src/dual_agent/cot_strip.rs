//! dual_agent/cot_strip — [2026-10-05 语义拆分] 由 dual_agent.rs 按设计文档
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

/// 剥离 Dante 直出正文里混入的 CoT / 规划文本。
/// 现象：模型无视 DANTE_SYS「输出纯正文」约束，把「好的，我需要写第3章…结构规划…开始写正文」
/// 等思考尾巴原样吐进 draft（afb win-03 尾部、cba 第1章开头均复现）。
/// 策略：先尝试定位「正文起点」标记（开始写正文/以下是正文…），取其后的内容；
/// 否则在文本后半段定位规划标记（好的，我需要写/结构规划/字数控制在…），从该处截断。
pub(crate) fn strip_dante_cot(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    // 1) BODY 起点标记（模型提示词里的分隔词），取其后的正文。优先级最高。
    const BODY_MARKERS: &[&str] = &[
        "开始写正文",
        "以下是正文",
        "正文如下：",
        "正文如下",
        "以下为正文",
        "写正文如下",
        "正文：",
        "正文:",
    ];
    for m in BODY_MARKERS {
        if let Some(pos) = t.find(m) {
            let after = t[pos + m.len()..].trim().trim_start_matches('：').trim_start_matches(':').trim();
            if after.chars().count() > 20 {
                return after.to_string();
            }
        }
    }
    let bytes = t.as_bytes();
    let len = bytes.len() as f64;
    // 2) 规划起始弱信号：一旦出现且其后跟随规划特征词（章/规划/结构/分析/字数/大纲/结尾），
    //    判定为 CoT 起点并截断（不受 50% 位置限制——规划尾巴通常紧跟正文）。
    const PLAN_STARTS: &[&str] = &[
        "好的，我需要写",
        "好的，用户需要",
        "好的，我",
        "让我仔细分析",
        "让我先规划",
        "我需要写第",
        "我先规划",
    ];
    const PLAN_FEATURES: &[&str] = &[
        "章", "规划", "结构", "分析", "字数", "大纲", "结尾", "分镜", "场景",
    ];
    for m in PLAN_STARTS {
        if let Some(pos) = t.find(m) {
            let probe = &t[pos..t.len().min(pos + 40)];
            if PLAN_FEATURES.iter().any(|f| probe.contains(f)) {
                // 该位置之后整体视为规划尾巴
                return t[..pos].trim().to_string();
            }
        }
    }
    // 3) 强规划特征词（正文里几乎不会出现）——仅在后半段截断，避免误伤正常叙述。
    let mut s = t.to_string();
    for m in &[
        "字数控制在",
        "字数要求：",
        "结构规划：",
        "大纲目标：",
        "叙事视角是",
        "写作要求：",
        "开始写正文",
    ] {
        if let Some(pos) = s.find(m) {
            if pos as f64 > len * 0.5 {
                s.truncate(pos);
                break;
            }
        }
    }
    s.trim().to_string()
}

/// 计算新稿与既有稿的开头重叠率（0.0-1.0）。
/// 专门抓「开头一字不差复制」的 bug（afb win-04 整段照抄 win-03 开头）。
/// 取两稿前 300 字符逐字符比对连续相同的部分。
pub(crate) fn dante_overlap_rate(new: &str, existing: &str) -> f64 {
    let n: Vec<char> = new.trim().chars().collect();
    let e: Vec<char> = existing.trim().chars().collect();
    if n.is_empty() || e.is_empty() {
        return 0.0;
    }
    let limit = 300usize.min(n.len()).min(e.len());
    if limit == 0 {
        return 0.0;
    }
    let mut same = 0usize;
    for i in 0..limit {
        if n[i] == e[i] {
            same += 1;
        } else {
            break;
        }
    }
    same as f64 / limit as f64
}

/// 找出与某窗口草稿重叠率最高的既有已写窗口，返回 (重叠率, 该窗口id)。无则 None。
pub(crate) fn dante_max_overlap(
    s: &DualAgentSession,
    win: &WritingWindow,
    new: &str,
) -> Option<(f64, String)> {
    s.windows
        .iter()
        .filter(|w| w.status == "written" && w.id != win.id)
        .filter_map(|w| {
            w.draft
                .as_deref()
                .map(|d| (dante_overlap_rate(new, d), w.id.clone()))
        })
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .filter(|(rate, _)| *rate > 0.0)
}

// ── P1 AI 审稿引擎 ──────────────────────────────────────────────────────

