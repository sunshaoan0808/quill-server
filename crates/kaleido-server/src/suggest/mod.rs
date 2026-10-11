#![allow(unused_imports)]
//! 书架改编模式 A1: 章节文档实体 + CRUD（Scriverse 三件套的地基）。
//!
//! 章节存 `projects/{pid}/chapters/{chId}.json`（WorksFs jail）：
//! ```json
//! {"id":"ch1","title":"第一章","content":"正文…","versionNo":3,"updatedAt":"…","source":"manual|crawler"}
//! ```
//! Routes（挂 author router 旁，见 `suggest_router()`）：
//! - `GET    /api/v1/author/projects/{id}/chapters` — 列表
//! - `POST   /api/v1/author/projects/{id}/chapters` — 新建 `{title?, content?}`
//! - `GET    /api/v1/author/projects/{id}/chapters/{ch}` — 读
//! - `PUT    /api/v1/author/projects/{id}/chapters/{ch}` — 写正文（versionNo+1）
//! - `DELETE /api/v1/author/projects/{id}/chapters/{ch}` — 删

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{map_core_err, session_from, AppState};
use crate::error_codes::*;

// [R6④b-3 2026-10-06] 拆分自单文件 1762 行。

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/author/projects/{id}/chapters",
            get(list_chapters).post(create_chapter),
        )
        .route(
            "/api/v1/author/projects/{id}/chapters/{ch}",
            get(get_chapter).put(put_chapter).delete(delete_chapter),
        )
}

#[allow(unused_imports)]
pub(crate) mod types;
#[allow(unused_imports)]
pub(crate) use types::*;
#[allow(unused_imports)]
pub(crate) mod chapters;
#[allow(unused_imports)]
pub(crate) use chapters::*;
#[allow(unused_imports)]
pub(crate) mod suggestions;
#[allow(unused_imports)]
pub(crate) use suggestions::*;
#[allow(unused_imports)]
pub(crate) mod context;
#[allow(unused_imports)]
pub(crate) use context::*;
#[allow(unused_imports)]
pub(crate) mod guard;
#[allow(unused_imports)]
pub(crate) use guard::*;
#[allow(unused_imports)]
pub(crate) mod api;
#[allow(unused_imports)]
pub(crate) use api::*;
#[allow(unused_imports)]
pub(crate) mod triage;
#[allow(unused_imports)]
pub(crate) use triage::*;
#[allow(unused_imports)]
pub(crate) mod advance;
#[allow(unused_imports)]
pub(crate) use advance::*;
#[allow(unused_imports)]
pub(crate) mod views;
#[allow(unused_imports)]
pub(crate) use views::*;
#[allow(unused_imports)]
pub(crate) mod guard_run;
#[allow(unused_imports)]
pub(crate) use guard_run::*;
