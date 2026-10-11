#![allow(unused_imports)]
//! Background multi-stage job core (S5-W2 T2).
//!
//! Uses JobStore v2 (`create` / `push_event` / `complete` / `cancel`).
//! Routes (wired from main):
//!   POST /api/v1/background/start
//!   POST /api/v1/background/{stage}   — stage ∈ stage_one|items|character_card|pipeline
//!   POST /api/v1/background/stop
//!   GET  /api/v1/background/stream?id=...
//!   GET  /api/v1/background/runs/{id}           — W1+ status/checkpoint
//!   POST /api/v1/background/runs/{id}/resume    — W1+ resume from checkpoint
//!   POST /api/v1/background/apply    — map job result → Partner world books / cards
//!
//! Stages (structured templates aligned with upstream v0.9.2 shapes):
//!   stage_one      → worldBooks + characterNames
//!   items          → worldBooks + characterCards (full extract)
//!   character_card → single character card for characterName
//!   pipeline       → stage_one → items → deepen up to deepenMax character cards (checkpointed)

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use kaleido_core::{
    is_terminal_job_status, normalize_job_status, JobEvent, PartnerItem,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration as StdDuration;
use uuid::Uuid;

use crate::{map_core_err, session_from, stream_job_sse, AppState};
use crate::error_codes::*;

// [R6④b-2 2026-10-06] 拆分自单文件 2047 行。

#[allow(unused_imports)]
pub(crate) mod types;
#[allow(unused_imports)]
pub(crate) use types::*;
#[allow(unused_imports)]
pub(crate) mod runs;
#[allow(unused_imports)]
pub(crate) use runs::*;
#[allow(unused_imports)]
pub(crate) mod worker;
#[allow(unused_imports)]
pub(crate) use worker::*;
#[allow(unused_imports)]
pub(crate) mod llm;
#[allow(unused_imports)]
pub(crate) use llm::*;
#[allow(unused_imports)]
pub(crate) mod templates;
#[allow(unused_imports)]
pub(crate) use templates::*;
#[allow(unused_imports)]
pub(crate) mod apply;
#[allow(unused_imports)]
pub(crate) use apply::*;
