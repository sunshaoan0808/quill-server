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

/// X2b (吞噬自 xiami emotional_hooks.rs): 用最近 N 条 assistant 正文构造 `PlotSignalSample`。
/// 简单截取：ending_state 取最后一句（按中文句读符号切），summary 取正文开头片段，hook_changes 留空。
pub(crate) fn build_hook_recent_samples(session: &TavernSession) -> Vec<PlotSignalSample> {
    session
        .messages
        .iter()
        .rev()
        .filter(|m| m.role == "assistant")
        .take(6)
        .map(|m| {
            let content = m.content.trim();
            let last_sentence = content
                .rsplit(|c| {
                    matches!(c, '。' | '！' | '？' | '…' | '，' | '.' | '!' | '?')
                })
                .next()
                .unwrap_or(content)
                .trim()
                .to_string();
            PlotSignalSample {
                ending_state: last_sentence,
                summary: content.chars().take(80).collect(),
                hook_changes: Vec::new(),
            }
        })
        .collect()
}

/// X2a (吞噬自 xiami skimming.rs): 默认读者速读质检配置 —— Tomato 快扫 + 平衡强度 + 参与门禁。
pub(crate) fn reader_skimming_config() -> ReaderSkimmingConfig {
    ReaderSkimmingConfig {
        platform: ReaderPlatform::Tomato,
        primary_reader: ReaderProfile::FastScan,
        ..ReaderSkimmingConfig::default()
    }
}

