//! book_travel/types —— 常量表 + 请求/标签结构体（classify/start/stop/stream/runs）。（R6④a 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) const STEPS: &[&str] = &[
    "assemble",
    "plan_scene",
    "write_change_scene",
    "write_insert_beat",
    "judge_ending",
    "summarize_memory",
    "pipeline",
];

/// Ordered stages for mode=pipeline (one job, multi progress).
pub(crate) const PIPELINE_STEPS: &[&str] = &[
    "assemble",
    "plan_scene",
    "write_change_scene",
    "write_insert_beat",
    "judge_ending",
    "summarize_memory",
];

// ---------------------------------------------------------------------------
// Request / response models
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookTravelClassifyBody {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub genre_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookTravelLabels {
    pub genre: String,
    pub tone: String,
    pub pov: String,
    pub era: String,
    pub themes: Vec<String>,
    pub pacing: String,
    pub audience: String,
    pub mode: String,
    pub confidence: f64,
    pub notes: Vec<String>,
}

/// Shared body for start and /{step}.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookTravelStartBody {
    #[serde(default)]
    pub premise: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// assemble | plan_scene | write_change_scene | write_insert_beat | judge_ending | summarize_memory
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub step: Option<String>,
    /// Free-form user input / instruction for scene writers.
    #[serde(default)]
    pub user_input: Option<String>,
    /// Optional prior scene / plan context (from previous step result).
    #[serde(default)]
    pub context: Option<Value>,
    /// When `context` is empty, load this job's result (same workspace) as context.
    #[serde(default, alias = "previous_run_id", alias = "prevRunId")]
    pub previous_run_id: Option<String>,
    #[serde(default)]
    pub labels: Option<Value>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub payload: Option<Value>,
    /// When true, skip LLM and use templates (smoke / offline).
    #[serde(default, alias = "heuristicOnly", alias = "heuristic_only")]
    pub prefer_heuristic: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookTravelStopBody {
    #[serde(alias = "run_id", alias = "runId", alias = "jobId")]
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookTravelStreamQuery {
    #[serde(alias = "run_id", alias = "runId", alias = "jobId")]
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookTravelRunsQuery {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub status: Option<String>,
}
