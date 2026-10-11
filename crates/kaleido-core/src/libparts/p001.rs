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

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("auth: {0}")]
    Auth(String),
    #[error("rate limited: {0}")]
    RateLimited(String),
    /// Auth session cap hit after GC + optional auto-evict (W12).
    #[error("session cap: {message}")]
    SessionCap {
        message: String,
        active: usize,
        cap: usize,
        policy: String,
    },
    #[error("not found: {0}")]
    NotFound(String),
    #[error("forbidden: {0}")]
    Forbidden(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Stable machine-readable API error (W11+). `details` is JSON object extras.
    #[error("{code}: {message}")]
    Coded {
        code: String,
        message: String,
        details: Value,
    },
    /// Revision CAS write conflict: on-disk revision no longer matches the base.
    #[error("conflict: {0}")]
    Conflict(String),
}

impl CoreError {
    pub fn coded(code: impl Into<String>, message: impl Into<String>) -> Self {
        CoreError::Coded {
            code: code.into(),
            message: message.into(),
            details: json!({}),
        }
    }

    pub fn coded_with(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Value,
    ) -> Self {
        CoreError::Coded {
            code: code.into(),
            message: message.into(),
            details: if details.is_object() { details } else { json!({ "extra": details }) },
        }
    }

    /// W11: file/content over Works text limit.
    pub fn works_too_large(kind: &str, size: u64, max_bytes: u64) -> Self {
        let code = match kind {
            "read" | "file" => "WORKS_FILE_TOO_LARGE",
            "append" => "WORKS_APPEND_TOO_LARGE",
            _ => "WORKS_CONTENT_TOO_LARGE",
        };
        let message = match kind {
            "read" | "file" => format!("file too large (max {max_bytes} bytes)"),
            "append" => format!("content too large after append (max {max_bytes} bytes)"),
            _ => format!("content too large (max {max_bytes} bytes)"),
        };
        CoreError::coded_with(
            code,
            message,
            json!({ "size": size, "maxBytes": max_bytes }),
        )
    }

    pub fn works_parent_missing() -> Self {
        CoreError::coded(
            "WORKS_PARENT_MISSING",
            "parent directory does not exist; mkdir first",
        )
    }

    pub fn works_binary() -> Self {
        CoreError::coded("WORKS_BINARY_REJECTED", "binary files are not allowed")
    }

    pub fn works_not_utf8() -> Self {
        CoreError::coded("WORKS_NOT_UTF8", "file is not valid UTF-8 text")
    }

    pub fn works_not_file() -> Self {
        CoreError::coded("WORKS_NOT_FILE", "not a file")
    }

    pub fn works_is_dir() -> Self {
        CoreError::coded("WORKS_IS_DIR", "path is a directory")
    }

    pub fn works_path_escape(msg: impl Into<String>) -> Self {
        CoreError::Coded {
            code: "WORKS_PATH_ESCAPE".into(),
            message: msg.into(),
            details: json!({}),
        }
    }

    pub fn works_path_traversal() -> Self {
        CoreError::coded("WORKS_PATH_TRAVERSAL", "path traversal is not allowed")
    }

    pub fn works_absolute_path() -> Self {
        CoreError::coded("WORKS_ABSOLUTE_PATH", "absolute paths are not allowed")
    }

    pub fn works_not_found(path: impl Into<String>) -> Self {
        let path = path.into();
        CoreError::Coded {
            code: "WORKS_NOT_FOUND".into(),
            message: format!("path {path}"),
            details: json!({ "path": path }),
        }
    }
}

pub type CoreResult<T> = Result<T, CoreError>;

/// Configurable data root (env KALEIDO_DATA).
#[derive(Debug, Clone)]
pub struct DataRoot {
    root: PathBuf,
}

