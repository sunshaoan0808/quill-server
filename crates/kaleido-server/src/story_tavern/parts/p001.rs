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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeMemoryRequest {
    /// Optional character ID to focus analysis on; None = current focus character.
    pub character_id: Option<String>,
    /// Optional: apply proposed updates as MemoryPatch immediately.
    #[serde(default)]
    pub apply: bool,
}

/// U13 M1: Batch-optimization request for memory deduplication/merge.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptimizeMemoryRequest {
    /// Optional: apply resulting MemoryPatch immediately.
    #[serde(default)]
    pub apply: bool,
}

/// U13 M2: Explicit session compaction request.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactSessionRequest {
    /// Compression style: "conversation" (dialogue flow) / "bulletin" (key points) / "battle_report" (dual-agent battlefield).
    #[serde(default = "default_compact_style")]
    pub style: String,
}

pub(crate) fn default_compact_style() -> String {
    "conversation".into()
}

/// X5 (吞噬自 xiami writing_style.rs): 文笔风格分析请求。
/// `sourceText` 为小说样本；`workId` 可选（预留 pack narrative_style 落库，MVP 仅返回不落库）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleAnalysisRequest {
    /// 小说 TXT 正文或粘贴的小说样本。
    pub source_text: String,
    /// 可选作品/包 ID（预留，MVP 不落库）。
    #[serde(default)]
    pub work_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartRequest {
    pub message: String,
    #[serde(default)]
    pub model: Option<String>,
    /// P0 (吞噬 denova 写作三档): 回合正文生成质量档位。
    /// lite=单次直出（现状零回归）；standard=初稿后审稿+修订；
    /// heavy=context-plan→write→review→fix→final-gate 多轮管道。
    /// 缺省 lite；旧请求体（无该字段）兼容。
    /// 前端已暴露档位开关（写作台工具栏「写作档位」下拉，2026-08-08 a0fe865f），此处为 API 参数与默认值兜底。
    #[serde(default)]
    pub quality: Option<TurnQuality>,
}

/// P0: 回合叙事生成质量档位（对应 denova novel-lite / novel-standard / novel-heavy）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnQuality {
    Lite,
    Standard,
    Heavy,
}

impl Default for TurnQuality {
    fn default() -> Self {
        TurnQuality::Lite
    }
}

impl TurnQuality {
    #[allow(dead_code)] // [P7] TurnQuality 显示别名预留
    fn as_str(self) -> &'static str {
        match self {
            TurnQuality::Lite => "lite",
            TurnQuality::Standard => "standard",
            TurnQuality::Heavy => "heavy",
        }
    }
}

impl<'de> serde::Deserialize<'de> for TurnQuality {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "standard" => TurnQuality::Standard,
            "heavy" => TurnQuality::Heavy,
            _ => TurnQuality::Lite,
        })
    }
}

/// 归一化请求档位：None / 非法值 → lite（默认，保持现状）。
#[allow(dead_code)] // [P7] P6 收敛决议：文档化 test helper，生产零调用属预期
pub(crate) fn resolve_turn_quality(q: Option<TurnQuality>) -> TurnQuality {
    q.unwrap_or_default()
}

/// U11: 估算本回合 LLM 上下文载荷（字符数口径，与 build_tavern_system_prompt 实际输入近似）：
/// system prompt 字符 + 实际进 prompt 的对话窗口（最近 KEEP_RECENT_MESSAGES 条）字符
/// + 本轮用户消息 + 初稿字符。改为窗口载荷而非全量 messages（epoch 压缩后消息被裁剪，
/// 判据可回落，避免越过阈值后每回合触发压缩）。
pub(crate) const KEEP_RECENT_MESSAGES: usize = 12;
pub(crate) fn estimate_turn_ctx_chars(
    session: &TavernSession,
    sys_prompt: &str,
    user_msg: &str,
    draft: &str,
) -> usize {
    let hist: usize = session
        .messages
        .iter()
        .rev()
        .take(KEEP_RECENT_MESSAGES)
        .map(|m| m.content.chars().count())
        .sum();
    sys_prompt.chars().count() + hist + user_msg.chars().count() + draft.chars().count()
}

/// U11: 上下文阈值判定 —— 超过字符阈值或消息数兜底阈值才触发 epoch 压缩（不再机械 turn%8）。
pub(crate) fn should_epoch_compress(session: &TavernSession, ctx_chars: usize) -> bool {
    let t = u11_tuning();
    should_epoch_compress_with(session, ctx_chars, t.epoch_hard_chars, t.epoch_hard_messages)
}

/// 纯函数变体：阈值显式传入（供单测与 env 前的常量语义保持对齐）。
pub(crate) fn should_epoch_compress_with(
    session: &TavernSession,
    ctx_chars: usize,
    epoch_hard_chars: usize,
    epoch_hard_messages: usize,
) -> bool {
    ctx_chars >= epoch_hard_chars || session.messages.len() >= epoch_hard_messages
}

/// U11: 质量管道额外 LLM 调用次数（估算口径）：lite=0；standard=审稿+修订=2；
/// heavy=plan/write/review/fix/gate + fix×MAX_ROUNDS + memory（6..=8）。
pub(crate) fn refine_llm_calls(quality: TurnQuality) -> u32 {
    match quality {
        TurnQuality::Lite => 0,
        TurnQuality::Standard => 2,
        TurnQuality::Heavy => 5 + QUALITY_MAX_FIX_ROUNDS as u32 + 1,
    }
}

