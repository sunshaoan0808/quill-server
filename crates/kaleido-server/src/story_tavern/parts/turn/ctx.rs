//! [P7 2026-10-06] 回合上下文参数包（设计文档 §3.1 P7）：把 run_main_stream_phase /
//! post_turn_memory_phase 原散装值签名（22/27 参数）中的 LLM 路由、身份标识、环境、
//! 计时与计数收编为单一结构体。基础设施句柄（stores/state/jobs/tx/hub）语义清晰，
//! 保持显式参数；回合局部值（full_text/chapter_body/roster_names/pack/works_store/
//! engine_tag/llm_advance_to）仍为独立参数。
#![allow(unused_imports)]
use super::super::*;

#[allow(clippy::struct_excessive_bools)]
pub(crate) struct TurnCtx {
    // LLM 路由
    pub base_bg: String,
    pub key_bg: String,
    pub model_bg: String,
    pub fallback_model_bg: String,
    pub prov_bg: String,
    pub sys_prompt_bg: String,
    pub user_bg: String,
    // 身份标识
    pub run_id_bg: String,
    pub session_id_bg: String,
    pub workspace_id_bg: String,
    pub player_msg_bg: String,
    // 环境
    pub cross_dir: std::path::PathBuf,
    pub embedding_base_bg: Option<String>,
    pub quality_bg: TurnQuality,
    pub resumed_bg: bool,
    pub next_turn_bg: u32,
    // 计时与计数
    pub turn_started: std::time::Instant,
    pub turn_started_ms: u64,
    pub used_fallback: bool,
    pub extra_llm_calls: u32,
}
