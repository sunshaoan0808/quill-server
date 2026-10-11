#![allow(unused_imports)]
//! BookTravel full pipeline (S5-W2 T3 + W2 jobization).
//!
//! Routes:
//!   POST /api/v1/book-travel/classify
//!   POST /api/v1/book-travel/start              (assemble default; mode=pipeline for full journey)
//!   POST /api/v1/book-travel/{step}            step ∈ assemble|plan_scene|write_change_scene|
//!                                                   write_insert_beat|judge_ending|summarize_memory|pipeline
//!   POST /api/v1/book-travel/stop
//!   GET  /api/v1/book-travel/stream?id=...
//!   GET  /api/v1/book-travel/runs[/{id}]       list/get book_travel jobs (workspace-scoped)
//!
//! Single steps still one job each. `pipeline` runs all travel steps in **one** job with
//! progress events, cancel checks, and result persisted under works/book-travel/.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use kaleido_core::{is_terminal_job_status, normalize_job_status, JobEvent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration as StdDuration;
use uuid::Uuid;

use crate::{map_core_err, session_from, session_from_any, stream_job_sse, AppState};
use crate::error_codes::*;

// [R6④a 2026-10-06] 拆分自单文件 2162 行。

#[allow(unused_imports)]
pub(crate) mod types;
#[allow(unused_imports)]
pub(crate) use types::*;
#[allow(unused_imports)]
pub(crate) mod classify;
#[allow(unused_imports)]
pub(crate) use classify::*;
#[allow(unused_imports)]
pub(crate) mod runs;
#[allow(unused_imports)]
pub(crate) use runs::*;
#[allow(unused_imports)]
pub(crate) mod session_stream;
#[allow(unused_imports)]
pub(crate) use session_stream::*;
#[allow(unused_imports)]
pub(crate) mod pipeline;
#[allow(unused_imports)]
pub(crate) use pipeline::*;
#[allow(unused_imports)]
pub(crate) mod persist;
#[allow(unused_imports)]
pub(crate) use persist::*;
#[cfg(test)]
mod tests;