/// U11: 回合成本估算结果（纯函数，可单测）。
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct TurnCostEstimate {
    pub(crate) llm_calls: u32,
    pub(crate) est_in_tokens: u32,
    pub(crate) est_out_tokens: u32,
    pub(crate) est_cost_usd: f64,
}

/// U11: 回合级 LLM 调用次数与估算成本。口径：
/// - llm_calls = 主流(1) + 模型回退(0/1) + 质量管道 + 非阻塞后处理(extra_calls)
/// - in/out tokens 用 kaleido_core::estimate_tokens 启发式近似（CJK≈1.5 字符/token）
/// - cost = in/1M×单价 + out/1M×单价；模型名含 "free" 记 0（如 deepseek-v4-flash-free）
pub(crate) fn turn_cost_estimate(
    model: &str,
    quality: TurnQuality,
    sys_prompt: &str,
    user_msg: &str,
    draft: &str,
    extra_calls: u32,
    used_fallback: bool,
) -> TurnCostEstimate {
    let mode = kaleido_core::TokenEstimateMode::Heuristic;
    let main_in =
        kaleido_core::estimate_tokens(&format!("{}\n{}", sys_prompt, user_msg), mode).max(0) as u32;
    let draft_out = kaleido_core::estimate_tokens(draft, mode).max(0) as u32;
    let refine = refine_llm_calls(quality);
    // 质量管道每轮输入 ≈ 主调用输入规模；输出 ≈ 正文规模（write/fix 会重写正文）
    let writes = match quality {
        TurnQuality::Lite => 0,
        TurnQuality::Standard => 1,
        TurnQuality::Heavy => 2 + QUALITY_MAX_FIX_ROUNDS as u32,
    };
    let llm_calls = 1 + if used_fallback { 1 } else { 0 } + refine + extra_calls;
    let est_in_tokens = main_in * (1 + refine + extra_calls);
    let est_out_tokens = draft_out * (1 + writes);
    let free = model.to_ascii_lowercase().contains("free");
    let est_cost_usd = if free {
        0.0
    } else {
        (est_in_tokens as f64 / 1_000_000.0) * COST_PER_MT_IN
            + (est_out_tokens as f64 / 1_000_000.0) * COST_PER_MT_OUT
    };
    TurnCostEstimate {
        llm_calls,
        est_in_tokens,
        est_out_tokens,
        est_cost_usd,
    }
}

/// U11: 回合预算检查（硬超时看门狗，纯函数可单测）。
pub(crate) fn turn_over_budget(started_at_ms: u64, budget_ms: u64) -> bool {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64 >= started_at_ms.saturating_add(budget_ms))
        .unwrap_or(false)
}

/// U11: 回合级记账 JSON（合并进 job payload，GET /api/v1/jobs 可见；成功/失败路径通用）。
pub(crate) fn u11_accounting_json(
    model: &str,
    quality: TurnQuality,
    sys_prompt: &str,
    user_msg: &str,
    draft: &str,
    extra_calls: u32,
    used_fallback: bool,
    elapsed_ms: u64,
    resumed: bool,
    epoch: u32,
    turn: Option<u32>,
    err: Option<&str>,
) -> Value {
    let est = turn_cost_estimate(model, quality, sys_prompt, user_msg, draft, extra_calls, used_fallback);
    json!({
        "u11": {
            "turn": turn,
            "epoch": epoch,
            "resumed": resumed,
            "durationMs": elapsed_ms,
            "llmCalls": est.llm_calls,
            "estTokensIn": est.est_in_tokens,
            "estTokensOut": est.est_out_tokens,
            "estCostUsd": est.est_cost_usd,
            "error": err,
        }
    })
}

/// TurnQuality ↔ kaleido_core::Quality（同一语义三档，便于会话级持久化）。
impl From<kaleido_core::Quality> for TurnQuality {
    fn from(q: kaleido_core::Quality) -> Self {
        match q {
            kaleido_core::Quality::Lite => TurnQuality::Lite,
            kaleido_core::Quality::Standard => TurnQuality::Standard,
            kaleido_core::Quality::Heavy => TurnQuality::Heavy,
        }
    }
}

impl From<TurnQuality> for kaleido_core::Quality {
    fn from(q: TurnQuality) -> Self {
        match q {
            TurnQuality::Lite => kaleido_core::Quality::Lite,
            TurnQuality::Standard => kaleido_core::Quality::Standard,
            TurnQuality::Heavy => kaleido_core::Quality::Heavy,
        }
    }
}

/// P0 质量管道阶段（denova 角色映射）。仅供测试断言表达「期望阶段形状」，
/// 实际执行在 `run_quality_refine` 内。P4 起 `MemoryPatch` 归入 heavy 管道末尾
/// （产出可应用 patch 并回写 actor_states；回合尾 L0-L4 抽取仍为独立代码）。
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityStage {
    ContextPlan,
    Write,
    Review,
    Fix,
    FinalGate,
    MemoryPatch,
}

/// heavy 防死循环上限：final-gate 未达标最多再修复 2 轮。
pub const QUALITY_MAX_FIX_ROUNDS: usize = 2;