/// X2a (吞噬自 xiami skimming.rs): 速读风险问题清单渲染为审稿/修复 prompt 文案。
pub(crate) fn render_skim_issues_for_prompt(issues: &[SkimIssue]) -> String {
    issues
        .iter()
        .map(|i| {
            format!(
                "[P{}][{}] {}（{}）修复建议：{}",
                i.severity, i.category, i.message, i.evidence, i.fix
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}


/// Parse option list after 【选项】 / <choices>: JSON array, or line/numbered list.
pub(crate) fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// ST-26: Canon hard guard — 检测叙事中出现的"未知人名"（原著外对象）。
/// 规则：扫描 full_text 中 [2-4个连续汉字]+(说|道|问|喊|叫|笑|哭|心想) 的说话主体，
/// 若该名字既不在已知角色/关键词集合、也未出现在当前章节正文（chapter_body）中 → 判为越界。
/// 返回违规名字；未发现返回 None。
/// 退役(2026-08-16 统一来源根治)：不再被调用——切词启发式会把「跟您说/按理说/带着点」
/// 误判为外对象，结构上无法区分人名与短语碎片。角色判定只信统一来源
/// (pack characters + LLM roster + 场景标签 + LLM 兜底识别)。
/// 保留定义供未来 LLM 语义识别复用（不参与 guard_narrative 主链路）。
#[allow(dead_code)]
pub(crate) fn detect_unknown_canon_names(full_text: &str, chapter_body: &str, known: &std::collections::HashSet<String>) -> Option<String> {
    // 触发标记: 仅纯说话动词——笑/哭/怒/叹 等情绪字若作触发，
    // 「她亲恼羞成怒」会被切出「亲恼羞成」当角色名（实测误报，2026-08-14）。
    // 情绪字保留在 STRIP_MARKERS 中仅用于候选去尾（林晚笑道 → 林晚）。
    // 2026-08-16 瞎报根治: '道' 在现代网文里大量作名词尾（胡说八道/谁知道/
    // 城市主干道/坡道），独立触发会把名词短语切碎当角色名。仅当 '道' 前紧邻
    // 说话动词（说道/问道/答道/喊道/叫道/笑道/叹道）时才视为说话标记。
    let speech_markers = ['说', '问', '喊', '叫'];
    const STRIP_MARKERS: [char; 9] = ['说', '道', '问', '喊', '叫', '笑', '哭', '怒', '叹'];
    let chars: Vec<char> = full_text.chars().collect();
    let n = chars.len();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        let mut is_marker = speech_markers.contains(&c);
        // '道' 单独出现时，仅当紧邻说话动词（说道/问道/答道/喊道/叫道/笑道/叹道）
        if c == '道' && i >= 1 {
            let prev = chars[i - 1];
            if matches!(prev, '说' | '问' | '答' | '喊' | '叫' | '笑' | '叹' | '应' | '回' | '沉' | '低' | '厉' | '轻' | '冷' | '淡' | '缓' | '惊' | '急' | '忙' | '怒') {
                is_marker = true;
            }
        }
        if is_marker && i >= 2 {
            // 向前找连续 2-4 个汉字作为候选名字
            let mut j = i - 1;
            let mut name_chars: Vec<char> = Vec::new();
            while j < n && j < i && name_chars.len() < 4 && is_cjk(chars[j]) {
                name_chars.push(chars[j]);
                if j == 0 { break; }
                j -= 1;
            }
            if name_chars.len() >= 2 && name_chars.len() <= 4 {
                let mut name: String = name_chars.iter().rev().collect();
                // P2 修复: "XX说道/问道" 等——'道' 前紧邻 '说'，候选名会误含尾部说话标记。
                // 去掉尾部标记后再校验（"林晚说道" → 候选"林晚说" → "林晚"）。
                // 2026-08-14: 去尾用 STRIP_MARKERS（含笑/哭/怒/叹 情绪字——"林晚笑道" → "林晚"）。
                if name.chars().count() >= 3 {
                    let last = name.chars().last().unwrap_or(' ');
                    if STRIP_MARKERS.contains(&last) {
                        name.pop();
                    }
                }
                if name.chars().count() < 2 {
                    i += 1;
                    continue;
                }
                // 跳过代词/称呼类：他/她/你/我/您/它/谁/大家 + 1字
                if matches!(name.as_str(), "他" | "她" | "你" | "我" | "您" | "它" | "谁" | "大家" | "现在" | "忽然" | "突然" | "果然" | "终于" | "然后" | "不过") {
                    i += 1;
                    continue;
                }
                // 跳过"他/她+字"（如"他道"已经排除，"她笑"）形式：首字为代词时名字>=3才可能有效
                let first = name.chars().next().unwrap_or(' ');
                if (first == '他' || first == '她' || first == '你' || first == '我') && name.chars().count() <= 2 {
                    i += 1;
                    continue;
                }
                // 2026-08-16 瞎报根治: 4 字候选几乎全是切词碎片（东电话里/段石阶坡/城市主干），
                // 真实人名 4 字极罕见（仅复姓+双字名），宁漏勿瞎——直接排除 4 字候选。
                if name.chars().count() > 3 {
                    i += 1;
                    continue;
                }
                // 2026-08-16 瞎报根治: 称呼后缀（老板娘/先生/小姐…）是通用称谓不是专名。
                if TITLE_SUFFIXES.iter().any(|t| name.ends_with(t)) {
                    i += 1;
                    continue;
                }
                // P2 误报修复（2026-08-06）: 过滤 n-gram 切碎的短语碎片——
                // 「开口时没有说话」→ 候选「口时没有」、「茶馆只有一道门」→「馆只有一」。
                // 特征：2-4 字且含功能字（的/了/有/一/只/没…），真实人名几乎不含这些字。
                if name.chars().any(is_functional_char) {
                    i += 1;
                    continue;
                }
                // 再过滤：末字是功能字/副词性收尾的碎片（"时候""地方""过去"等）
                let last = name.chars().last().unwrap_or(' ');
                if matches!(
                    last,
                    '时' | '候' | '方' | '去' | '来' | '着' | '过' | '出' | '起' | '到' | '走' | '看' | '听' | '说' | '想'
                ) {
                    i += 1;
                    continue;
                }
                if !known.contains(&name) && !chapter_body.contains(&name) {
                    return Some(name);
                }
            }
        }
        i += 1;
    }
    None
}

/// P2 (叙界守卫, 吸收自叙界): 生成后多维守卫 — 扩展现 ST-26 人名黑名单为
/// 人物/节拍/出场/大纲四维检查。high → 打回重生成（阻止推进）；medium → 仅提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuardSeverity {
    High,
    Medium,
}

#[derive(Debug, Clone)]
pub(crate) struct GuardViolation {
    pub(crate) severity: GuardSeverity,
    pub(crate) dim: &'static str,
    pub(crate) msg: String,
}

/// 称呼后缀: 通用称谓（老板娘/先生/小姐/老师…）不是专名，兜底名单也不当外对象
pub(crate) const TITLE_SUFFIXES: [&str; 18] = [
    "老板娘", "老板", "先生", "小姐", "女士", "阿姨", "叔叔", "大哥", "大姐",
    "老师", "同学", "医生", "护士", "司机", "服务员", "店员", "邻居", "房东",
];

/// 单字功能词：用于过滤 n-gram 碎片（以功能字开头/结尾的候选基本不成实义词）。
pub(crate) fn is_functional_char(c: char) -> bool {
    matches!(
        c,
        '的' | '了' | '在' | '是' | '和' | '与' | '或' | '被' | '把' | '将' | '让' | '给' | '从'
            | '到' | '对' | '向' | '往' | '有' | '没' | '一' | '这' | '那' | '我' | '你' | '他'
            | '她' | '它' | '们' | '自' | '己' | '已' | '正' | '还' | '就' | '但' | '可' | '只'
            | '如' | '因' | '所' | '虽' | '即' | '无' | '并' | '且' | '再' | '终' | '忽'
            | '突' | '现' | '过' | '后' | '时' | '事' | '地' | '什' | '么' | '怎' | '为' | '何'
            | '直' | '未' | '不' | '非' | '想' | '要' | '能' | '会' | '应' | '该' | '都' | '也'
    )
}

/// 提取中文串中的有效关键词：2-4 字 n-gram 集合（去重、过滤停用词/功能碎片），
/// 用于节拍/大纲命中检测。命中判定基于集合交集，避免整句长串无法匹配的问题。
pub(crate) fn guard_keywords(text: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "不能", "不要", "不可", "不许", "不得", "必须", "应该", "一定", "的", "了", "在", "是", "和", "与", "或",
        "被", "把", "将", "让", "给", "从", "到", "对", "向", "往", "有", "没有", "一个", "这个", "那个", "我们",
        "你们", "他们", "咱们", "自己", "已经", "正在", "还是", "就是", "但是", "可是", "只是", "如果", "那么",
        "然后", "因为", "所以", "虽然", "即使", "无论", "以及", "并且", "而且", "还要", "再次", "终于", "忽然",
        "突然", "现在", "过去", "以后", "时候", "事情", "地方", "什么", "怎么", "为什么", "如何", "一直", "从未",
    ];
    let chars: Vec<char> = text
        .chars()
        .filter(|c| is_cjk(*c) || c.is_alphanumeric())
        .collect();
    if chars.is_empty() {
        return vec![];
    }
    let mut out = std::collections::HashSet::new();
    for win in 2..=4usize {
        if chars.len() < win {
            break;
        }
        for i in 0..=(chars.len() - win) {
            let g: String = chars[i..i + win].iter().collect();
            if STOP.contains(&g.as_str()) {
                continue;
            }
            let first = g.chars().next().unwrap_or(' ');
            let last = g.chars().last().unwrap_or(' ');
            if is_functional_char(first) || is_functional_char(last) {
                continue;
            }
            out.insert(g);
        }
    }
    out.into_iter().collect()
}

