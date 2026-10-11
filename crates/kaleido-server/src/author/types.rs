//! author/types —— AuthorDocEntry / AuthorLivePolicy / AuthorProject / 请求体。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorDocEntry {
    pub path: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playable: Option<String>,
    #[serde(default)]
    pub updated_at: String,
}

/// AZ-6 realtime save policy for project-bound sessions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorLivePolicy {
    /// Master switch (default true).
    #[serde(default = "default_live_enabled")]
    pub enabled: bool,
    /// Append live.md every N completed turns (min 1).
    #[serde(default = "default_live_every_n")]
    pub every_n: u32,
    /// Also write sessions/.../turns/{n}.md slices.
    #[serde(default)]
    pub write_turns: bool,
    /// Rewrite sessions/.../summary.md every N turns (0/None = off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write_summary_every_n: Option<u32>,
}

pub(crate) fn default_live_enabled() -> bool {
    true
}

pub(crate) fn default_live_every_n() -> u32 {
    1
}

impl Default for AuthorLivePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            every_n: 1,
            write_turns: false,
            write_summary_every_n: None,
        }
    }
}

impl AuthorLivePolicy {
    pub fn normalize(mut self) -> Self {
        if self.every_n == 0 {
            self.every_n = 1;
        }
        if let Some(0) = self.write_summary_every_n {
            self.write_summary_every_n = None;
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorProject {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub character_ids: Vec<String>,
    #[serde(default)]
    pub world_book_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_id: Option<String>,
    /// Relative works root: projects/{id}/
    pub works_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_playable: Option<String>,
    #[serde(default)]
    pub doc_index: Vec<AuthorDocEntry>,
    /// AZ-6: realtime live/turn/summary policy (applied on launch/bind).
    #[serde(default)]
    pub live_policy: AuthorLivePolicy,
    pub created_at: String,
    pub updated_at: String,
    /// Owning auth workspace (multi-user ready).
    #[serde(default)]
    pub workspace_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateProjectBody {
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) character_ids: Vec<String>,
    #[serde(default)]
    pub(crate) world_book_ids: Vec<String>,
    #[serde(default)]
    pub(crate) pack_id: Option<String>,
    #[serde(default)]
    pub(crate) default_playable: Option<String>,
    #[serde(default)]
    pub(crate) live_policy: Option<AuthorLivePolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatchProjectBody {
    #[serde(default)]
    pub(crate) title: Option<String>,
    #[serde(default)]
    pub(crate) character_ids: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) world_book_ids: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) pack_id: Option<Option<String>>,
    #[serde(default)]
    pub(crate) default_playable: Option<Option<String>>,
    #[serde(default)]
    pub(crate) doc_index: Option<Vec<AuthorDocEntry>>,
    #[serde(default)]
    pub(crate) live_policy: Option<AuthorLivePolicy>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BindSessionBody {
    pub(crate) session_id: String,
}