/// Brand directory under the data root, with legacy fallback:
/// prefer `<root>/Kaleido/<name>`; if it does not exist but the pre-rename
/// `<root>/MuseAI/<name>` does, keep using the legacy location (no data
/// migration shock for existing deployments).
pub fn brand_dir(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let new = root.join("Kaleido").join(name);
    let legacy = root.join("MuseAI").join(name);
    if new.exists() || !legacy.exists() { new } else { legacy }
}

impl DataRoot {
    pub fn new(root: impl Into<PathBuf>) -> CoreResult<Self> {
        let root = root.into();
        let me = Self { root };
        me.ensure_layout()?;
        Ok(me)
    }

    pub fn from_env() -> CoreResult<Self> {
        // KALEIDO_DATA primary, legacy MUSEAI_DATA fallback (rename compat).
        let root = std::env::var("KALEIDO_DATA")
            .or_else(|_| std::env::var("MUSEAI_DATA"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("./data"));
        Self::new(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure_layout(&self) -> CoreResult<()> {
        for sub in [
            "state",
            "sessions",
            "jobs",
            "artifacts",
            "audit",
            "secrets",
            "works",
            // Brand dir (Kaleido); legacy MuseAI/ dirs keep working via brand_dir()
            "Kaleido/agent-sessions",
            "Kaleido/config",
            "web",
            // Story Tavern (ST-0)
            "story-packs",
            "tavern-sessions",
            "tavern-persona",
            "tavern-saves",
            "cross-session",
        ] {
            fs::create_dir_all(self.root.join(sub))?;
        }
        Ok(())
    }

    pub fn story_packs_dir(&self) -> PathBuf {
        self.root.join("story-packs")
    }

    pub fn tavern_sessions_dir(&self) -> PathBuf {
        self.root.join("tavern-sessions")
    }

    pub fn tavern_persona_dir(&self) -> PathBuf {
        self.root.join("tavern-persona")
    }

    pub fn tavern_saves_dir(&self) -> PathBuf {
        self.root.join("tavern-saves")
    }

    pub fn cross_session_dir(&self) -> PathBuf {
        self.root.join("cross-session")
    }

    pub fn state_file(&self, name: &str) -> PathBuf {
        self.root.join("state").join(name)
    }

    pub fn jobs_dir(&self) -> PathBuf {
        self.root.join("jobs")
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// Upstream-compatible: $DATA/Kaleido/agent-sessions (legacy MuseAI fallback)
    pub fn agent_sessions_dir(&self) -> PathBuf {
        crate::brand_dir(&self.root, "agent-sessions")
    }

    /// Upstream-compatible app state: $DATA/Kaleido/config/{name}.json (legacy fallback)
    pub fn app_state_path(&self, name: &str) -> PathBuf {
        let safe = name.replace(['/', '\\'], "_");
        crate::brand_dir(&self.root, "config").join(format!("{safe}.json"))
    }

    pub fn audit_path(&self, name: &str) -> PathBuf {
        self.root.join("audit").join(name)
    }

    pub fn append_audit(&self, name: &str, line: &str) -> CoreResult<()> {
        let path = self.audit_path(name);
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{line}")?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRecord {
    pub username: String,
    pub password_hash: String,
    pub user_id: String,
    pub workspace_id: String,
    #[serde(default)]
    pub created_at: Option<String>,
    /// P0-2 审计修复：admin 角色标记；任意登录用户可访问 /api/v1/ai/* 会被 403 拦截。
    /// 老 users.json 文件反序列化时默认 `false`，bootstrap admin 时设为 `true`。
    #[serde(default)]
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub token: String,
    pub user_id: String,
    pub username: String,
    pub workspace_id: String,
    pub expires_at: DateTime<Utc>,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    /// P0-2 审计修复：admin 角色标记缓存（登录时从 UserRecord 复制）；
    /// 老 sessions.json 反序列化时默认 `false`，新登录自动重新计算。
    #[serde(default)]
    pub is_admin: bool,
}

/// W12: auth session capacity snapshot (not story-tavern sessions).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCapStats {
    pub active: usize,
    pub cap: usize,
    pub free: usize,
    /// `auto_evict` (default) | `reject`
    pub policy: String,
    pub ttl_hours: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_expires_at: Option<String>,
    pub expired_present: usize,
}

/// Canonical job kinds for jobs v2. Unknown kinds map to `other` on create if not listed.
pub const JOB_KINDS: &[&str] = &[
    "background",
    "book_travel",
    "outline",
    "agent",
    "chat",
    "other",
    "noop",
    "test",
];

/// Normalize legacy chat statuses onto the jobs-v2 enum.
/// Legacy: done → succeeded, error → failed, stopped → cancelled.
pub fn normalize_job_status(status: &str) -> String {
    match status {
        "done" => "succeeded".into(),
        "error" => "failed".into(),
        "stopped" => "cancelled".into(),
        "queued" | "running" | "succeeded" | "failed" | "cancelled" => status.into(),
        other => other.into(),
    }
}

pub fn is_terminal_job_status(status: &str) -> bool {
    matches!(
        normalize_job_status(status).as_str(),
        "succeeded" | "failed" | "cancelled"
    )
}

pub fn is_active_job_status(status: &str) -> bool {
    matches!(
        normalize_job_status(status).as_str(),
        "queued" | "running"
    )
}

/// [SSRF 加固 2026-08-15, 吸收 6fef9d12] 校验 LLM base URL：必须 http/https、
/// 非空 hostname，且禁回环/私网地址。
///
/// [审计修复 核心存储与认证-6] host 不再只做字面量匹配：先做 WHATWG 风格的
/// IP 规范化 —— 十进制/十六进制/八进制整数与缺省段短形式（`2130706433`、
/// `0x7f000001`、`0177.0.0.1`、`127.1`）均解析为规范 IPv4/IPv6 后再判回环/私网，
/// 与浏览器/getaddrinfo 的实际解析语义一致；形似数字 IP 却无法规范化的（浏览器
/// 同样视为非法 URL）一律 fail-closed 拒绝。userinfo 伪装（`http://evil.com@127.0.0.1`）
/// 取 `@` 之后真实主机；`[::ffff:127.0.0.1]` 等内嵌 IPv4 按对应 IPv4 递归判定。
///
/// 放行机制（精确白名单，非全局开关）：`KALEIDO_ALLOW_LOCAL_LLM` 按逗号分隔
/// 列举允许的 host（如 `127.0.0.1,localhost`），仅这些 host 豁免（IP 字面量按
/// 规范化形式比对）；其余私网/回环一律拒绝。`=1` 仍兼容为「全放行」但强烈
/// 不建议（会绕过全部防护）。
pub fn validate_llm_base_url(raw: &str) -> CoreResult<()> {
    let allow_raw = std::env::var("KALEIDO_ALLOW_LOCAL_LLM").unwrap_or_default();
    let allow_all = allow_raw.trim() == "1";
    let allow_hosts: Vec<String> = allow_raw.split(',').filter_map(canonical_host_token).collect();
    validate_llm_base_url_with_allow(raw, allow_all, &allow_hosts)
}

/// 纯函数内核（白名单由调用方注入，便于测试，避免进程级 env 竞态；同 P10
/// `parse_max_concurrent` 的直测纯函数思路）。注入的条目在比对前同样按
/// `canonical_host_token` 规范化，宽松 IP 字面量条目（如 `0x7f000001`）与
/// 规范化后的 URL host 按同一形式命中；无法规范化的条目永不放行。
pub(crate) fn validate_llm_base_url_with_allow(raw: &str, allow_all: bool, allow_hosts: &[String]) -> CoreResult<()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return Err(CoreError::BadRequest(
            "llmBaseUrl must start with http:// or https://".into(),
        ));
    }
    let authority = lower
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split(|c| c == '/' || c == '?' || c == '#')
        .next()
        .unwrap_or("");
    // userinfo 伪装：`http://evil.com@127.0.0.1/` 实际连接的是 `@` 之后的主机。
    let hostport = authority.rsplit('@').next().unwrap_or("");
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        hostport.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return Err(CoreError::BadRequest("llmBaseUrl has no hostname".into()));
    }
    // IP 规范化：宽松 IP 字面量 → 规范地址；普通 DNS 域名 → 原样放行到域名分支。
    let parsed_ip = normalize_host_ip(host).map_err(|_| {
        CoreError::BadRequest(
            "llmBaseUrl: ambiguous numeric IP host (use canonical dotted form)".into(),
        )
    })?;
    let host_token = match parsed_ip {
        Some(ip) => ip.to_string(),
        None => host.strip_suffix('.').unwrap_or(host).to_string(),
    };
    if let Some(kind) = forbidden_host_kind(parsed_ip, &host_token) {
        // 白名单条目与 URL host 双方按规范化形式比对（宽松 IP 字面量 / 大小写 /
        // 尾点均归一，同 `canonical_host_token` 契约）；无法规范化的条目原样保留，
        // 永不与规范化 host 相等（fail-closed，不静默扩大白名单）。
        let allow_canon: Vec<String> =
            allow_hosts.iter().filter_map(|h| canonical_host_token(h)).collect();
        if allow_all || allow_canon.iter().any(|h| h == &host_token) {
            return Ok(());
        }
        if kind == "loopback" {
            return Err(CoreError::BadRequest(
                "llmBaseUrl: localhost/loopback not allowed (set KALEIDO_ALLOW_LOCAL_LLM=127.0.0.1,localhost to override)".into(),
            ));
        }
        return Err(CoreError::BadRequest("llmBaseUrl: private IP not allowed".into()));
    }
    Ok(())
}

/// WHATWG URL 标准的 IPv4 number 解析：单段支持十进制 / `0x` 十六进制 /
/// 前导 `0` 八进制。空段、非法数字或溢出返回 `None`。
pub(crate) fn parse_ipv4_number(part: &str) -> Option<u64> {
    let (radix, digits): (u32, &str) =
        if let Some(rest) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            (16, rest)
        } else if part.len() > 1 && part.starts_with('0') {
            (8, &part[1..])
        } else {
            (10, part)
        };
    if digits.is_empty() {
        return None;
    }
    let valid = match radix {
        16 => digits.bytes().all(|b| b.is_ascii_hexdigit()),
        8 => digits.bytes().all(|b| (b'0'..=b'7').contains(&b)),
        _ => digits.bytes().all(|b| b.is_ascii_digit()),
    };
    if !valid {
        return None;
    }
    u64::from_str_radix(digits, radix).ok()
}

