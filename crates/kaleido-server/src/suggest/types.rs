//! suggest/types —— ChapterDoc / 请求体 / id·路径·读取助手（struct 字段已提升）。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChapterDoc {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub content: String,
    /// 卷弧分组（吞噬 ainovel-cli Volume→Arc→Chapter 三层，Kaleido 化：可选，不动写入）
    #[serde(default)]
    pub volume: u32,
    #[serde(default)]
    pub arc: u32,
    #[serde(default = "one")]
    pub version_no: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default = "manual_src")]
    pub source: String,
}

pub(crate) fn one() -> u32 {
    1
}
pub(crate) fn manual_src() -> String {
    "manual".into()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NewChapterBody {
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) volume: u32,
    #[serde(default)]
    pub(crate) arc: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PutChapterBody {
    pub(crate) content: String,
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339()
}

pub(crate) fn safe_id(id: &str) -> Result<String, Response> {
    let s = id.trim();
    if s.is_empty() || s.contains('/') || s.contains('\\') || s.contains("..") || s.chars().any(|c| c.is_control()) {
        return Err(bad_request("CHAPTER_BAD_ID", "invalid id"));
    }
    Ok(s.to_string())
}

pub(crate) fn ch_path(pid: &str, ch: &str) -> String {
    format!("projects/{pid}/chapters/{ch}.json")
}

pub(crate) fn read_ch(state: &AppState, ws: &str, pid: &str, ch: &str) -> Result<ChapterDoc, Response> {
    let body = state
        .works
        .read_text(ws, &ch_path(pid, ch))
        .map_err(|_| not_found("CHAPTER_NOT_FOUND", format!("chapter not found: {ch}")))?;
    serde_json::from_str::<ChapterDoc>(&body.content)
        .map_err(|e| internal("CHAPTER_CORRUPT", format!("corrupt chapter: {e}")))
}