/// P2-1 四维守卫主函数：
/// 1) 人物越界(high)：原著外对象（扩展现 ST-26 detect_unknown_canon_names）
/// 2) 节拍遗漏(high)：locked_beats 红线关键词在叙事中完全未体现
/// 3) 人物出场(medium)：node.present_characters 应出场角色未在叙事中出现
/// 4) 章节目标偏离(medium)：章节 goals 关键词完全未体现
pub(crate) fn guard_narrative(
    full_text: &str,
    chapter_body: &str,
    known_names: &std::collections::HashSet<String>,
    present_names: &[String],
    locked_beats: &[String],
    chapter_goals: &[String],
    roster_names: Option<&[String]>,
) -> Vec<GuardViolation> {
    let mut violations = Vec::new();
    let full_ngrams: std::collections::HashSet<String> =
        guard_keywords(full_text).into_iter().collect();
    // ST-30 (2026-08-15 根治): 生成端角色清单优先——LLM 回合末尾自报出场人名，
    // 精确集合比对 known。根治「说话标记前切词」启发式的两个固有缺陷：
    //   漏报: 叙述/动作形态点名（「李铁柱的声音比雨声重」）无说话标记 → 检测不到
    //   误报: 切词切碎短语（「明日柜上/却带着点/亲恼羞成」）→ 堆词过滤打地鼠修不完
    // 清单存在 → 以其为准（不再跑切词启发式，避免其误报）；
    // 清单缺失/空 → 降级现有启发式（场景标签 + 引语切词），行为与修复前一致。
    if let Some(roster) = roster_names {
        for name in roster {
            let n = name.trim();
            // [守卫降噪] 玩家 vessel 不是原著外对象
            if n == "玩家" { continue; }
            if n.chars().count() >= 2 && !known_names.contains(n) && !chapter_body.contains(n) {
                violations.push(GuardViolation {
                    severity: GuardSeverity::High,
                    dim: "人物",
                    msg: format!("原著外对象「{}」（角色清单）", n),
                });
            }
        }
    } else {
        // 场景标签检测（<场景：…｜雨夜｜沈棠、林晚、王麻子>）——引语式检测
        // 只抓「XX说道」前的人名，场景标签/叙述里的原著外角色会漏检（实测王麻子案例）。
        // 提取「场景：」后顿号分隔的名字段，逐个与 known 对比。
        if let Some(tag) = full_text.find("<场景") {
            let seg = &full_text[tag..];
            if let Some(end) = seg.find('>') {
                let inner = &seg[..end];
                // 结构 <场景：地点｜天气/氛围｜角色、角色、角色> — 角色段在最后一个「｜」之后
                if let Some(sep) = inner.rfind('｜') {
                    for piece in inner[sep + 3..].split('、') {
                        let p = piece.trim();
                        let p = p.trim_matches(|c: char| c.is_whitespace() || c == '、');
                        // 2026-08-14 误报修复: 「温热的」「白的蓝」等形容词片段被误判为角色
                        // (1) 含 '的' → 形容词/所属结构，中文人名几乎不含此字;
                        // (2) known 角色名前缀 → 角色延伸（沈棠的伞/林晚的），非外角色
                        let known_prefix = known_names.iter().any(|k| !k.is_empty() && p.starts_with(k.as_str()));
                        // [守卫降噪] 玩家 vessel 跳过
                        if p == "玩家" { continue; }
                        if p.chars().count() >= 2
                            && !p.contains('的')
                            && !known_prefix
                            && !known_names.contains(p)
                        {
                            violations.push(GuardViolation {
                                severity: GuardSeverity::High,
                                dim: "人物",
                                msg: format!("场景标签出现原著外对象「{}」", p),
                            });
                        }
                    }
                }
            }
        }
        // 2026-08-16 统一来源根治: 退役切词启发式（detect_unknown_canon_names）——
        // 它从「XX说道」前切 2-3 字当角色名，结构上无法区分人名与短语碎片
        // （「跟您说/按理说/带着点…」被误判为外对象，实测「跟您」「按理」「带着点」误报）。
        // 角色判定只信统一来源: pack characters + LLM roster（<角色清单>）+ 场景标签 + LLM 兜底识别。
        // 宁漏勿瞎——真外对象由 LLM 语义识别兜底（roster 缺失时每 3 轮 LLM 兜底提取）。
    }
    for beat in locked_beats {
        let kws = guard_keywords(beat);
        if kws.is_empty() {
            continue;
        }
        let hit = kws.iter().any(|k| full_ngrams.contains(k));
        // [守卫降噪] 抽象节拍兜底：kws 全未命中时，若 beat 的核心实词（去功能字单字）
        // 在正文/状态出现 ≥3 次，算体现（如“信”出现 5 次但无“抹除”二字）
        let hit = hit || {
            let core: Vec<char> = beat.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c) && !is_functional_char(*c)).collect();
            core.iter().any(|c| {
                let s: String = [*c].iter().collect();
                full_ngrams.iter().any(|g| g.contains(&s[..]))
                    && full_ngrams.iter().filter(|g| g.contains(&s[..])).count() >= 2
            })
        };
        if !hit {
            violations.push(GuardViolation {
                severity: GuardSeverity::High,
                dim: "节拍",
                msg: format!("硬节拍「{}」未被体现", beat),
            });
        }
    }
    for name in present_names {
        let n = name.trim();
        if n.is_empty() || n.chars().count() < 2 {
            continue;
        }
        if !full_text.contains(n) {
            violations.push(GuardViolation {
                severity: GuardSeverity::Medium,
                dim: "出场",
                msg: format!("应出场角色「{}」未出现", n),
            });
        }
    }
    // [守卫降噪] 自由 pack 常无章节目标：空 goals 直接跳过，不告空气
    if chapter_goals.is_empty() {
        return violations;
    }
    // NOTE: 双持检测在调用方做（需 sess.pockets），见下方 guard_text 块后。
    for goal in chapter_goals {
        let kws = guard_keywords(goal);
        if kws.is_empty() {
            continue;
        }
        let hit = kws.iter().any(|k| full_ngrams.contains(k));
        if !hit {
            violations.push(GuardViolation {
                severity: GuardSeverity::Medium,
                dim: "大纲",
                msg: format!("章节目标「{}」未体现", goal),
            });
        }
    }
    violations
}

