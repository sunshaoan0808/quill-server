//! suggest/suggestions —— Suggestion / GuardIssue / ContinuationGuard / DimensionScore / 存取助手。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: String,
    pub project_id: String,
    pub chapter_id: String,
    pub chapter_version: u32,
    #[serde(default = "continue_tt")]
    pub task_type: String,
    #[serde(default = "append_ac")]
    pub action: String,
    #[serde(default)]
    pub instruction: String,
    #[serde(default)]
    pub content: String,
    #[serde(default = "pending_st")]
    pub status: String,
    #[serde(default)]
    pub source_text: String,
    #[serde(default)]
    pub created_at: String,
}

pub(crate) fn continue_tt() -> String {
    "continue".into()
}
pub(crate) fn append_ac() -> String {
    "append".into()
}
pub(crate) fn pending_st() -> String {
    "pending".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardIssue {
    #[serde(rename = "type")]
    pub kind: String,
    /// 七维：consistency/character/pacing/continuity/foreshadow/hook/aesthetic（吞噬 ainovel-cli editor.md）
    #[serde(default)]
    pub dimension: String,
    pub severity: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub candidate_quote: String,
    #[serde(default)]
    pub source_refs: Vec<Value>,
    #[serde(default)]
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationGuard {
    pub suggestion_id: String,
    pub chapter_version: u32,
    pub content_hash: String,
    pub status: String,
    #[serde(default)]
    pub issues: Vec<GuardIssue>,
    /// 七维评分 0-100（吞噬 ainovel-cli DimensionScore；LLM 给分，verdict 系统推导）
    #[serde(default)]
    pub dimensions: Vec<DimensionScore>,
    #[serde(default)]
    pub context_refs: Value,
    #[serde(default)]
    pub failure: String,
}

/// 七维评分单项（吞噬 ainovel-cli `domain.DimensionScore`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DimensionScore {
    pub dimension: String,
    pub score: i32,
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SuggestBody {
    #[serde(default = "continue_tt")]
    pub(crate) task_type: String,
    #[serde(default = "append_ac")]
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) instruction: String,
    #[serde(default)]
    pub(crate) source_text: String,
    #[serde(default)]
    pub(crate) model: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AcceptBody {
    #[serde(default)]
    pub(crate) content: Option<String>,
}

pub(crate) fn sg_path(pid: &str, sg: &str) -> String {
    format!("projects/{pid}/suggestions/{sg}.json")
}
pub(crate) fn guard_path(pid: &str, sg: &str) -> String {
    format!("projects/{pid}/guards/{sg}.json")
}

pub(crate) fn hash_content(s: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    format!("{:x}", h.finish())
}

pub(crate) fn read_sg(state: &AppState, ws: &str, pid: &str, sg: &str) -> Result<Suggestion, Response> {
    let body = state
        .works
        .read_text(ws, &sg_path(pid, sg))
        .map_err(|_| not_found("SUGGESTION_NOT_FOUND", format!("suggestion not found: {sg}")))?;
    serde_json::from_str::<Suggestion>(&body.content)
        .map_err(|e| internal("SUGGESTION_CORRUPT", format!("corrupt suggestion: {e}")))
}