/// U11: 回合硬超时（秒）—— 主流 + 质量管道总预算。超时后以 error 终态释放锁（可 resume 重发）。
pub const TURN_HARD_TIMEOUT_SECS: u64 = 600;
/// U11: 上下文窗口 epoch 压缩阈值（估算字符数：system prompt + 全量对话 + 本轮消息 + 初稿）。
/// 超过阈值才触发压缩（替换机械 turn%8），由 build_tavern_system_prompt 实际载荷近似估算。
/// [fix 2026-08-15] 模型支持 1M 上下文（≈200 万中文字符），阈值 20K→200K，减少长文被反复压缩切碎。
pub const TURN_EPOCH_HARD_CHARS: usize = 200_000;
/// U11: 消息数兜底阈值（对话条数超限同样触发 epoch 压缩，防字符阈值未达但记忆层已膨胀）。
pub const TURN_EPOCH_HARD_MESSAGES: usize = 64;
/// U11: 成本估算单价（USD / 百万 token；DeepSeek 类廉价档默认，free 模型计 0）。
pub(crate) const COST_PER_MT_IN: f64 = 0.25;
pub(crate) const COST_PER_MT_OUT: f64 = 1.25;

/// U11: 可调参数集（env 覆盖，默认与上方 pub const 一致，零行为变更）。
///
/// 生产调参无需重编译，通过 env 注入：
/// - `KALEIDO_U11_EPOCH_CHARS`    —— 上下文压缩字符阈值（默认 200_000）
/// - `KALEIDO_U11_EPOCH_MESSAGES` —— 消息数兜底阈值（默认 64）
/// - `KALEIDO_U11_TIMEOUT_SECS`   —— 回合硬超时秒数（默认 600）
/// - `KALEIDO_U11_MAX_FIX_ROUNDS` —— heavy final-gate 最大修复轮数（默认 2）
/// 非法/缺失值回落默认，绝不影响进程启动。
#[derive(Debug, Clone, Copy)]
pub struct U11Tuning {
    pub epoch_hard_chars: usize,
    pub epoch_hard_messages: usize,
    pub hard_timeout_secs: u64,
    pub max_fix_rounds: usize,
}

impl Default for U11Tuning {
    fn default() -> Self {
        Self {
            epoch_hard_chars: TURN_EPOCH_HARD_CHARS,
            epoch_hard_messages: TURN_EPOCH_HARD_MESSAGES,
            hard_timeout_secs: TURN_HARD_TIMEOUT_SECS,
            max_fix_rounds: QUALITY_MAX_FIX_ROUNDS,
        }
    }
}

pub(crate) fn u11_env_u64(key: &str, def: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(def)
}

/// 从 env 解析 U11 调参（纯函数，供测试直接构造）。
pub fn u11_tuning_from_env() -> U11Tuning {
    U11Tuning {
        epoch_hard_chars: u11_env_u64("KALEIDO_U11_EPOCH_CHARS", TURN_EPOCH_HARD_CHARS as u64)
            as usize,
        epoch_hard_messages: u11_env_u64(
            "KALEIDO_U11_EPOCH_MESSAGES",
            TURN_EPOCH_HARD_MESSAGES as u64,
        ) as usize,
        hard_timeout_secs: u11_env_u64("KALEIDO_U11_TIMEOUT_SECS", TURN_HARD_TIMEOUT_SECS),
        max_fix_rounds: u11_env_u64(
            "KALEIDO_U11_MAX_FIX_ROUNDS",
            QUALITY_MAX_FIX_ROUNDS as u64,
        ) as usize,
    }
}

pub(crate) static U11_TUNING: std::sync::OnceLock<U11Tuning> = std::sync::OnceLock::new();

/// 进程级 U11 调参（首用解析并缓存；运行中改 env 不生效，属预期）。
pub fn u11_tuning() -> &'static U11Tuning {
    U11_TUNING.get_or_init(u11_tuning_from_env)
}

/// 回合硬超时（毫秒，调参后口径）。
pub(crate) fn u11_hard_timeout_ms() -> u64 {
    u11_tuning().hard_timeout_secs * 1000
}

/// 三档阶段计划。仅作为测试断言，反映 `run_quality_refine` 的实际执行形状
/// （重写自查用，非生产驱动数据）。manifest: lite=Write；standard=Write/Review/Fix；
/// heavy=ContextPlan/Write/Review/Fix/FinalGate/MemoryPatch（P4 起 MemoryPatch 归入管道）。
/// [P6 收敛 2026-08-25] 修改 `run_quality_refine` 阶段结构时，必须同步更新本函数与
/// `test_plan_quality_stages_semantics`（审查C「测试镜像漂移」防护——当前已比对一致）。
#[cfg(test)]
pub fn plan_quality_stages(quality: TurnQuality) -> Vec<QualityStage> {
    match quality {
        TurnQuality::Lite => vec![QualityStage::Write],
        TurnQuality::Standard => {
            vec![QualityStage::Write, QualityStage::Review, QualityStage::Fix]
        }
        TurnQuality::Heavy => vec![
            QualityStage::ContextPlan,
            QualityStage::Write,
            QualityStage::Review,
            QualityStage::Fix,
            QualityStage::FinalGate,
            QualityStage::MemoryPatch,
        ],
    }
}

// ─── P0 审稿/修订/终检 prompt 模板（中文）──────────────────────────────────

