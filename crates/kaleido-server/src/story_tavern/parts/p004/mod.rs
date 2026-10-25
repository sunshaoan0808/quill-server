#![allow(unused_imports)]
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::Utc;
use futures_util::stream::Stream;
use kaleido_core::{
    ActorStateUpdate, Compass, CompassStore, ContentTier, CreateSessionRequest, DirectorPlan,
    DirectorPlanRunStatus, PackStore, StageDirectorConfig, StoryPack, TavernMessage,
    TavernPersona, TavernSession, TavernSessionStore, TurnPhase, build_side_branch_catalog,
    enter_side_branch, seed_opening_if_needed,
    MemoryL2Event,
};
use kaleido_core::ledger::{LedgerKind, LedgerStore};
use kaleido_core::st_skimming::{
    ReaderPlatform, ReaderProfile, ReaderSkimmingConfig, SkimIssue, analyze_skimming,
};
use kaleido_core::st_emotional_hooks::{
    EmotionalHookConfig, PlotSignalSample, render_hook_execution_contract,
    repeated_recent_hook_signal,
};
use kaleido_core::st_simulation::{
    ChapterSimulation, SYSTEM_PROMPT, render as render_simulation, validate_simulation,
};
use kaleido_core::st_outline::{ChapterBriefView, build_chapter_execution_contract, render_execution_contract};
use serde::Deserialize;
use serde_json::{json, Value};
use rand::Rng;
use std::collections::HashMap;
use std::convert::Infallible;
use std::hash::Hasher;
use std::time::Duration as StdDuration;
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;
use crate::{map_core_err, session_from, AppState, ChatStreamEvent};
use crate::error_codes::*;
use crate::llm_stream::{
    stream_chat_completions_dispatch, TurnStreamError,
};
use crate::skill_layer::{SkillDoc, append_writing_skill_hint, load_writing_skill};
use crate::state::StreamHub;
use super::*;

/// 吞噬资产接线（P0）: 账本/情感曲线/角色弧/关系演化 → 注入块。
/// 调用点预载（对齐 preload_foreshadow_block），保持 build_tavern_system_prompt 纯函数。
/// 全部启发式零 LLM（词表/文件/graph），fail-open：查不到即空，零开销。

// [R6-2② 2026-10-07] 拆分自单文件 1638 行：prompt.rs（build_tavern_system_prompt）
// + perf.rs（性能基线测试）。账本/资产/合同助手保留在本文件。
#[allow(unused_imports)]
pub(crate) mod prompt;
#[allow(unused_imports)]
pub(crate) use prompt::*;
#[cfg(test)]
mod perf;