// [ST-15 fix 2026-08-16] 提取【节点推进】marker 的节点 ID。
// 兼容两种写法：
//   1) 【节点推进:n2】   —— 冒号+节点ID在括号内（prompt 教 LLM 的格式）
//   2) 【节点推进】 n2   —— 节点ID在括号外（旧实现期望的格式）
// 旧实现用 rfind("【节点推进】") 匹配，遇到格式1 永远返回 -1，节点推进从未生效，
// 导致剧情一直卡在楔子节点（选项重复/场景原地打转的直接根因）。
// 返回 (marker 起始位置, 节点ID)；未找到返回 None。


/// ST-30: 提取回合正文末尾的 <角色清单>…</角色清单> 结构化块（生成端角色自报）。
///
/// 2026-08-15 根治方案：守卫不再依赖「说话标记前切词」启发式（漏报：叙述形态点名
/// 检测不到；误报：切词切碎短语「明日柜上/却带着点」，堆词过滤打地鼠修不完）。
/// 改为要求 LLM 在回合正文末尾自报出场人名清单，守卫做精确集合比对。
/// 返回 (clean_text, Option<Vec<String>>)：清单存在且非空 → Some(名字列表)；缺失/空 → None。
/// S7 改进① (2026-08-18): query 叠加当前节点剧情要点（标题 + locked_beats）。
/// 使久远关键剧情（承诺/约定/关系确立）在剧情推进到相关节点时能语义召回，
/// 而非只依赖"最近消息"——此前长间距关键事件因 query 无剧情导向而漏召回。
/// §14.7.2B (2026-08-18): 查 L4 affinity 中某条关系的数值（用于 L3 edges 渲染附 好感 N）。
/// L4 affinity 键格式：`charId`（玩家→该角色）或 `charId:targetId`（双向）。
/// from→to 关系匹配：优先精确 `from:to`，其次 `to:from`（affinity 可能是反向存），
/// 最后若 from 是玩家（无 target 单键 `to`）也命中。返回好感值。
pub fn affinity_for_edge(affinity: &serde_json::Value, from: &str, to: &str) -> Option<i64> {
    let obj = affinity.as_object()?;
    for key in [format!("{from}:{to}"), format!("{to}:{from}"), to.to_string()] {
        if let Some(v) = obj.get(&key).and_then(|v| v.as_i64()) {
            return Some(v);
        }
    }
    None
}