/// 审稿角色：只审不改，返回问题清单。
pub(crate) const QUALITY_REVIEW_SYS: &str = "你是一位资深网文审稿人。请只审不改，针对下方叙事正文输出简洁的中文问题清单，逐条一行。覆盖：连续性、人物声线、文风、剧情逻辑、节奏。每条格式：`[问题] 位置/维度：问题说明（含可执行修改建议）`。不要输出正文，不要输出赞扬。\n\n[P4/P7 2026-08-15] 必须额外检查：①与上一回合正文的重复度——若大面积复用句式/意象/段落，列为 blocker/continuity 问题；②角色声线——对照角色卡示例对白校验，若角色被写成气音/半截话/被动弱气而示例对白是泼辣直接，列为 blocker 问题。\n\n[H6 吞噬 humanizer-zh] 必须额外检查去 AI 味（列为 blocker/style 问题）：①破折号“——”一回合超过 1 处即列出；②三段式列举（A、B和C）；③“此外/彰显/格局/作为……的证明”类 AI 词；④“不仅仅是……而是……”排比；⑤与上一回合重复的整句/意象。";

/// [时间天气 v2 2026-08-17] LLM 剧情时间评估间隔（回合）：每 N 回合评估一次是否推进时间。
/// 低频省成本——正文 [时间推进] 标注与用户自然语言信号是主通道，LLM 评估是兜底校准。
pub(crate) const LLM_CLOCK_EVAL_INTERVAL: u32 = 4;
pub(crate) fn build_review_user(sys_prompt: &str, user_prompt: &str, plan: &str, draft: &str) -> String {
    format!(
        "## 系统约束（背景设定与 canon 规则，审稿时须对照）\n{}\n\n## 上下文计划（Required Beats 为硬节拍，逐条核对是否体现）\n{}\n\n## 玩家本轮输入\n{}\n\n## 待审正文\n{}\n\n请输出审稿问题清单。",
        sys_prompt, plan, user_prompt, draft
    )
}

/// 修订角色：按审稿意见修订，保留原文强段落与连续性。
/// [fix 2026-08-15 结构根治] 思维自动折叠：要求分析过程放入 <thinking>…</thinking> 块
/// （机器可解析，不依赖措辞猜测），正文放 <story>…</story> 块；后端按标签剥离折叠，
/// 替代堆关键词打地鼠（旧规则靠 <场景 标签/措辞前缀，fix 思维无标签则不认）。
pub(crate) const QUALITY_FIX_SYS: &str = "你是一位内容负责人。请根据审稿意见修订下方叙事正文。只修真正需要修的问题，保留原文的强段落、人物声线、有效情节节点与连续性。若问题清单为空或已达标，原样输出正文。\n\n[P4/P7 2026-08-15] 若审稿指出与上一回合重复：从上次结束位置**续写推进**而非重写，可保留的强段落最多 1-2 处。角色台词必须贴合角色卡示例对白声线，不得弱化为气音/半截话。\n\n输出结构（严格遵守）：如需先整理分析过程（对照审稿意见的取舍思路），放在 <thinking>…</thinking> 块内（该块不会展示给读者）；随后输出 <story>…</story> 块，内含修订后的完整正文。正文必须完整写在 <story> 内，不得省略、不得写「同上文」或「保持前文」；<story> 结束后不得追加任何「让我检查」「再检查」等自检文字。";
pub(crate) fn build_fix_user(sys_prompt: &str, review: &str, draft: &str) -> String {
    format!(
        "## 系统约束\n{}\n\n## 审稿意见\n{}\n\n## 待修订正文\n{}\n\n请输出：<thinking>…</thinking>（可选，分析过程）+ <story>…</story>（修订后的完整正文）。",
        sys_prompt, review, draft
    )
}

/// heavy 专属：上下文策划（轻量，重述写作目标/必须节拍/风格/风险）。
pub(crate) const QUALITY_PLAN_SYS: &str = "你是一位剧情策划。请为下方回合输出轻量上下文计划，逐行列出：写作范围、剧情目标、必须节拍、人物状态、canon 约束、风格约束、最易写崩的风险。不要输出正文。";
pub(crate) fn build_plan_user(sys_prompt: &str, user_prompt: &str) -> String {
    format!(
        "## 系统约束\n{}\n\n## 玩家本轮输入\n{}\n\n请输出上下文计划。",
        sys_prompt, user_prompt
    )
}

/// heavy 专属：正文作者（按计划写稿）。
pub(crate) const QUALITY_WRITE_SYS: &str = "你是一位正文作者。请严格依据上下文计划与系统约束撰写本回合叙事正文。直接输出正文，不要输出计划或思考过程。";
pub(crate) fn build_write_user(plan: &str, user_prompt: &str, draft: &str) -> String {
    format!(
        "## 上下文计划\n{}\n\n## 玩家本轮输入\n{}\n\n## 可参考的初稿（若缺失可重写）\n{}\n\n请输出正文。",
        plan, user_prompt, draft
    )
}