pub(crate) fn preload_asset_blocks(
    state: &AppState,
    pack: &StoryPack,
    session: &TavernSession,
    chapter_body: &str,
    plan: Option<&crate::inject_router::InjectPlan>,
) -> Option<String> {
    // [M1] 块开关：plan=None → 全注（= 改造前现状，零回归）。
    let block_on = |id: &str| -> bool {
        match plan {
            None => true,
            Some(p) => p.block_on(id),
        }
    };
    let mut blocks: Vec<String> = Vec::new();
    let data_root = state.auth.data_root().root();

    // 1. ledger —— Liyuan 记忆账本快照（≤1200 字符）
    if block_on("ledger") {
        let store = kaleido_core::ledger::LedgerStore::load(&data_root);
        if let Ok(snap) = store.snapshot() {
            let snap: String = snap.chars().take(1200).collect();
            if !snap.trim().is_empty() {
                blocks.push(format!("## 记忆账本（Liyuan 吞噬）\n{}", snap));
            }
        }
    }

    // 2. emotion —— 当前章节情感快照（novel2hermes T5，启发式词表）
    if block_on("emotion") && !chapter_body.trim().is_empty() {
        let curve = kaleido_core::emotion_curve::build_emotion_curve(&[kaleido_core::emotion_curve::ChapterText {
            chapter: session
                .chapter_cursor
                .clone()
                .unwrap_or_else(|| "当前章节".into()),
            text: chapter_body.to_string(),
        }]);
        if let Some(ch) = curve.chapters.first() {
            blocks.push(format!(
                "## 情感快照（当前章节）\n峰值强度 {}，主导情绪 {}，曲线形态 {}。整体弧线：{}",
                ch.peak_intensity, ch.dominant_emotion, ch.curve_shape, curve.overall_arc
            ));
        }
    }

    // 3. arc + relation —— graph 派生（ai-novel T4 + novel2hermes T5），聚焦玩家 vessel 相关
    let work_id: String = pack
        .source
        .refs
        .iter()
        .find(|r| r.len() == 36 && r.chars().filter(|c| *c == '-').count() == 4)
        .cloned()
        .unwrap_or_else(|| session.pack_id.clone());
    if !block_on("asset_blocks") { /* M1: router 关闭此块 */ } else if let Ok((_chars, rels)) = state.graph.list(&work_id) {
        if !rels.is_empty() {
            // 玩家 vessel 名（用于聚焦）
            let vessel_name = session
                .player
                .control_character_id
                .as_ref()
                .or(session.entry.vessel_character_id.as_ref())
                .and_then(|vid| pack.characters.iter().find(|c| c.id == *vid))
                .map(|c| c.name.as_str())
                .unwrap_or("");

            // 3a. 关系演化（T4）—— 最近章节关系
            let evos = crate::relation_evolution::RelationEvolution::build_evolution(&rels);
            let mut rel_lines: Vec<String> = Vec::new();
            let mut budget = 0usize;
            for e in evos.iter().take(8) {
                if !vessel_name.is_empty() && e.pair.0 != vessel_name && e.pair.1 != vessel_name {
                    continue;
                }
                if let Some(last) = e.chapters.last() {
                    let line = format!(
                        "- {} ↔ {}：{}（最近 {}）",
                        e.pair.0, e.pair.1, e.trend, last.chapter
                    );
                    budget += line.chars().count();
                    if budget > 800 {
                        break;
                    }
                    rel_lines.push(line);
                }
            }
            if !rel_lines.is_empty() {
                blocks.push(format!("## 关系演化（角色图谱）\n{}", rel_lines.join("\n")));
            }

            // 3b. 角色弧（T5）—— 跨章变化
            let mut entries = Vec::new();
            for rel in &rels {
                for w in rel.chapters.windows(2) {
                    entries.push(kaleido_core::character_arc::ArcEntry {
                        character: rel.from_char.clone(),
                        chapter: w[1].clone(),
                        field: format!("与{}的关系({})", rel.to_char, rel.category),
                        from: w[0].clone(),
                        to: w[1].clone(),
                    });
                }
            }
            let arcs = kaleido_core::character_arc::build_character_arcs(&entries);
            let mut arc_lines: Vec<String> = Vec::new();
            let mut budget = 0usize;
            for a in arcs.iter().take(5) {
                if !vessel_name.is_empty() && a.character != vessel_name {
                    continue;
                }
                for c in a.changes.iter().rev().take(2) {
                    let line = format!(
                        "- {}：{} {}→{}（{}）",
                        a.character, c.field, c.from, c.to, c.chapter
                    );
                    budget += line.chars().count();
                    if budget > 800 {
                        break;
                    }
                    arc_lines.push(line);
                }
            }
            if !arc_lines.is_empty() {
                blocks.push(format!("## 角色弧（跨章变化）\n{}", arc_lines.join("\n")));
            }
        }
    }

    if blocks.is_empty() {
        None
    } else {
        Some(blocks.join("\n\n"))
    }
}

/// 账本写入链路（P0 补全）: 回合完成后启发式提取关键叙事信息写入 Liyuan 账本
/// （ledger.json），供下回合 preload_asset_blocks 注入。
///
/// 设计约束：
/// - 零 LLM：纯词表 + 正则启发式（与 preload_asset_blocks 一致），fail-open 不阻塞回合。
/// - 只提取高置信度事实：场景时段（场景标签）、角色状态（角色名 + 状态词表）、
///   伏笔（未解关键词）、物品（「」引号内名词）、好感（角色对 + 情绪词表）。
/// - 每类限流（max N 条/回合），避免账本膨胀。
/// - upsert 失败仅 warn，不打断回合主流程。