/// §13.5 Scene Gate (2026-08-18): 首回合场景错位检测（确定性地点校验，不靠语义）。
/// 从正文首行 <场景：地点｜…> 提取地点段；若含明显"后期地点"词（酒店/套房/机场/三亚/
/// 月见/海边/海滩/沙滩/度假村）且首回合（turn=1，节点应为开局场景）→ 判定错位。
/// 保守设计：只拦"明确写错到后续章节地点"的强信号；未知/学校/家里等地名一律放行（不误伤）。
pub fn is_scene_mismatch_location(text: &str) -> bool {
    let head = text.trim_start();
    let Some(open) = head.find("<场景") else { return false };
    let Some(colon) = head[open..].find('：').map(|i| open + i) else { return false };
    // ：是全角（3 字节），+1 会落在字符中间（UTF-8 越界 panic）；用 len_utf8 跳过整个字符
    let after = &head[colon + '：'.len_utf8()..];
    let loc: String = after
        .chars()
        .take_while(|c| *c != '｜' && *c != '|' && *c != '>')
        .collect::<String>()
        .trim()
        .to_string();
    if loc.is_empty() {
        return false; // 无地点段 → 不误拦
    }
    const LATE_LOCATION_WORDS: [&str; 10] = [
        "酒店", "套房", "机场", "三亚", "月见", "海边", "海滩", "沙滩", "度假村", "客房",
    ];
    LATE_LOCATION_WORDS.iter().any(|w| loc.contains(w))
}