/// heavy 专属：终检（是否达标，不达标带明确问题交回 fix）。
/// [fix 2026-08-15] 传入 context plan：gate 必须对照 Required Beats 逐条核对，
/// 否则 canon 硬节拍（宿醉 6/6 未体现）在终检环节永远漏检。
pub(crate) const QUALITY_GATE_SYS: &str = "你是一位终检评审。请判断下方修订稿是否达标：连续性、canon/改写强度约束、是否回应了玩家输入、人物声线、语言质量。**必须逐条核对上下文计划中的 Required Beats（硬节拍）：任一硬节拍未在正文中体现即判未达标**。只输出 JSON：{\"pass\":true} 或 {\"pass\":false,\"problems\":\"简述未达标问题\"}。";
pub(crate) fn build_gate_user(sys_prompt: &str, user_prompt: &str, plan: &str, draft: &str) -> String {
    format!(
        "## 系统约束\n{}\n\n## 上下文计划（Required Beats 为硬节拍，逐条核对是否体现）\n{}\n\n## 玩家本轮输入\n{}\n\n## 待检修订稿\n{}\n\n请输出终检 JSON。",
        sys_prompt, plan, user_prompt, draft
    )
}

/// heavy 专属：Memory Patch（G5，吞噬 denova memory-patcher）。产出可应用
/// progress/character_state/world_state/foreshadowing 四类补丁 JSON。
pub(crate) const QUALITY_MEMORY_SYS: &str = "你是一位状态补丁生成器。请根据最终定稿正文生成可应用的记忆补丁，只输出 JSON：{\"progress\":\"...\",\"character_state\":{\"<characterId>\":\"当前状态摘要\"},\"world_state\":\"...\",\"foreshadowing\":\"...\"}。只基于正文中已发生内容，不臆造。";
pub(crate) fn build_memory_user(sys_prompt: &str, user_prompt: &str, final_text: &str) -> String {
    format!(
        "## 系统约束\n{}\n\n## 玩家本轮输入\n{}\n\n## 最终定稿正文\n{}\n\n请输出记忆补丁 JSON。",
        sys_prompt, user_prompt, final_text
    )
}

/// 解析终检结果：{"pass":true} 视为通过；显式 false 为未通过。
/// [refactor] 无法解析时不再保守放行（fail-open 会让提示词偏差击穿 heavy 质量门）：
/// 先截取回复中首个括号平衡的 {...} 再解析（容忍 ```json 围栏与前后缀文本），
/// 仍无法判定视为未通过（fail-closed），误判代价是多一轮 fix，由 max_fix_rounds 兜底。
pub fn gate_passes(verdict: &str) -> bool {
    let trimmed = verdict.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v.get("pass").and_then(|p| p.as_bool()).unwrap_or(false);
    }
    if let Some(obj) = extract_first_json_object(trimmed) {
        if let Ok(v) = serde_json::from_str::<Value>(&obj) {
            return v.get("pass").and_then(|p| p.as_bool()).unwrap_or(false);
        }
    }
    false
}

/// 截取字符串中首个括号深度平衡的 `{...}` 片段（字符串/转义感知）。
/// 终检模型常见输出形态：```json\n{"pass":true}\n```、`结论：{"pass":false,...}`。
pub(crate) fn extract_first_json_object(s: &str) -> Option<String> {
    let start = s.find('{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, b) in s.bytes().enumerate().skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// 可注入 LLM 调用（生产=远程，测试=本地 mock）。避免在管道内硬编码网络。
/// 返回 `Send` future：生产路径在 `tokio::spawn` 内被 await（Send 约束）。
pub trait QualityLlm {
    fn call(
        &self,
        system: &str,
        user: &str,
    ) -> std::pin::Pin<
        Box<dyn futures_util::Future<Output = Result<String, String>> + Send + '_>,
    >;
}

/// 生产实现：走现有 `stream_chat_completions`（复用 llm_stream helper）。
pub(crate) struct RemoteQualityLlm<'a> {
    pub(crate) base_url: &'a str,
    pub(crate) api_key: &'a str,
    pub(crate) model: &'a str,
    /// G6: provider protocol for call-side dispatch.
    pub(crate) provider_kind: &'a str,
}

impl QualityLlm for RemoteQualityLlm<'_> {
    fn call(
        &self,
        system: &str,
        user: &str,
    ) -> std::pin::Pin<
        Box<dyn futures_util::Future<Output = Result<String, String>> + Send + '_>,
    > {
        let base_url = self.base_url.to_string();
        let api_key = self.api_key.to_string();
        let model = self.model.to_string();
        let prov_kind = self.provider_kind.to_string();
        let sys = system.to_string();
        let usr = user.to_string();
        Box::pin(async move {
            stream_chat_completions_dispatch(
                &base_url, &api_key, &model, &prov_kind, &sys, &usr, 0.35, 32768, 90, |_| true,
            )
            .await
        })
    }
}

/// P4: stage 模板回退。skill 有对应 template 则用之，否则回退既有 const prompt（零回归）。
pub(crate) fn stage_sys<'a>(
    skill: Option<&'a SkillDoc>,
    select: impl FnOnce(&'a SkillDoc) -> Option<&'a str>,
    fallback: &'static str,
) -> &'a str {
    skill.and_then(select).unwrap_or(fallback)
}

