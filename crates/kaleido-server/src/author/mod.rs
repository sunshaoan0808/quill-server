#![allow(unused_imports)]
//! Author Zone (AZ-1..AZ-6): AuthorProject CRUD + live + compose/launch + publish/inject + live policy.
//!
//! Routes:
//!   GET    /api/v1/author/projects
//!   POST   /api/v1/author/projects
//!   GET    /api/v1/author/projects/{id}
//!   PATCH  /api/v1/author/projects/{id}
//!   DELETE /api/v1/author/projects/{id}
//!   POST   /api/v1/author/projects/{id}/bind-session
//!   POST   /api/v1/author/projects/{id}/compose
//!   POST   /api/v1/author/projects/{id}/launch
//!   POST   /api/v1/author/projects/{id}/publish
//!   POST   /api/v1/author/projects/{id}/inject
//!
//! Truth: docs/AUTHOR_ZONE.md (AZ-6: P2/P4 strategy + configurable live)

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use kaleido_core::{
    ContentTier, CreateSessionRequest, EntryConfig, PackCharacterRef, PackSource, PartnerItem,
    PlayMode, Playable, StoryChapter, StoryNode, StoryPack, WorksFs, NodeExit,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{map_core_err, session_from, AppState};
use crate::error_codes::*;

// [R6④b 2026-10-06] 拆分自单文件 2049 行：types/crud/compose_launch/publish_inject。

pub(crate) const INDEX_NAME: &str = "author-projects";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/author/projects",
            get(list_projects).post(create_project),
        )
        .route(
            "/api/v1/author/projects/{id}",
            get(get_project).patch(patch_project).delete(delete_project),
        )
        .route(
            "/api/v1/author/projects/{id}/bind-session",
            post(bind_session),
        )
        .route(
            "/api/v1/author/projects/{id}/compose",
            post(compose_project),
        )
        .route(
            "/api/v1/author/projects/{id}/launch",
            post(launch_project),
        )
        .route(
            "/api/v1/author/projects/{id}/publish",
            post(publish_project),
        )
        .route(
            "/api/v1/author/projects/{id}/inject",
            post(inject_to_session),
        )
}

pub(crate) fn now() -> String {
    Utc::now().to_rfc3339()
}

pub(crate) fn safe_project_id(id: &str) -> Result<String, Response> {
    let s = id.trim();
    if s.is_empty()
        || s.contains('/')
        || s.contains('\\')
        || s.contains("..")
        || s.chars().any(|c| c.is_control())
    {
        return Err(bad_request("AUTHOR_BAD_ID",  "invalid project id"));
    }
    Ok(s.to_string())
}

use std::sync::Mutex;

/// Serializes load-modify-save on the author-projects index (lost-update under concurrent create/compose).
pub(crate) static AUTHOR_INDEX_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn with_index_mut<F, T>(state: &AppState, f: F) -> Result<T, Response>
where
    F: FnOnce(&mut Vec<AuthorProject>) -> Result<T, Response>,
{
    let _guard = AUTHOR_INDEX_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mut list = load_index_unlocked(state)?;
    let out = f(&mut list)?;
    save_index_unlocked(state, &list)?;
    Ok(out)
}

pub(crate) fn load_index(state: &AppState) -> Result<Vec<AuthorProject>, Response> {
    let _guard = AUTHOR_INDEX_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    load_index_unlocked(state)
}

pub(crate) fn load_index_unlocked(state: &AppState) -> Result<Vec<AuthorProject>, Response> {
    match state.app_state.load(INDEX_NAME) {
        Ok(raw) => {
            if raw.trim().is_empty() {
                return Ok(vec![]);
            }
            match serde_json::from_str::<Vec<AuthorProject>>(&raw) {
                Ok(v) => Ok(v),
                Err(e) => Err(internal("AUTHOR_INDEX_CORRUPT", format!("corrupt author-projects index: {e}"))),
            }
        }
        Err(kaleido_core::CoreError::NotFound(_)) => Ok(vec![]),
        Err(e) => Err(map_core_err(e)),
    }
}

#[allow(dead_code)]
pub(crate) fn save_index(state: &AppState, list: &[AuthorProject]) -> Result<(), Response> {
    let _guard = AUTHOR_INDEX_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    save_index_unlocked(state, list)
}

pub(crate) fn save_index_unlocked(state: &AppState, list: &[AuthorProject]) -> Result<(), Response> {
    let content = serde_json::to_string_pretty(list).map_err(|e| {
        internal("AUTHOR_SERIALIZE", format!("serialize index: {e}"))
    })?;
    state
        .app_state
        .save(INDEX_NAME, &content)
        .map_err(map_core_err)
}

pub(crate) fn scaffold_project_dirs(works: &WorksFs, workspace_id: &str, project_id: &str) -> Result<(), Response> {
    let root = format!("projects/{project_id}");
    for rel in [
        root.as_str(),
        &format!("{root}/canon"),
        &format!("{root}/imports"),
        &format!("{root}/sessions"),
        &format!("{root}/exports"),
    ] {
        if let Err(e) = works.mkdir(workspace_id, rel) {
            // idempotent: if already dir, ok
            match works.stat(workspace_id, rel) {
                Ok(st) if st.kind == "dir" => {}
                _ => return Err(map_core_err(e)),
            }
        }
    }
    // mirror project.json stub (filled by caller after)
    Ok(())
}

pub(crate) fn write_project_mirror(
    works: &WorksFs,
    workspace_id: &str,
    project: &AuthorProject,
) -> Result<(), Response> {
    let path = format!("projects/{}/project.json", project.id);
    let body = serde_json::to_string_pretty(project).map_err(|e| {
        internal("AUTHOR_SERIALIZE", format!("serialize project: {e}"))
    })?;
    works
        .write_text(workspace_id, &path, &body)
        .map_err(map_core_err)?;
    Ok(())
}

pub(crate) fn filter_workspace(list: Vec<AuthorProject>, workspace_id: &str) -> Vec<AuthorProject> {
    list.into_iter()
        .filter(|p| p.workspace_id.is_empty() || p.workspace_id == workspace_id)
        .collect()
}

#[allow(unused_imports)]
pub(crate) mod types;
#[allow(unused_imports)]
pub(crate) use types::*;
#[allow(unused_imports)]
pub(crate) mod crud;
#[allow(unused_imports)]
pub(crate) use crud::*;
#[allow(unused_imports)]
pub(crate) mod compose_launch;
#[allow(unused_imports)]
pub(crate) use compose_launch::*;
#[allow(unused_imports)]
pub(crate) mod publish_inject;
#[allow(unused_imports)]
pub(crate) use publish_inject::*;