/// §13.4① (2026-08-18): 玩家动作指令强制包装——短动作型消息加系统侧强调，
/// 让「环顾/打招呼」等明确动作必须在本回合真实发生（实证：度蜜月首回合"打招呼"被 LLM
/// 忽略成"追上她+她先笑"，选项-场景解耦）。保守触发：≤24 字 + 命中动作信号词，
/// 长叙述/自由输入不包装（避免过度干预）。
pub fn wrap_player_action(msg: &str) -> Option<String> {
    let m = msg.trim();
    if m.chars().count() > 24 {
        return None;
    }
    const ACTION_WORDS: [&str; 18] = [
        "环顾", "打招呼", "查看", "观察", "走向", "拿起", "尝试", "询问",
        "抱", "摸", "亲", "问", "看", "听", "站", "坐", "跟", "说",
    ];
    if !ACTION_WORDS.iter().any(|w| m.contains(w)) {
        return None;
    }
    Some(format!(
        "【玩家动作指令】\n{m}\n\n——这是玩家的明确动作。本回合叙事必须让该动作真实发生（打招呼须有真实问候/回应，环顾须描述当下场景的观察所得），不得跳过、弱化或替换为其他行为。"
    ))
}

/// §14.6① 非在场角色一行概要（身份锚点）：名字+分级+personality 前 60 字。
/// 用途：非在场角色保留存在感（防 LLM 忘掉全书角色），但不喂示例对白/心智模型等
/// 压制源（避免非在场角色声线污染当前叙事）。
pub fn character_summary_line(c: &kaleido_core::PackCharacterRef) -> String {
    let tier = match c.content_tier {
        Some(ContentTier::Safe) => " [全年龄]",
        Some(ContentTier::Standard) => " [标准]",
        Some(ContentTier::Open) => " [开放]",
        None => "",
    };
    let pers: String = c.personality.chars().take(60).collect();
    format!("- {}{}：{}（未在场）", c.name, tier, pers)
}

/// §14.6③ 按需召回压缩卡（非在场但玩家/剧情点名的角色）：概要+声线+界限 ≤300 字模板。
/// 让"提到但没信息"的角色有设定可依，但不铺开示例对白（省 token、防声线污染）。保留供后续
/// 更强召回（完整卡）用；当前按需召回附 character_summary_line 轻量概要。
#[allow(dead_code)]
pub fn character_compact_card(c: &kaleido_core::PackCharacterRef) -> String {
    let mut out = format!("**{}**：{}。{}", c.name, c.personality, c.speech_style);
    if !c.voice_profile.trim().is_empty() {
        out.push_str(&format!("\n声线：{}\n", c.voice_profile.trim()));
    }
    if !c.boundaries.is_empty() {
        out.push_str("\n界限：");
        for b in &c.boundaries {
            out.push_str(&format!("{b} "));
        }
    }
    out.chars().take(300).collect()
}

