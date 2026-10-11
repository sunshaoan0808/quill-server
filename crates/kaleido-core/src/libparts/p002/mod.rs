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

// [R6-2③ 2026-10-07] 拆分自单文件 1505 行：auth.rs（AuthStore）/ jobs.rs+
// jobs_ops.rs（JobStore 两组 impl 块）。settings/口令/会话持久化助手保留在本文件。
#[allow(unused_imports)]
pub(crate) mod auth;
#[allow(unused_imports)]
pub use auth::*;
#[allow(unused_imports)]
pub(crate) mod jobs;
#[allow(unused_imports)]
pub use jobs::*;
#[allow(unused_imports)]
pub(crate) mod jobs_ops;

pub(crate) fn json_settings_shell(content: &str) -> Value {
    serde_json::json!({ "state": { "raw": content } })
}

pub(crate) fn default_state_json(name: &str) -> String {
    match name {
        "partner-store" => serde_json::to_string_pretty(&serde_json::json!({
            "state": {
                "worldBooks": [],
                "characterCards": [],
                "selectedWorldBookId": null,
                "selectedCharacterCardId": null
            }
        }))
        .unwrap_or_else(|_| "{}".into()),
        "settings-store" => serde_json::to_string_pretty(&serde_json::json!({
            "state": {
                "modelInterface": "OpenAI",
                "llmBaseUrl": "",
                "llmApiKey": "[server]",
                "llmModel": "",
                "partnerChatPrompt": "你是一个体贴温和的伴侣，请用温暖、真实而细节丰富的语言与用户交谈，避免机器感。"
            }
        }))
        .unwrap_or_else(|_| "{}".into()),
        "author-projects" => "[]".into(),
        "chat-shelf-schedule" => serde_json::to_string_pretty(&serde_json::json!({
            "enabled": false,
            "intervalHours": 24,
            "minTurns": 3,
            "toPack": true,
            "source": "tavern",
            "lastRunAt": null,
            "lastResult": null
        })).unwrap_or_else(|_| "{}".into()),
        _ => "{}".into(),
    }
}

pub(crate) fn merge_settings_preserving_keys(existing: &mut Value, incoming: &Value) {
    let Some(ex_state) = existing.get_mut("state").and_then(|s| s.as_object_mut()) else {
        *existing = incoming.clone();
        return;
    };
    let Some(in_state) = incoming.get("state").and_then(|s| s.as_object()) else {
        return;
    };
    for (k, v) in in_state {
        if k == "llmApiKey" {
            let s = v.as_str().unwrap_or("");
            if s.is_empty() || s.contains('•') || s == "[server]" {
                continue; // keep existing secret marker / value
            }
        }
        if k == "llmBaseUrl" && v.as_str().map(|s| s.is_empty()).unwrap_or(false) {
            continue;
        }
        ex_state.insert(k.clone(), v.clone());
    }
}

pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .expect("argon2 hash")
        .to_string()
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// P2-3 审计修复：收紧敏感数据文件权限为 owner-only（0600），
/// 防止同机其他用户读取 users.json / sessions.json / partner-store 等。
// path 仅在 #[cfg(unix)] 块内使用——windows 编译时参数看似未用，勿改名
//（cargo fix 曾在 Windows 上误改为 _path，导致 Linux CI 编译失败 E0423）。
#[cfg_attr(not(unix), allow(unused_variables))]
pub(crate) fn restrict_owner_only(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(path)?;
        let current = meta.permissions().mode();
        let masked = current & 0o777;
        // 仅 owner 读写；strip group/other 读/写/执行
        let desired = masked & 0o700 | 0o600;
        if desired != masked {
            let mut perm = meta.permissions();
            perm.set_mode(desired);
            fs::set_permissions(path, perm)?;
        }
    }
    Ok(())
}

pub(crate) fn load_session_cap_config(data: &DataRoot) -> (usize, String) {
    let env_cap = std::env::var("KALEIDO_MAX_SESSIONS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50usize)
        .max(1);
    let env_policy = std::env::var("KALEIDO_SESSION_CAP_POLICY")
        .unwrap_or_else(|_| "auto_evict".into());
    let path = data.app_state_path("settings-store");
    if let Ok(raw) = fs::read_to_string(&path) {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            let state = v.get("state").cloned().unwrap_or(v);
            let cap = state
                .get("sessionMax")
                .or_else(|| state.get("maxSessions"))
                .and_then(|x| x.as_u64())
                .map(|n| n as usize)
                .filter(|n| *n >= 1)
                .unwrap_or(env_cap)
                .min(10_000);
            let policy = state
                .get("sessionCapPolicy")
                .and_then(|x| x.as_str())
                .unwrap_or(env_policy.as_str())
                .to_string();
            return (cap.max(1), AuthStore::normalize_policy(&policy));
        }
    }
    (env_cap, AuthStore::normalize_policy(&env_policy))
}

pub(crate) fn load_or_bootstrap_users(data: &DataRoot) -> CoreResult<HashMap<String, UserRecord>> {
    let path = data.state_file("users.json");
    if path.exists() {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(map) = serde_json::from_str::<HashMap<String, UserRecord>>(&raw) {
                if !map.is_empty() {
                    return Ok(map);
                }
            }
        }
    }

    let username = std::env::var("KALEIDO_ADMIN_USER").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("KALEIDO_ADMIN_PASSWORD").map_err(|_| {
        CoreError::Auth(
            "KALEIDO_ADMIN_PASSWORD is not set — refusing to bootstrap admin with a default password; set it before starting"
                .into(),
        )
    })?;
    if password.len() < 8 {
        eprintln!("[kaleido-core] WARNING: admin password shorter than 8 chars");
    }
    let user = UserRecord {
        username: username.clone(),
        password_hash: hash_password(&password),
        user_id: Uuid::new_v4().to_string(),
        workspace_id: Uuid::new_v4().to_string(),
        created_at: Some(Utc::now().to_rfc3339()),
        // P0-2 审计修复：bootstrap admin 默认带 admin 角色。
        is_admin: true,
    };
    let mut map = HashMap::new();
    map.insert(username, user);
    fs::write(&path, serde_json::to_string_pretty(&map)?)?;
    let _ = restrict_owner_only(&path);
    eprintln!(
        "[kaleido-core] bootstrapped admin into {} (set KALEIDO_ADMIN_PASSWORD in prod)",
        path.display()
    );
    let _ = data.append_audit(
        "auth.log",
        &format!("{} bootstrap admin users={}", Utc::now().to_rfc3339(), map.len()),
    );
    Ok(map)
}

pub(crate) fn load_sessions(data: &DataRoot) -> CoreResult<HashMap<String, SessionRecord>> {
    let path = data.state_file("sessions.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let raw = fs::read_to_string(path)?;
    let mut map: HashMap<String, SessionRecord> = serde_json::from_str(&raw)?;
    let now = Utc::now();
    map.retain(|_, s| s.expires_at > now);
    Ok(map)
}

pub(crate) fn persist_sessions(data: &DataRoot, sessions: &HashMap<String, SessionRecord>) -> CoreResult<()> {
    let path = data.state_file("sessions.json");
    fs::write(&path, serde_json::to_string_pretty(sessions)?)?;
    // P2-3 审计修复：sessions 含 bearer token，收紧为 owner-only。
    let _ = restrict_owner_only(&path);
    Ok(())
}
