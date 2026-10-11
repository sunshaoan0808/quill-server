//! Fanqie crawler (S5-W4 real fetch).
// [机械拆分产物 2026-10-05] 头部 use 块经 `use super::*` glob 桥接供部件文件使用，
// 本文件不直接引用属预期；cargo fix 会误判未使用并删除（导致整片编译失败），
// 工程纪律见 docs/REFACTOR_20260927.md「遗留」节末条。
#![allow(unused_imports)]

//!
//! Route (wired in `main.rs`):
//! - `POST /api/v1/crawler/fanqie`
//!
//! Gate: public settings `crawlerEnabled` (default **false** — never auto-enable).
//! Real fetch for fanqienovel.com reader/page URLs + articleId;
//! failures return W19 diagnostic fields: `code`/`stage`/`retryable`/`hint`
//! (optional mock only when `mockOnFailure=true` for gate demos).

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

/// Resin proxy pool — auto-rotates IP on each new TCP connection.
/// 凭证从环境变量读取（M-1 修复）；未设置时不启用代理，不再回退硬编码凭据。

mod parts;
pub use parts::*;