pub fn s7_attach_plot_scope(query: String, node: Option<(&str, &[String])>) -> String {
    let mut q = query;
    if let Some((title, beats)) = node {
        let mut scope = title.to_string();
        if !beats.is_empty() {
            scope.push_str("\n【剧情要点】");
            scope.push_str(&beats.join("；"));
        }
        if !scope.trim().is_empty() {
            q.push('\n');
            q.push_str(&scope);
        }
    }
    q
}

/// S7 改进② (2026-08-18): 注入标题按命中文本是否含剧情关键信号分级——
/// 含承诺/约定/答应/约好 → 硬约束语气（关键事实必须延续）；否则软参考（按需沿用）。
pub fn s7_recall_title(joined: &str) -> &'static str {
    const KEY_SIGNALS: [&str; 6] = ["承诺", "约定", "答应", "约好", "说好", "答应过"];
    if KEY_SIGNALS.iter().any(|s| joined.contains(s)) {
        "【历史回忆·关键约定/承诺（已确立事实，必须延续，不得遗忘或推翻）】"
    } else {
        "【历史回忆·混合检索命中（较早回合的对话细节，按需沿用）】"
    }
}

/// [waidrin ①] `**Name**` 引用追踪（吸收自 waidrin engine.ts narrate 口径）：
/// 扫正文里 `**名**` 包裹的角色引用（支持 `**沈棠的**` 属格变体，取首段名）。
/// 返回去重后的引用名列表（顺序=出现序）。纯文本扫描，零 LLM。
pub fn scan_bold_refs(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(s) = rest.find("**") {
        let after = &rest[s + 2..];
        let Some(e) = after.find("**") else { break };
        let mut name = after[..e].trim().to_string();
        // 属格/后缀变体：取 `的/之/，、。/` 之前段
        for sep in ['的', '之', '，', '、', ',', '/', ' '] {
            if let Some(i) = name.find(sep) {
                name = name[..i].trim().to_string();
                break;
            }
        }
        if name.chars().count() >= 2 && !out.contains(&name) {
            out.push(name);
        }
        rest = &after[e + 2..];
    }
    out
}

/// ST-26: 提取【状态更新】JSON 块（括号深度匹配，同【面板】）。
/// 成功解析且 character_id 非空的块收集并剥离；解析失败/character_id 为空时跳过该块但保留原文。
/// 软上限 20 块。
/// [fix §10 2026-08-16] 关键物品/承诺特征识别：所有权/收藏/承诺语义的事实入 L3 永久层
/// （窝边草「素描原稿被向明初收藏」即此类——此前被 take(6) 挤出导致失忆）。
pub(crate) fn is_key_fact(text: &str) -> bool {
    const MARKERS: [&str; 18] = [
        "素描",
        "原稿",
        "画作",
        "收藏",
        "画好了",
        "亲手画",
        "承诺",
        "答应",
        "约定",
        "发誓",
        "保证",
        "送给",
        "赠予",
        "收到",
        "留在",
        "拥有",
        "贴身",
        "存着",
    ];
    MARKERS.iter().any(|m| text.contains(m))
}

