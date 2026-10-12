//! Story Tavern API (ST-0/1): packs / sessions / persona CRUD + turn jobs + LLM streaming.
// [机械拆分产物 2026-10-05] 头部 use 块经 `use super::*` glob 桥接供部件文件使用，
// 本文件不直接引用属预期；cargo fix 会误判未使用并删除（导致整片编译失败），
// 工程纪律见 docs/REFACTOR_20260927.md「遗留」节末条。
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
// X2 (吞噬自 xiami skimming.rs / emotional_hooks.rs / story_simulation.rs):
// 虾米三大质检模块 —— 读者速读分析 / 情绪钩子合同 / 剧情因果推演校验。
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
// X4 (吞噬自 xiami outline.rs): 大纲补丁影响分析 + 章节执行合同 —— 导演台计划与章节生成之间的合同约束层。
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

// ─── Request types ───────────────────────────────────────────────────────────

mod parts;
pub use parts::*;
/// 会话分享链接（吸收 DEEIX conversation share）。
pub mod share;