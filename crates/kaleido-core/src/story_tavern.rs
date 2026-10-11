//! Story Tavern (ST-0): domain types + file-backed stores.
// [机械拆分产物 2026-10-05] 头部 use 块经 `use super::*` glob 桥接供部件文件使用，
// 本文件不直接引用属预期；cargo fix 会误判未使用并删除（导致整片编译失败），
// 工程纪律见 docs/REFACTOR_20260927.md「遗留」节末条。
#![allow(unused_imports)]

//! Layout (ADR-0002 / ADR-0003):
//! - `$DATA/story-packs/{id}/pack.json` + `chapters/*.md`
//! - `$DATA/tavern-sessions/tavern-session-{uuid}.json`
//! - `$DATA/tavern-persona/{characterId}.json`

use crate::{
    st_compass::{Compass, CompassStore},
    st_skimming::SkimIssue,
    text_hash, CoreError, CoreResult, DataRoot,
};
use crate::world_state::{EntityKind as WsEntityKind, WorldEntity, WorldEvent, WorldState};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};
use uuid::Uuid;

// ─── Enums ───────────────────────────────────────────────────────────────────

mod parts;
pub use parts::*;