/// 将 host 解析为规范 IP：
/// - `Ok(Some(ip))`：host 是 IPv4/IPv6 字面量（含 WHATWG 宽松形式），已规范化；
/// - `Ok(None)`：host 是普通 DNS 域名（非 IP 字面量形态）；
/// - `Err(())`：形似数字 IP 却无法规范化（浏览器同样视为非法 URL）→ 调用方 fail-closed。
pub(crate) fn normalize_host_ip(host: &str) -> Result<Option<IpAddr>, ()> {
    if host.contains(':') {
        // 只有 `[]` 内剥出来的裸 IPv6 字面量会带 `:` 到这里（非括号形式的 `:` 已按端口切掉）。
        return host
            .parse::<Ipv6Addr>()
            .map(|ip| Some(IpAddr::V6(ip)))
            .map_err(|_| ());
    }
    let mut parts: Vec<&str> = host.split('.').collect();
    if parts.len() > 1 && parts.last() == Some(&"") {
        parts.pop(); // 容忍单个结尾点：`127.0.0.1.`
    }
    let n = parts.len();
    if n > 4 {
        return Ok(None);
    }
    // WHATWG「ends in a number」：仅当末段形如数字（十进制或 0x 十六进制）才按 IPv4 解析，
    // 避免误伤 `dead.beef.be` 之类十六进制风格的真实域名。
    let last = parts[n - 1];
    let ends_in_number = if let Some(rest) = last.strip_prefix("0x").or_else(|| last.strip_prefix("0X")) {
        rest.chars().all(|c| c.is_ascii_hexdigit()) // `0x` 空尾段也算数字形 → 下方解析 fail-closed
    } else {
        !last.is_empty() && last.chars().all(|c| c.is_ascii_digit())
    };
    if !ends_in_number {
        return Ok(None);
    }
    // WHATWG IPv4 parser：第 i 段（0-based，除末段）占自高起的第 i 个八位组（×256^(3-i)），
    // 末段占剩余低字节（上限 256^(5-n)-1）。与 inet_aton/getaddrinfo 的宽松语义一致：
    // `127.1` → 127.0.0.1，`2130706433` → 127.0.0.1，`0x7f.1` → 127.0.0.1。
    let mut ipv4: u64 = 0;
    for (i, part) in parts.iter().enumerate() {
        let value = parse_ipv4_number(part).ok_or(())?;
        if i + 1 == n {
            let max: u64 = (1u64 << (8 * (5 - n as u64))) - 1;
            if value > max {
                return Err(());
            }
            ipv4 += value;
        } else {
            if value > 255 {
                return Err(());
            }
            ipv4 += value * (1u64 << (8 * (3 - i as u64)));
        }
    }
    Ok(Some(IpAddr::V4(Ipv4Addr::from(ipv4 as u32))))
}