/// UTF-8 安全截窗：从 byte 偏移向左右收缩到 char 边界。
/// 用于在 CJK 正文中围绕关键词截取上下文窗口（saturating_sub 可能切进多字节字符）。
pub(crate) fn utf8_window(text: &str, mid: usize, before: usize, after: usize) -> &str {
    let bytes = text.as_bytes();
    let mut start = mid.saturating_sub(before);
    while start > 0 && (bytes[start] & 0b1100_0000) == 0b1000_0000 {
        start -= 1;
    }
    let mut end = (mid + after).min(bytes.len());
    while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
        end += 1;
    }
    &text[start..end]
}

pub(crate) fn ledger_upsert_from_turn(
    state: &AppState,
    pack: &StoryPack,
    session: &TavernSession,
    story_body: &str,
    _user_text: &str,
) {
    let data_root = state.auth.data_root().root();
    let store = LedgerStore::load(&data_root);
    let turn = session.turn;

    let char_names: Vec<&str> = pack.characters.iter().map(|c| c.name.as_str()).collect();
    for (kind, key, value) in ledger_extract_entries(story_body, &char_names) {
        if let Err(e) = store.upsert(kind, key, value) {
            tracing::warn!(error = %e, turn, "st ledger upsert failed");
        }
    }
}

/// 启发式提取账本条目（纯函数，无 IO，可单测）。
/// 返回 `(kind, key, value)` 列表，每类限流避免膨胀。
pub(crate) fn ledger_extract_entries(story_body: &str, char_names: &[&str]) -> Vec<(LedgerKind, String, Value)> {
    let mut out: Vec<(LedgerKind, String, Value)> = Vec::new();

    let char_name = |body: &str| -> Option<&str> {
        // 按角色表顺序返回第一个在正文中出现的角色名
        char_names.iter().copied().find(|n| !n.is_empty() && body.contains(n))
    };

    // ---- 1. 时间（Time）: 场景标签时段 ----
    // 标签形如 `<场景：...｜午后>` / `<场景：...｜深夜>` / `<时间：午后>`
    const TIME_OF_DAY: &[&str] = &[
        "清晨", "早晨", "上午", "正午", "中午", "午后", "下午", "傍晚", "黄昏", "夜晚", "夜里",
        "深夜", "凌晨", "午夜", "黎明",
    ];
    let mut time_of_day: Option<&str> = None;
    for t in TIME_OF_DAY {
        // 场景标签内（<...> 之间）优先
        if let Some(lt) = story_body.find('<') {
            if let Some(gt) = story_body[lt..].find('>') {
                let tag = &story_body[lt..lt + gt];
                if tag.contains(t) {
                    time_of_day = Some(t);
                    break;
                }
            }
        }
        if time_of_day.is_none() && story_body.contains(t) {
            time_of_day = Some(t);
        }
    }
    if let Some(tod) = time_of_day {
        // 场景名从标签 `<场景：XXX｜时段｜角色>` 中解析, 不是取正文前 60 字符
        let scene_name = (|| {
            let lt = story_body.find('<')?;
            let gt = story_body[lt..].find('>')?;
            let tag = &story_body[lt + 1..lt + gt];
            let rest = tag.strip_prefix("场景：")?;
            Some(rest.split('｜').next()?.trim().to_string())
        })()
        .filter(|s| !s.is_empty());
        out.push((
            LedgerKind::Time,
            "当前场景".into(),
            json!({
                "timeOfDay": tod,
                "scene": scene_name.unwrap_or_else(|| "未知场景".into()),
            }),
        ));
    }

    // ---- 2. 状态（Status）: 角色名 + 状态词表 ----
    const STATUS_WORDS: &[&str] = &[
        "怀孕", "孕吐", "发烧", "受伤", "昏迷", "醉酒", "病倒", "疲惫", "流血", "虚弱",
        "失眠", "难受", "不安", "发抖", "落泪",
    ];
    let mut status_written = 0usize;
    for w in STATUS_WORDS {
        if status_written >= 2 {
            break;
        }
        if let Some(idx) = story_body.find(w) {
            if let Some(c) = char_name(utf8_window(story_body, idx, 60, w.len() + 10)) {
                out.push((
                    LedgerKind::Status,
                    c.to_string(),
                    json!({"state": w, "detail": utf8_window(story_body, idx, 0, w.len() + 20).to_string()}),
                ));
                status_written += 1;
            }
        }
    }

    // ---- 3. 伏笔（Foreshadow）: 未解关键词 ----
    const HOOK_WORDS: &[&str] = &["还没", "尚未", "究竟", "到底", "秘密", "谜", "蹊跷", "疑点", "未解"];
    let mut hook_written = 0usize;
    for w in HOOK_WORDS {
        if hook_written >= 2 {
            break;
        }
        if let Some(idx) = story_body.find(w) {
            let ctx: String = utf8_window(story_body, idx, 20, w.len() + 30).chars().collect();
            out.push((
                LedgerKind::Foreshadow,
                w.to_string(),
                json!({"promise": ctx, "status": "open"}),
            ));
            hook_written += 1;
        }
    }

    // ---- 4. 物品（Item）: 「」引号内名词（2-8 字符，非对话）----
    static ITEM_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // 字面量正则不可能编译失败：OnceLock 静态化同时消除每次调用的重编译开销。
    let item_re =
        ITEM_RE.get_or_init(|| regex::Regex::new(r"「([^「」]{2,8})」").expect("literal regex"));
    let mut item_written = 0usize;
    for cap in item_re.captures_iter(story_body) {
        if item_written >= 3 {
            break;
        }
        let candidate = cap[1].trim();
        if candidate.is_empty() {
            continue;
        }
        // 排除对话/语气/常见虚词（含 CJK 标点——「老板，来壶热茶。」这类对话句
        // 不含 ASCII 标点但含中文逗号/句号，必须一并排除）
        if candidate.chars().any(|c| {
            c.is_ascii_punctuation()
                || matches!(
                    c,
                    '。' | '，' | '、' | '！' | '？' | '；' | '：' | '…' | '～' | '「' | '」' | '『' | '』' | '“' | '”'
                )
        }) || candidate.contains("说")
            || candidate.contains("道")
            || candidate.contains("吗")
            || candidate.contains("呢")
            || candidate.contains("啊")
            || candidate.contains("吧")
        {
            continue;
        }
        out.push((
            LedgerKind::Item,
            candidate.to_string(),
            json!({"state": "carried", "source": "turn"}),
        ));
        item_written += 1;
    }

    // ---- 5. 好感（Affinity）: 角色对 + 情绪词表 ----
    const AFFINITY_WORDS: &[&str] = &[
        "温柔", "依恋", "抗拒", "冷漠", "疏远", "亲近", "疼惜", "心疼", "厌恶", "感激", "愧疚",
        "嗔怪", "宠溺",
    ];
    let mut aff_written = 0usize;
    for w in AFFINITY_WORDS {
        if aff_written >= 2 {
            break;
        }
        if let Some(idx) = story_body.find(w) {
            let window = utf8_window(story_body, idx, 40, w.len() + 20);
            let a = char_name(window);
            // 找第二个角色：从窗口内再找一个不同角色
            let b = char_names.iter().copied().find(|n| {
                !n.is_empty() && *n != a.unwrap_or("") && window.contains(n)
            });
            if let (Some(an), Some(bn)) = (a, b) {
                out.push((
                    LedgerKind::Affinity,
                    format!("{}↔{}", an, bn),
                    json!({"sentiment": w, "trend": "stable", "detail": window.to_string()}),
                ));
                aff_written += 1;
            }
        }
    }

    out
}

/// X4 (吞噬自 xiami outline.rs): 从导演计划派生章节执行合同文本（system prompt 注入 + director-config 诊断展示共用）。
/// 简单方案：required_events = plan.hits_beats，summary = plan.goal（可含 X2c 推演的 ending_state/next_impetus），
/// source_plan_id = "director"。**不阻断**：无 director_plan 或无数据时返回 None，调用方跳过。
pub(crate) fn build_director_execution_contract(session: &TavernSession) -> Option<String> {
    let plan = session.director_plan.as_ref()?;
    if plan.hits_beats.is_empty() && plan.goal.trim().is_empty() {
        return None;
    }
    let chapter_number = session
        .chapter_cursor
        .as_deref()
        .and_then(|cursor| {
            cursor
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .ok()
        })
        .unwrap_or(0);
    let brief = ChapterBriefView {
        chapter_number,
        key_events: plan.hits_beats.clone(),
        summary: plan.goal.clone(),
        ..Default::default()
    };
    let contract = build_chapter_execution_contract(&brief, "director", 0);
    let rendered = render_execution_contract(&contract);
    if rendered.trim().is_empty() {
        None
    } else {
        Some(rendered)
    }
}
