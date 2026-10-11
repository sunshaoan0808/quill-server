#![allow(unused_imports)]
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;
use super::*;

pub(crate) fn validate_workspace_id(id: &str) -> CoreResult<()> {
    if id.is_empty() || id.len() > 128 {
        return Err(CoreError::coded(
            "WORKS_INVALID_WORKSPACE",
            "invalid workspace_id",
        ));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(CoreError::coded(
            "WORKS_INVALID_WORKSPACE",
            "invalid workspace_id",
        ));
    }
    Ok(())
}

/// Normalize a client-supplied relative path. Rejects absolute / drive / escape.
pub(crate) fn normalize_rel_path(input: &str) -> CoreResult<String> {
    let s = input.trim();
    let s = s.trim_start_matches("./");
    if s.is_empty() || s == "." {
        return Ok(String::new());
    }
    // Reject absolute (unix + windows)
    if s.starts_with('/') || s.starts_with('\\') {
        return Err(CoreError::works_absolute_path());
    }
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        return Err(CoreError::works_absolute_path());
    }
    if s.contains('\0') {
        return Err(CoreError::coded("WORKS_INVALID_PATH", "invalid path"));
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in s.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            // Logical escape attempt — always reject (no stack pop that could leave jail)
            return Err(CoreError::works_path_traversal());
        }
        if part.contains(':') {
            return Err(CoreError::coded("WORKS_INVALID_PATH", "invalid path component"));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

pub(crate) fn ensure_under_jail(path: &Path, jail: &Path) -> CoreResult<()> {
    if path == jail {
        return Ok(());
    }
    if path.starts_with(jail) {
        // Extra guard: next component boundary (starts_with can be fooled by prefix names
        // only if jail lacks trailing sep — Path::starts_with is component-aware on Rust).
        return Ok(());
    }
    Err(CoreError::works_path_escape("path escapes works jail"))
}

pub(crate) fn display_rel(abs: &Path, jail: &Path) -> CoreResult<String> {
    if abs == jail {
        return Ok(String::new());
    }
    let rel = abs
        .strip_prefix(jail)
        .map_err(|_| CoreError::works_path_escape("path escapes works jail"))?;
    Ok(rel.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn system_time_ms(t: Option<SystemTime>) -> Option<u64> {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
}

pub(crate) fn is_probably_text(path: &Path, len: u64) -> bool {
    if len == 0 {
        return true;
    }
    let sample_len = std::cmp::min(len, 8192) as usize;
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    let sample = &bytes[..sample_len.min(bytes.len())];
    if sample.contains(&0) {
        return false;
    }
    // Reject if high ratio of non-text control chars
    let bad = sample
        .iter()
        .filter(|&&b| b < 0x09 || (b > 0x0d && b < 0x20))
        .count();
    bad * 20 < sample.len() // allow up to 5% weird controls
}