/// 规范化后的 host 是否落入禁止访问（回环/私网）范围。
/// 返回 `Some("loopback")` / `Some("private")` 表示禁止（用于错误文案），`None` 表示放行。
pub(crate) fn forbidden_host_kind(parsed_ip: Option<IpAddr>, host_token: &str) -> Option<&'static str> {
    match parsed_ip {
        Some(IpAddr::V4(v4)) => ipv4_forbidden_kind(v4),
        Some(IpAddr::V6(v6)) => ipv6_forbidden_kind(v6),
        None => {
            // RFC 6761：`localhost` / `*.localhost` 由解析器保证指向回环。
            if host_token == "localhost" || host_token.ends_with(".localhost") {
                Some("loopback")
            } else {
                None
            }
        }
    }
}

pub(crate) fn ipv4_forbidden_kind(v4: Ipv4Addr) -> Option<&'static str> {
    let octets = v4.octets();
    if v4.is_loopback() // 127/8
        || v4.is_unspecified() // 0.0.0.0（多数协议栈会连到本机）
        || v4.is_broadcast()
        || octets[0] == 0
    {
        // 0/8「this network」：`http://0/`、`http://0x7f/` → 0.0.0.x 同样打回本机
        Some("loopback")
    } else if v4.is_private() // 10/8, 172.16/12, 192.168/16
        || v4.is_link_local()
    {
        // 169.254/16 云元数据服务（经典 SSRF 目标）
        Some("private")
    } else {
        None
    }
}