/// P0 质量管道：lite 直出 draft（零回归）；standard 一次审稿+一次修订；
/// heavy context-plan→write→review→fix→final-gate，gate 未达标最多 `QUALITY_MAX_FIX_ROUNDS` 次修复；
/// heavy 末尾插 MemoryPatch 阶段（best-effort，产出可应用 patch，失败不挡正文）。
/// 任意核心子步骤失败均返回 Err，调用方保留初稿（best-effort，不挡正文）。
/// 返回最终正文 + 可选 memory patch（heavy 专属）。
pub(crate) async fn run_quality_refine<L: QualityLlm>(
    quality: TurnQuality,
    llm: &L,
    sys_prompt: &str,
    user_prompt: &str,
    draft: &str,
    skill: Option<&SkillDoc>,
    thinking_out: &mut String,
) -> Result<(String, Option<String>), String> {
    match quality {
        TurnQuality::Lite => Ok((strip_lite_reasoning_leak(draft.to_string()), None)),
        TurnQuality::Standard => {
            let review_sys = stage_sys(skill, |s| s.templates.review.as_deref(), QUALITY_REVIEW_SYS);
            let fix_sys = stage_sys(skill, |s| s.templates.fix.as_deref(), QUALITY_FIX_SYS);
            let review = llm
                .call(review_sys, &build_review_user(sys_prompt, user_prompt, "", draft))
                .await?;
            let fix_out = llm
                .call(fix_sys, &build_fix_user(sys_prompt, &review, draft))
                .await?;
            // [fix 2026-08-15 结构根治] fix 输出结构化剥离（<thinking>/<story> 标签），
            // 思维段汇入 thinking_out（前端 monologue 折叠展示），正文纯净化后返回。
            let mut fixed_clean = fix_out;
            append_thinking(thinking_out, &strip_fix_thinking_blocks(&mut fixed_clean));
            Ok((fixed_clean, None))
        }
        TurnQuality::Heavy => {
            let plan_sys = stage_sys(skill, |s| s.templates.plan.as_deref(), QUALITY_PLAN_SYS);
            let write_sys = stage_sys(skill, |s| s.templates.write.as_deref(), QUALITY_WRITE_SYS);
            let review_sys = stage_sys(skill, |s| s.templates.review.as_deref(), QUALITY_REVIEW_SYS);
            let fix_sys = stage_sys(skill, |s| s.templates.fix.as_deref(), QUALITY_FIX_SYS);
            let gate_sys = stage_sys(skill, |s| s.templates.gate.as_deref(), QUALITY_GATE_SYS);
            let memory_sys = stage_sys(skill, |s| s.templates.memory.as_deref(), QUALITY_MEMORY_SYS);
            let plan = llm
                .call(plan_sys, &build_plan_user(sys_prompt, user_prompt))
                .await?;
            let written = llm
                .call(write_sys, &build_write_user(&plan, user_prompt, draft))
                .await?;
            let review = llm
                .call(review_sys, &build_review_user(sys_prompt, user_prompt, &plan, &written))
                .await?;
            let mut fixed = llm
                .call(fix_sys, &build_fix_user(sys_prompt, &review, &written))
                .await?;
            // [fix 2026-08-15 结构根治] 每轮 fix 输出结构化剥离：gate 只见纯正文
            // （避免 <thinking>/<story> 标签干扰达标判定），思维段汇入折叠展示。
            append_thinking(thinking_out, &strip_fix_thinking_blocks(&mut fixed));
            let max_fix_rounds = u11_tuning().max_fix_rounds;
            for round in 0..max_fix_rounds {
                let verdict = llm
                    .call(gate_sys, &build_gate_user(sys_prompt, user_prompt, &plan, &fixed))
                    .await?;
                if gate_passes(&verdict) {
                    break;
                }
                fixed = llm
                    .call(
                        fix_sys,
                        &build_fix_user(
                            sys_prompt,
                            &format!("（第{}轮终检未达标）{}", round + 1, verdict),
                            &fixed,
                        ),
                    )
                    .await?;
                append_thinking(thinking_out, &strip_fix_thinking_blocks(&mut fixed));
            }
            // P4: heavy MemoryPatch 阶段（best-effort；失败仅记日志，不挡正文）
            let mut memory_patch = llm
                .call(memory_sys, &build_memory_user(sys_prompt, user_prompt, &fixed))
                .await
                .ok();
            // P4.1: 记忆补丁结构契约 + 修复循环（吸收自 OpenHanako rolling-summary-format：
            // JSON 解析失败/缺 character_state 时附原因重试 1 次，仍失败保持 None 降级）。
            if let Some(patch) = memory_patch.as_deref() {
                let issues = kaleido_core::st_memory_contract::validate_memory_patch(patch);
                if !issues.ok {
                    tracing::warn!(
                        issues = ?issues.issues,
                        "记忆补丁结构校验失败，尝试格式修复"
                    );
                    let repair_prompt =
                        kaleido_core::st_memory_contract::build_memory_patch_repair_prompt();
                    let repair_input = kaleido_core::st_memory_contract::build_memory_patch_repair_input(
                        &issues.issues,
                        patch,
                    );
                    if let Ok(repaired) = llm.call(&repair_prompt, &repair_input).await {
                        let after =
                            kaleido_core::st_memory_contract::validate_memory_patch(&repaired);
                        if after.ok {
                            tracing::info!("记忆补丁格式修复成功");
                            memory_patch = Some(repaired);
                        } else {
                            tracing::warn!(
                                issues = ?after.issues,
                                "记忆补丁修复后仍不合规，保持降级（不应用）"
                            );
                        }
                    }
                }
            }
            Ok((fixed, memory_patch))
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct StopPayload {
    #[serde(alias = "runId")]
    pub(crate) run_id: String,
}

// ─── Router ──────────────────────────────────────────────────────────────────

pub fn router() -> Router<AppState> {
    Router::new()
        // [主动关怀 P2 2026-10-11] 显式提醒创建/列表/取消（调度器在 main 启动期拉起）
        .merge(crate::proactive::router())
        // 会话分享链接（吸收 DEEIX conversation share：快照式公开只读）
        .merge(crate::story_tavern::share::router())
        .route(
            "/api/v1/story-tavern/packs",
            get(list_packs).post(upsert_pack),
        )
        .route(
            "/api/v1/story-tavern/packs/demo",
            post(ensure_demo),
        )
        .route(
            "/api/v1/story-tavern/packs/import",
            post(import_pack_zip),
        )
        .route(
            "/api/v1/story-tavern/packs/{id}",
            get(get_pack).delete(delete_pack),
        )
        .route(
            "/api/v1/story-tavern/packs/{id}/export.zip",
            get(export_pack_zip),
        )
        // SoulLink 吸收：档案维护端点（analyze 增量分析 / refine 精编 / purge 删楼溯源清理）
        .route(
            "/api/v1/story-tavern/packs/{id}/archive/analyze",
            post(archive_analyze),
        )
        .route(
            "/api/v1/story-tavern/packs/{id}/archive/refine",
            post(archive_refine),
        )
        .route(
            "/api/v1/story-tavern/packs/{id}/archive/purge/{source}",
            delete(archive_purge),
        )
        // 2026-08-15: 存量 pack 角色补抽入口（修复多 pack 少角色根因 ②）
        // 手动触发 spawn_auto_cast_extraction：阈值相对判断 + 断链容错 + 会话另存检测全在抽取函数内。
        .route(
            "/api/v1/story-tavern/packs/{id}/cast-extract",
            post(trigger_cast_extract),
        )
        .route(
            "/api/v1/story-tavern/packs/{id}/chapters/{*rel}",
            get(read_chapter).put(write_chapter),
        )
        .route(
            "/api/v1/story-tavern/packs/{id}/vector-index/rebuild",
            post(rebuild_pack_vector_index),
        )
        .route(
            "/api/v1/story-tavern/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}",
            get(get_session)
                .put(save_session)
                .patch(save_session)
                .delete(delete_session),
        )
        // ST-1: turn jobs
        .route(
            "/api/v1/story-tavern/sessions/{id}/turn",
            post(start_turn),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/stream",
            get(session_stream),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/mode",
            post(set_play_mode),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/tier",
            post(set_session_tier),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/opening",
            post(ensure_opening),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/side-branches",
            get(list_side_branches),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/side-branches/enter",
            post(enter_side_branch_route),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/focus",
            post(set_focus_character),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/rebind-vessel",
            post(rebind_vessel),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/saves",
            get(list_saves).post(create_save),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/worldline",
            get(get_worldline),
        )
        // T2 世界认知：U2 实体图谱 + U7 真相账本
        .route(
            "/api/v1/story-tavern/sessions/{id}/world/entities",
            get(world_entities),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/world/entities/{entity_id}",
            get(world_entity_detail),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/world/events",
            post(world_apply_events),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/world/truth",
            get(world_truth),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/world/truth/check",
            post(world_truth_check),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/assistant",
            post(assistant_chat),
        )
        // P0-1: story_command —— /rewind N 回退、/reroll 重生成
        .route(
            "/api/v1/story-tavern/sessions/{id}/rewind",
            post(rewind_session),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/reroll",
            post(reroll_session),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/messages/{mid}",
            axum::routing::delete(delete_message),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/messages/{mid}",
            axum::routing::put(edit_message),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/saves/{save_id}",
            axum::routing::delete(delete_save),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/saves/{save_id}/restore",
            post(restore_save),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/saves/{save_id}/fork",
            post(fork_save),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/fork-at",
            post(fork_at_message),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/stop",
            post(stop_turn),
        )
        // U13 M1: character memory analysis + batch optimization
        .route(
            "/api/v1/story-tavern/sessions/{id}/analyze-memory",
            post(analyze_memory),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/optimize-memory",
            post(optimize_memory),
        )
        // U13 M2: explicit session compaction API
        .route(
            "/api/v1/story-tavern/sessions/{id}/compact",
            post(compact_session),
        )
        // U13 M3: branch-level summary
        .route(
            "/api/v1/story-tavern/sessions/{id}/branches/{bid}/summary",
            get(branch_summary),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/director-plan",
            get(get_director_plan),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/director-plan/run",
            post(run_director_plan),
        )
        // G13: 导演工具面——导演 plan 提交（HTTP 版 submit_director_plan_update）。
        .route(
            "/api/v1/story-tavern/sessions/{id}/director-plan/submit",
            post(submit_director_plan),
        )
        // S5/S6 演出机只读 + 归档（吞噬 denova event_package / actor archive）
        .route(
            "/api/v1/story-tavern/sessions/{id}/director-config",
            get(get_director_config).put(put_director_config),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/event-packages",
            get(get_event_packages),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/last-event",
            get(get_last_event),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/actor-states",
            get(get_actor_states).put(put_actor_states),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/pockets",
            get(get_pockets).put(put_pockets),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/pockets-enabled",
            get(get_pockets_enabled).put(put_pockets_enabled),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/needs",
            get(get_needs).put(put_needs),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/needs/tick",
            post(tick_needs),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/growth",
            get(get_growth).put(put_growth),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/world-climate",
            get(get_world_climate).put(put_world_climate),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/chaos",
            get(get_chaos).put(put_chaos),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/chaos/tick",
            post(tick_chaos),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/milestones",
            get(get_milestones),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/objectives",
            get(get_objectives).post(create_objective),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/objectives/{oid}",
            axum::routing::put(update_objective).delete(delete_objective),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/ambitions",
            get(get_ambitions).post(create_ambition),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/dreams",
            get(get_dreams).post(push_dream),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/episodes",
            get(get_episodes).post(push_episode),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/journals",
            get(get_journals).post(create_journal_card),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/journals/{card_id}",
            axum::routing::put(update_journal_card).delete(delete_journal_card),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/journals/{card_id}/pin",
            post(toggle_pin_journal),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/journals/recall",
            post(recall_journals),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/journals/embed-missing",
            post(embed_missing_journals),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/relationships",
            get(get_relationships).put(put_relationships),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/relationships/tick",
            post(tick_relationships),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/messages/{msg_id}/swipe",
            get(get_swipe).put(put_swipe),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/storyline",
            get(get_storyline),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/promises",
            get(get_promises).post(create_promise),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/promises/{pid}",
            axum::routing::put(resolve_promise),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/preferences",
            get(get_preferences).put(put_preferences),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/presence",
            get(get_presence).put(put_presence),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/mood",
            get(get_mood),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/event-extract",
            get(get_event_extract).put(put_event_extract),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/timed-world-info",
            get(get_timed_world_info),
        )
        // [morphling Wave B3 2026-08-16] 章节剧情摘要账本（吸收自 BakemonoMemory
        // summary-memory-model）：GET 查看每章总结，PUT 手动修改（manual_edited 保护）。
        .route(
            "/api/v1/story-tavern/sessions/{id}/chapter-summaries",
            get(get_chapter_summaries),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/chapter-summaries/{chapter_id}",
            put(put_chapter_summary),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/actor-archive",
            get(list_actor_archives).post(archive_actor_state),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/check-history",
            get(get_check_history),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/actor-archive/restore",
            post(restore_actor_archive),
        )
        .route(
            "/api/v1/story-tavern/persona/{character_id}",
            get(get_persona).put(save_persona),
        )
        // T2 创作罗盘：per-work 的 {author_intent, current_focus} 读写。
        .route(
            "/api/v1/story-tavern/works/{work_id}/compass",
            get(get_compass).put(put_compass),
        )
        // X5 (吞噬自 xiami writing_style.rs): 作品级文笔风格分析 —— 样本采样 + 12 维分析。
        .route(
            "/api/v1/story-tavern/style-analysis",
            post(style_analysis),
        )
        // 整个 story-tavern 路由放宽 body 上限到 64MB（axum 默认 2MB 会拒长剧情/长剧本
        // 生成与附带上下文的请求 → 413 "Failed to buffer the request body: length limit exceeded"）。
        // 与 crawler/works 端点的 64MB 保持一致；handler 内的字符上限仍先于 413 生效。
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
}

// ─── Pack handlers ───────────────────────────────────────────────────────────

async fn list_packs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.packs.list() {
        Ok(list) => Json(json!({ "packs": list })).into_response(),
        Err(e) => map_core_err(e),
    }
}

async fn get_pack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.packs.get(&id) {
        Ok(p) => Json(p).into_response(),
        Err(e) => map_core_err(e),
    }
}

async fn upsert_pack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(pack): Json<StoryPack>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.packs.save(pack) {
        Ok(p) => Json(p).into_response(),
        Err(e) => map_core_err(e),
    }
}

