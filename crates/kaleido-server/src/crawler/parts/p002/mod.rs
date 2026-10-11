#![allow(unused_imports)]
#![allow(unused_imports)]
use axum::{
    extract::State,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
use kaleido_core::JobStore;
use crate::encoding_sniff::decode_text;
use crate::error_codes::*;
use super::*;
use kaleido_core::{is_terminal_job_status, normalize_job_status, JobEvent, JobListFilter};
use crate::{map_core_err, session_from, AppState};
use std::path::PathBuf as FsPathBuf;

// [R6-3① 2026-10-07] 拆分自单文件 1449 行：html/pua/shelf/toc/auto_cast。

#[allow(unused_imports)]
pub(crate) mod html;
#[allow(unused_imports)]
pub(crate) use html::*;
#[allow(unused_imports)]
pub(crate) mod pua;
#[allow(unused_imports)]
pub(crate) use pua::*;
#[allow(unused_imports)]
pub(crate) mod shelf;
#[allow(unused_imports)]
pub(crate) use shelf::*;
#[allow(unused_imports)]
pub(crate) mod toc;
#[allow(unused_imports)]
pub(crate) use toc::*;
#[allow(unused_imports)]
pub(crate) mod auto_cast;
#[allow(unused_imports)]
pub(crate) use auto_cast::*;
