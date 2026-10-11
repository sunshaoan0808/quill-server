//! background/types —— 常量表 + start/stop/stream 请求体 + stage 规范化。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) const STAGES: &[&str] = &["stage_one", "items", "character_card", "pipeline"];

/// Request body for `POST /api/v1/background/start` and `/{stage}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundStartBody {
    /// Free-form seed / premise / reference text.
    #[serde(default)]
    pub premise: Option<String>,
    /// Alias of premise (upstream uses `text`).
    #[serde(default)]
    pub text: Option<String>,
    /// Optional title for the generated background.
    #[serde(default)]
    pub title: Option<String>,
    /// Which stage: `stage_one` | `items` | `character_card`.
    #[serde(default)]
    pub mode: Option<String>,
    /// For character_card stage.
    #[serde(default)]
    pub character_name: Option<String>,
    /// Optional world-book context for character_card.
    #[serde(default)]
    pub world_book_context: Option<String>,
    /// stage_one: include characterNames list (default true).
    #[serde(default)]
    pub include_character_names: Option<bool>,
    /// Optional model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Opaque extra payload merged into job.payload.
    #[serde(default)]
    pub payload: Option<Value>,
    /// Skip LLM; use heuristic templates (smoke / offline).
    #[serde(default)]
    pub prefer_heuristic: Option<bool>,
    /// Pipeline: max character cards to deepen (default 5; 0 = skip deepen).
    #[serde(default)]
    pub deepen_max: Option<usize>,
    /// Pipeline deepen policy: `all` (default, capped by deepenMax) | `first` | `none`.
    #[serde(default)]
    pub deepen_mode: Option<String>,
}

/// Request body for `POST /api/v1/background/stop`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundStopBody {
    #[serde(alias = "run_id", alias = "runId", alias = "jobId")]
    pub id: String,
}

/// Query for `GET /api/v1/background/stream`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundStreamQuery {
    #[serde(alias = "run_id", alias = "runId", alias = "jobId")]
    pub id: String,
}

pub(crate) fn normalize_stage(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_lowercase().replace('-', "_");
    match s.as_str() {
        "stage_one" | "stageone" | "one" => Some("stage_one".into()),
        "items" | "item" => Some("items".into()),
        "character_card" | "charactercard" | "card" | "character" => Some("character_card".into()),
        "pipeline" | "full" | "all" => Some("pipeline".into()),
        _ => None,
    }
}