/// S4 (吞噬 denova director_plan): 导演计划 LLM 输出块。
/// LLM 按 ST-26 指令输出「【导演计划】{...json...}」或「【导演计划】none」。
#[derive(Debug, Clone)]
pub(crate) enum DirectorPlanUpdate {
    /// 【导演计划】none —— 无需更新现有计划
    Skip,
    /// 解析出的新导演计划
    Set(DirectorPlanOutput),
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DirectorPlanOutput {
    #[serde(default)]
    pub(crate) goal: String,
    #[serde(default)]
    pub(crate) pressure: Option<String>,
    #[serde(default)]
    pub(crate) cost: Option<String>,
    /// 任务书约定 LLM 输出 snake_case `hits_beats`；camelCase 也兼容。
    #[serde(default, alias = "hits_beats")]
    pub(crate) hits_beats: Vec<String>,
}

/// [fix 2026-08-15 结构根治] 思维自动折叠——fix 阶段输出结构化剥离。
/// fix prompt 约定：分析过程放 <thinking>…</thinking> 块、正文放 <story>…</story> 块。
/// 本函数按标签剥离（机器可解析，不依赖措辞猜测，根治堆关键词打地鼠——
/// 旧规则靠 <场景 标签/措辞前缀识别，fix 思维无标签则剥不掉，实踩泄漏）。
/// 返回剥离出的思维段；body 原地改写为纯正文（含 <story> 标签剥除）。
/// 无任何标签时返回空串，调用方 fallback 到 strip_heavy_fix_preamble（兜底旧模型无标签输出）。
pub(crate) fn strip_fix_thinking_blocks(body: &mut String) -> String {
    let mut work = body.trim().to_string();
    let mut thinking = String::new();
    // 剥离所有 <thinking>…</thinking> 块（可能多个）
    loop {
        let Some(open) = work.find("<thinking>") else { break };
        let Some(close_rel) = work[open + "<thinking>".len()..].find("</thinking>") else {
            // 有开无闭：标签残片污染，自开标签处截断（保留标签前内容）
            work.truncate(open);
            break;
        };
        let inner = work[open + "<thinking>".len()..open + "<thinking>".len() + close_rel].trim();
        append_thinking(&mut thinking, inner);
        // 剥离 thinking 块时连其前的紧邻换行一起删（thinking 通常独占段落），
        // 避免残留双换行把正文段落撑出空行（测试实踩：前文\n\n中间）。
        let cut_start = if open > 0 && work.as_bytes()[open - 1] == b'\n' {
            open - 1
        } else {
            open
        };
        work = format!(
            "{}{}",
            &work[..cut_start],
            &work[open + "<thinking>".len() + close_rel + "</thinking>".len()..]
        );
    }
    // <story>…</story> 包裹时取块内内容（剥标签）；标签前若有非 thinking 的普通
    // 文本（模型偶发把部分正文写在 <story> 外），作为正文前缀保留，避免丢正文。
    if let Some(open) = work.find("<story>") {
        let after_open = &work[open + "<story>".len()..];
        if let Some(close_rel) = after_open.find("</story>") {
            let inner = after_open[..close_rel].trim();
            let before = work[..open].trim();
            let mut merged = String::new();
            if !before.is_empty() {
                merged.push_str(before);
            }
            if !inner.is_empty() {
                if !merged.is_empty() {
                    merged.push('\n');
                }
                merged.push_str(inner);
            }
            *body = merged;
            return thinking;
        } else {
            // [fix 2026-08-15] <story> 开标签无 </story> 闭合（正文被 max_tokens
            // 截断/流中断时实踩）：同样剥掉开标签，保留其后内容作正文，
            // 避免「<story> 烛火在茶几上跳着…」标签残片泄漏进用户可见正文。
            let before = work[..open].trim();
            let inner = after_open.trim();
            let mut merged = String::new();
            if !before.is_empty() {
                merged.push_str(before);
            }
            if !inner.is_empty() {
                if !merged.is_empty() {
                    merged.push('\n');
                }
                merged.push_str(inner);
            }
            *body = merged;
            return thinking;
        }
    }
    // [P2 2026-08-15] 尾部元话语段截断：fix 阶段模型常在正文末尾追加
    // 「让我检查/再检查/让我重新组织/让我起草/让我输出最终版本」等自指段
    // （实踩 turn72 [13] 尾部：「让我检查…」×4、「---」×3、自我问答式审稿）。
    // 从最后一个自指段起点截断；若截断后正文以分隔线结尾则一并剔除。
    work = strip_trailing_metadiscourse(work);
    *body = work.trim().to_string();
    thinking
}