pub(crate) fn ipv6_forbidden_kind(v6: Ipv6Addr) -> Option<&'static str> {
    let seg = v6.segments();
    // 内嵌 IPv4：`::ffff:a.b.c.d`（v4-mapped）与已弃用的 `::a.b.c.d`（v4-compatible）→ 按对应 IPv4 判定。
    if seg[..5] == [0, 0, 0, 0, 0] && (seg[5] == 0xffff || seg[5] == 0) && (seg[6] != 0 || seg[7] != 0) {
        let v4 = Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            (seg[6] & 0xff) as u8,
            (seg[7] >> 8) as u8,
            (seg[7] & 0xff) as u8,
        );
        return ipv4_forbidden_kind(v4);
    }
    if v6.is_loopback() || v6.is_unspecified() {
        Some("loopback")
    } else if (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local（v6 私网）
        || (seg[0] & 0xffc0) == 0xfe80
    {
        // fe80::/10 link-local
        Some("private")
    } else {
        None
    }
}

/// `KALEIDO_ALLOW_LOCAL_LLM` 条目规范化：IP 字面量（含宽松形式）→ 规范 IP 字符串；
/// 普通域名 → 去结尾点小写。空条目返回 `None`；无法规范化的数字形条目原样保留
/// （规范化后的 host 永远不会与之相等，即该条目永不放行，不会静默扩大白名单）。
pub(crate) fn canonical_host_token(entry: &str) -> Option<String> {
    let lowered = entry.trim().to_ascii_lowercase();
    if lowered.is_empty() {
        return None;
    }
    let bare = lowered
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .unwrap_or(&lowered);
    match normalize_host_ip(bare) {
        Ok(Some(ip)) => Some(ip.to_string()),
        Ok(None) => Some(bare.strip_suffix('.').unwrap_or(bare).to_string()),
        Err(()) => Some(lowered),
    }
}