async fn delete_pack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.packs.delete(&id) {
        Ok(()) => {
            let n = state.sessions_tavern.mark_pack_missing(&id).unwrap_or(0);
            Json(json!({ "ok": true, "sessionsMarked": n })).into_response()
        }
        Err(e) => map_core_err(e),
    }
}


async fn export_pack_zip(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.packs.export_zip(&id) {
        Ok(bytes) => {
            let filename = format!("{id}.zip");
            (
                [
                    (axum::http::header::CONTENT_TYPE, "application/zip"),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        &format!("attachment; filename=\"{filename}\""),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) => map_core_err(e),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportPackRequest {
    /// base64-encoded zip bytes
    zip_base64: String,
    #[serde(default)]
    id: Option<String>,
}

async fn import_pack_zip(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ImportPackRequest>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let raw = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        body.zip_base64.trim(),
    ) {
        Ok(b) => b,
        Err(e) => {
            return map_core_err(kaleido_core::CoreError::BadRequest(format!(
                "invalid base64: {e}"
            )));
        }
    };
    if raw.len() > 20 * 1024 * 1024 {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "zip too large (max 20MB)".into(),
        ));
    }
    match state.packs.import_zip(&raw, body.id) {
        Ok(p) => Json(p).into_response(),
        Err(e) => map_core_err(e),
    }
}

async fn ensure_demo(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.packs.ensure_demo_pack() {
        Ok(p) => Json(p).into_response(),
        Err(e) => map_core_err(e),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChapterBody {
    pub(crate) content: String,
}

async fn read_chapter(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, rel)): Path<(String, String)>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    match state.packs.read_chapter_body(&id, &rel) {
        Ok(content) => Json(json!({ "path": rel, "content": content })).into_response(),
        Err(e) => map_core_err(e),
    }
}