/// SSE / progress event attached to a job (also used for stream replay).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEvent {
    pub event_type: String,
    pub ts: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    /// D2: machine-readable code for eventType="error" events (P1-4 parity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JobEvent {
    pub fn progress(message: impl Into<String>, progress: f64) -> Self {
        Self {
            event_type: "progress".into(),
            ts: Utc::now(),
            message: Some(message.into()),
            progress: Some(progress.clamp(0.0, 1.0)),
            code: None,
            data: None,
        }
    }

    pub fn event(message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            event_type: "event".into(),
            ts: Utc::now(),
            message: Some(message.into()),
            progress: None,
            code: None,
            data,
        }
    }

    pub fn done(message: Option<String>) -> Self {
        Self {
            event_type: "done".into(),
            ts: Utc::now(),
            message,
            progress: Some(1.0),
            code: None,
            data: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::error_code("JOB_ERROR", message)
    }

    /// D2: error with a stable machine-readable code (P1-4 envelope parity).
    pub fn error_code(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            event_type: "error".into(),
            ts: Utc::now(),
            message: Some(message.into()),
            progress: None,
            code: Some(code.into()),
            data: None,
        }
    }
}

/// Disk + internal representation uses snake_case for S4 chat job JSON compat.
/// API responses use `to_api_json()` (camelCase + id alias).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRecord {
    pub run_id: String,
    pub kind: String,
    pub user_id: String,
    pub workspace_id: String,
    /// Canonical: queued | running | succeeded | failed | cancelled
    /// (legacy done/error/stopped are normalized on write)
    pub status: String,
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub meta: Value,
    /// Create payload (jobs v2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    /// 0.0–1.0 progress fraction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_message: Option<String>,
    /// Opaque resume cursor for long jobs (Background / BookTravel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Recent events for SSE replay (capped).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<JobEvent>,
}

impl JobRecord {
    /// Alias used by jobs v2 JSON responses.
    pub fn id(&self) -> &str {
        &self.run_id
    }

    pub fn to_api_json(&self) -> Value {
        json!({
            "id": self.run_id,
            "runId": self.run_id,
            "kind": self.kind,
            "userId": self.user_id,
            "workspaceId": self.workspace_id,
            "status": normalize_job_status(&self.status),
            "model": self.model,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
            "meta": self.meta,
            "payload": self.payload,
            "progress": self.progress,
            "progressMessage": self.progress_message,
            "cursor": self.cursor,
            "error": self.error,
            "result": self.result,
            "events": self.events,
        })
    }
}

/// Filters for `JobStore::list`.
#[derive(Debug, Clone, Default)]
pub struct JobListFilter {
    pub status: Option<String>,
    pub kind: Option<String>,
    pub user_id: Option<String>,
    pub workspace_id: Option<String>,
    pub limit: usize,
}

pub(crate) const JOB_EVENTS_CAP: usize = 64;

// --- Agent sessions (upstream mobile contract, camelCase JSON) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_blocks: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionRecord {
    pub id: String,
    pub title: String,
    pub saved_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_kind: Option<String>,
    #[serde(default)]
    pub messages: Vec<AgentSessionMessage>,
    #[serde(default)]
    pub selected_reference_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_outline_file: Option<String>,
    #[serde(default)]
    pub todos: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_compaction: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_archived: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_card_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_world_book_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic_role_loading_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_style_preset_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_style_preset_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_system_prompt_snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_travel_state: Option<Value>,
    // --- Morphling additions (Liyuan-inspired) ---
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panels: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub world_lines: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionSummary {
    pub id: String,
    pub title: String,
    pub saved_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_card_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_card_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_world_book_id: Option<String>,
}

impl From<&AgentSessionRecord> for AgentSessionSummary {
    fn from(r: &AgentSessionRecord) -> Self {
        Self {
            id: r.id.clone(),
            title: r.title.clone(),
            saved_at: r.saved_at,
            session_kind: r.session_kind.clone(),
            character_card_id: r.character_card_id.clone(),
            character_card_ids: r.character_card_ids.clone(),
            selected_world_book_id: r.selected_world_book_id.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AgentSessionStore {
    data: DataRoot,
}

impl AgentSessionStore {
    pub fn new(data: DataRoot) -> Self {
        Self { data }
    }

    fn dir(&self) -> PathBuf {
        self.data.agent_sessions_dir()
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir().join(format!("{id}.json"))
    }

    pub fn validate_id(id: &str) -> CoreResult<()> {
        // Prefix + strict charset only — no `/`, `\`, `..`, or path separators
        // (prevents dir.join("{id}.json") from escaping agent_sessions/).
        let prefix_ok =
            id.starts_with("partner-session-") || id.starts_with("story-session-");
        let charset_ok = !id.is_empty()
            && id.len() <= 128
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && !id.contains("..");
        if prefix_ok && charset_ok {
            Ok(())
        } else {
            Err(CoreError::Forbidden(
                "invalid session id (partner-session-*|story-session-* alnum/_/- only)".into(),
            ))
        }
    }

    pub fn list(
        &self,
        prefix: &str,
        session_kind: Option<&str>,
    ) -> CoreResult<Vec<AgentSessionSummary>> {
        if prefix != "partner-session-" && prefix != "story-session-" {
            return Err(CoreError::BadRequest("invalid session prefix".into()));
        }
        let mut out = Vec::new();
        let dir = self.dir();
        if !dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") {
                continue;
            }
            let id = name.trim_end_matches(".json").to_string();
            if !id.starts_with(prefix) {
                continue;
            }
            let Ok(raw) = fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(rec) = serde_json::from_str::<AgentSessionRecord>(&raw) else {
                continue;
            };
            if prefix == "story-session-" {
                let kind = rec.session_kind.as_deref().unwrap_or("story");
                let want = session_kind.unwrap_or("story");
                if kind != want {
                    continue;
                }
            }
            out.push(AgentSessionSummary::from(&rec));
        }
        out.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
        Ok(out)
    }

    pub fn load(&self, id: &str) -> CoreResult<AgentSessionRecord> {
        Self::validate_id(id)?;
        let path = self.path(id);
        if !path.exists() {
            return Err(CoreError::NotFound(format!("session {id}")));
        }
        let raw = fs::read_to_string(path)?;
        let rec: AgentSessionRecord = serde_json::from_str(&raw)?;
        if id.starts_with("story-session-")
            && !matches!(rec.session_kind.as_deref(), None | Some("story"))
        {
            return Err(CoreError::Forbidden("book travel sessions blocked".into()));
        }
        Ok(rec)
    }

    pub fn save(&self, mut record: AgentSessionRecord) -> CoreResult<AgentSessionSummary> {
        Self::validate_id(&record.id)?;
        if record.id.starts_with("story-session-") {
            match record.session_kind.as_deref() {
                None => record.session_kind = Some("story".into()),
                Some("story") => {}
                Some(_) => {
                    return Err(CoreError::Forbidden("book travel sessions blocked".into()));
                }
            }
        } else if record.session_kind.is_some() {
            return Err(CoreError::BadRequest(
                "chat session must not include sessionKind".into(),
            ));
        }
        if record.saved_at == 0 {
            record.saved_at = now_millis();
        }
        fs::create_dir_all(self.dir())?;
        fs::write(self.path(&record.id), serde_json::to_string_pretty(&record)?)?;
        Ok(AgentSessionSummary::from(&record))
    }

    pub fn delete(&self, id: &str) -> CoreResult<()> {
        Self::validate_id(id)?;
        let path = self.path(id);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn update_title(&self, id: &str, title: &str) -> CoreResult<AgentSessionSummary> {
        let mut rec = self.load(id)?;
        rec.title = title.to_string();
        rec.saved_at = now_millis();
        self.save(rec)
    }
}

#[derive(Clone)]
pub struct AppStateStore {
    data: DataRoot,
}

impl AppStateStore {
    pub fn new(data: DataRoot) -> Self {
        Self { data }
    }

    pub fn data_root(&self) -> &DataRoot {
        &self.data
    }

    /// Allowlist for S2 (partner + settings). Expand later.
    /// C2: `partner-store-*` prefix covers per-user scoped stores
    /// (`partner-store-{user_scope}.json`).
    pub fn is_allowed(name: &str) -> bool {
        if name.starts_with("partner-store-") {
            return true;
        }
        matches!(
            name,
            "partner-store"
                | "settings-store"
                | "style-presets"
                | "character-card-groups"
                | "works-store"
                | "author-projects"
                | "chat-shelf-schedule"
        )
    }

    pub fn load(&self, name: &str) -> CoreResult<String> {
        if !Self::is_allowed(name) {
            return Err(CoreError::Forbidden(format!("state {name} not allowed")));
        }
        let path = self.data.app_state_path(name);
        if !path.exists() {
            return Ok(default_state_json(name));
        }
        Ok(fs::read_to_string(path)?)
    }

    pub fn save(&self, name: &str, content: &str) -> CoreResult<()> {
        if !Self::is_allowed(name) {
            return Err(CoreError::Forbidden(format!("state {name} not allowed")));
        }
        // settings: never let client overwrite server-held secrets with empty if we inject later
        let path = self.data.app_state_path(name);
        if name == "settings-store" {
            let mut final_json: Value = serde_json::from_str(content)
                .unwrap_or_else(|_| json_settings_shell(content));
            if let Ok(existing) = fs::read_to_string(&path) {
                if let Ok(mut old) = serde_json::from_str::<Value>(&existing) {
                    merge_settings_preserving_keys(&mut old, &final_json);
                    final_json = old;
                }
            }
            // strip plaintext keys from client writes into secrets file instead
            if let Some(state) = final_json.get_mut("state").and_then(|s| s.as_object_mut()) {
                if let Some(key) = state.get("llmApiKey").and_then(|v| v.as_str()) {
                    if !key.is_empty() && !key.contains('•') && key != "[server]" {
                        let secrets = self.data.root().join("secrets").join("llm_api_key.txt");
                        let _ = fs::write(&secrets, key).map(|_| {
                            // M-6: restrict plaintext key file to owner (0600).
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                let _ = fs::set_permissions(&secrets, std::fs::Permissions::from_mode(0o600));
                            }
                        });
                        state.insert("llmApiKey".into(), Value::String("[server]".into()));
                    }
                }
            }
            fs::write(&path, serde_json::to_string_pretty(&final_json)?)?;
        } else {
            fs::write(&path, content)?;
        }
        // P2-3 审计修复：敏感 state 文件收紧为 owner-only。
        let _ = restrict_owner_only(&path);
        Ok(())
    }
}
