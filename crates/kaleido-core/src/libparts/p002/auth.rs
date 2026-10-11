//! p002/auth —— AuthStore（argon2 口令哈希 / 登录 / 会话签发）。
//! [R6-2③ 2026-10-07] 自 p002.rs 机械迁入。
#![allow(unused_imports)]

use super::*;

#[derive(Clone)]
pub struct AuthStore {
    data: DataRoot,
    users: Arc<Mutex<HashMap<String, UserRecord>>>,
    sessions: Arc<Mutex<HashMap<String, SessionRecord>>>,
    login_attempts: Arc<Mutex<HashMap<String, (DateTime<Utc>, u32)>>>,
    session_ttl_hours: i64,
    max_login_per_window: u32,
    login_window_secs: i64,
    #[allow(dead_code)] // [P7] 引导值仅作 session_cap_live 初值来源；运行时统一读 live 覆盖
    max_sessions: usize,
    /// `auto_evict` (default) | `reject`
    #[allow(dead_code)] // [P7] 同上——normalized 读取走 session_cap_live.1
    session_cap_policy: String,
    /// Live overrides (settings PATCH) — (cap, policy). None fields keep boot value.
    session_cap_live: Arc<Mutex<(usize, String)>>,
}

impl AuthStore {
    pub fn load(data: DataRoot) -> CoreResult<Self> {
        let users = load_or_bootstrap_users(&data)?;
        let sessions = load_sessions(&data).unwrap_or_default();
        // Settings-store can override env for cap/policy (W12).
        let (max_sessions, policy) = load_session_cap_config(&data);
        Ok(Self {
            data,
            users: Arc::new(Mutex::new(users)),
            sessions: Arc::new(Mutex::new(sessions)),
            login_attempts: Arc::new(Mutex::new(HashMap::new())),
            session_ttl_hours: std::env::var("KALEIDO_SESSION_TTL_HOURS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(12),
            max_login_per_window: std::env::var("KALEIDO_LOGIN_MAX_ATTEMPTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(10),
            login_window_secs: std::env::var("KALEIDO_LOGIN_WINDOW_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(300),
            max_sessions,
            session_cap_policy: policy.clone(),
            session_cap_live: Arc::new(Mutex::new((max_sessions, policy))),
        })
    }

    pub fn data_root(&self) -> &DataRoot {
        &self.data
    }

    fn check_login_rate(&self, key: &str) -> CoreResult<()> {
        let mut map = self.login_attempts.lock();
        let now = Utc::now();
        let entry = map.entry(key.to_string()).or_insert((now, 0));
        if (now - entry.0).num_seconds() > self.login_window_secs {
            *entry = (now, 0);
        }
        entry.1 += 1;
        if entry.1 > self.max_login_per_window {
            return Err(CoreError::RateLimited(format!(
                "too many login attempts for {key}; retry later"
            )));
        }
        Ok(())
    }

    pub fn login(&self, username: &str, password: &str, rate_key: &str) -> CoreResult<SessionRecord> {
        self.check_login_rate(rate_key)?;
        self.check_login_rate(&format!("user:{username}"))?;

        {
            let mut sessions = self.sessions.lock();
            // Drop expired first so dogfood / multi-device does not stick at max_sessions
            let now = Utc::now();
            let before = sessions.len();
            sessions.retain(|_, s| s.expires_at > now);
            let expired_dropped = before.saturating_sub(sessions.len());
            if expired_dropped > 0 {
                let _ = persist_sessions(&self.data, &sessions);
                let _ = self.data.append_audit(
                    "auth.log",
                    &format!(
                        "{} session_gc expired={} active={} cap={}",
                        Utc::now().to_rfc3339(),
                        expired_dropped,
                        sessions.len(),
                        self.effective_max_sessions()
                    ),
                );
            }
            // auto_evict: free slots by oldest until room for one new login
            let policy = self.session_cap_policy_normalized();
            let cap = self.effective_max_sessions();
            if sessions.len() >= cap && policy == "auto_evict" {
                let need = sessions.len() + 1 - cap;
                let mut victims: Vec<(String, DateTime<Utc>, DateTime<Utc>)> = sessions
                    .iter()
                    .map(|(k, s)| {
                        let created = s.created_at.unwrap_or(s.expires_at);
                        (k.clone(), s.expires_at, created)
                    })
                    .collect();
                victims.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.2.cmp(&b.2)));
                let mut evicted = 0usize;
                for (tok, _, _) in victims.into_iter().take(need) {
                    sessions.remove(&tok);
                    evicted += 1;
                    let _ = self.data.append_audit(
                        "auth.log",
                        &format!(
                            "{} session_evict token={} reason=max_sessions policy=auto_evict",
                            Utc::now().to_rfc3339(),
                            &tok[..8.min(tok.len())]
                        ),
                    );
                }
                if evicted > 0 {
                    let _ = persist_sessions(&self.data, &sessions);
                }
            }
            if sessions.len() >= cap {
                return Err(CoreError::SessionCap {
                    message: format!(
                        "too many active sessions ({}/{})",
                        sessions.len(),
                        cap
                    ),
                    active: sessions.len(),
                    cap,
                    policy,
                });
            }
        }

        let users = self.users.lock();
        let user = users.get(username).ok_or_else(|| {
            let _ = self.data.append_audit(
                "auth.log",
                &format!("{} login user={} fail=unknown", Utc::now().to_rfc3339(), username),
            );
            CoreError::Auth("invalid credentials".into())
        })?;
        if !verify_password(password, &user.password_hash) {
            let _ = self.data.append_audit(
                "auth.log",
                &format!("{} login user={} fail=badpass", Utc::now().to_rfc3339(), username),
            );
            return Err(CoreError::Auth("invalid credentials".into()));
        }

        let token = Uuid::new_v4().to_string();
        let expires = Utc::now() + Duration::hours(self.session_ttl_hours);
        let rec = SessionRecord {
            token: token.clone(),
            user_id: user.user_id.clone(),
            username: user.username.clone(),
            workspace_id: user.workspace_id.clone(),
            expires_at: expires,
            created_at: Some(Utc::now()),
            // P0-2 审计修复：登录时缓存 admin 角色，下游 handler 直接读取 SessionRecord
            // 即可判定 admin，避免每次都查 users map。
            is_admin: user.is_admin,
        };
        drop(users);

        self.sessions.lock().insert(token, rec.clone());
        let _ = persist_sessions(&self.data, &self.sessions.lock());
        let _ = self.data.append_audit(
            "auth.log",
            &format!(
                "{} login user={} ok session={}",
                Utc::now().to_rfc3339(),
                rec.username,
                &rec.token[..8]
            ),
        );
        Ok(rec)
    }

    pub fn logout(&self, token: &str) -> CoreResult<()> {
        self.sessions.lock().remove(token);
        let _ = persist_sessions(&self.data, &self.sessions.lock());
        Ok(())
    }

    pub fn resolve_session(&self, token: &str) -> CoreResult<SessionRecord> {
        let mut sessions = self.sessions.lock();
        let Some(s) = sessions.get(token).cloned() else {
            return Err(CoreError::Auth("invalid or expired session".into()));
        };
        if s.expires_at <= Utc::now() {
            sessions.remove(token);
            let _ = persist_sessions(&self.data, &sessions);
            return Err(CoreError::Auth("invalid or expired session".into()));
        }
        Ok(s)
    }

    pub fn user_count(&self) -> usize {
        self.users.lock().len()
    }

    pub fn normalize_policy(p: &str) -> String {
        match p.trim().to_ascii_lowercase().as_str() {
            "reject" | "hard" | "fail" => "reject".into(),
            _ => "auto_evict".into(),
        }
    }

    fn session_cap_policy_normalized(&self) -> String {
        let live = self.session_cap_live.lock();
        Self::normalize_policy(&live.1)
    }

    pub fn effective_max_sessions(&self) -> usize {
        self.session_cap_live.lock().0.max(1)
    }

    pub fn max_sessions(&self) -> usize {
        self.effective_max_sessions()
    }

    pub fn session_ttl_hours(&self) -> i64 {
        self.session_ttl_hours
    }

    pub fn session_cap_policy(&self) -> String {
        self.session_cap_policy_normalized()
    }

    /// Hot-apply cap/policy (from settings PATCH). Persists only via settings-store separately.
    pub fn apply_session_cap_config(&self, cap: Option<usize>, policy: Option<&str>) {
        let mut live = self.session_cap_live.lock();
        if let Some(c) = cap {
            if c >= 1 {
                live.0 = c.min(10_000);
            }
        }
        if let Some(p) = policy {
            live.1 = Self::normalize_policy(p);
        }
    }

    /// Snapshot for GET /api/v1/sessions/stats.
    pub fn session_stats(&self) -> SessionCapStats {
        let now = Utc::now();
        let sessions = self.sessions.lock();
        let mut active = 0usize;
        let mut expired_present = 0usize;
        let mut oldest_created: Option<DateTime<Utc>> = None;
        let mut oldest_exp: Option<DateTime<Utc>> = None;
        for s in sessions.values() {
            if s.expires_at <= now {
                expired_present += 1;
                continue;
            }
            active += 1;
            let created = s.created_at.unwrap_or(s.expires_at);
            oldest_created = Some(match oldest_created {
                Some(o) if o < created => o,
                _ => created,
            });
            oldest_exp = Some(match oldest_exp {
                Some(o) if o < s.expires_at => o,
                _ => s.expires_at,
            });
        }
        let cap = self.effective_max_sessions();
        SessionCapStats {
            active,
            cap,
            free: cap.saturating_sub(active),
            policy: self.session_cap_policy_normalized(),
            ttl_hours: self.session_ttl_hours,
            oldest_created_at: oldest_created.map(|d| d.to_rfc3339()),
            oldest_expires_at: oldest_exp.map(|d| d.to_rfc3339()),
            expired_present,
        }
    }

    /// Drop expired auth sessions; returns how many removed.
    pub fn prune_expired_sessions(&self) -> CoreResult<usize> {
        let mut sessions = self.sessions.lock();
        let now = Utc::now();
        let before = sessions.len();
        sessions.retain(|_, s| s.expires_at > now);
        let n = before.saturating_sub(sessions.len());
        if n > 0 {
            persist_sessions(&self.data, &sessions)?;
            let _ = self.data.append_audit(
                "auth.log",
                &format!(
                    "{} session_prune_expired removed={} active={}",
                    Utc::now().to_rfc3339(),
                    n,
                    sessions.len()
                ),
            );
        }
        Ok(n)
    }

    /// Evict up to `count` oldest active sessions (by expires_at, then created_at).
    pub fn prune_oldest_sessions(&self, count: usize) -> CoreResult<usize> {
        if count == 0 {
            return Ok(0);
        }
        let mut sessions = self.sessions.lock();
        let now = Utc::now();
        sessions.retain(|_, s| s.expires_at > now);
        let mut victims: Vec<(String, DateTime<Utc>, DateTime<Utc>)> = sessions
            .iter()
            .map(|(k, s)| {
                (
                    k.clone(),
                    s.expires_at,
                    s.created_at.unwrap_or(s.expires_at),
                )
            })
            .collect();
        victims.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.2.cmp(&b.2)));
        let mut removed = 0usize;
        for (tok, _, _) in victims.into_iter().take(count) {
            sessions.remove(&tok);
            removed += 1;
            let _ = self.data.append_audit(
                "auth.log",
                &format!(
                    "{} session_prune_oldest token={} reason=api",
                    Utc::now().to_rfc3339(),
                    &tok[..8.min(tok.len())]
                ),
            );
        }
        if removed > 0 {
            persist_sessions(&self.data, &sessions)?;
        }
        Ok(removed)
    }

    /// List active session summaries (no full tokens — prefix only).
    pub fn list_session_summaries(&self, limit: usize) -> Vec<serde_json::Value> {
        let now = Utc::now();
        let sessions = self.sessions.lock();
        let mut rows: Vec<_> = sessions
            .values()
            .filter(|s| s.expires_at > now)
            .cloned()
            .collect();
        rows.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        rows.into_iter()
            .take(limit.clamp(1, 500))
            .map(|s| {
                json!({
                    "tokenPrefix": &s.token[..8.min(s.token.len())],
                    "userId": s.user_id,
                    "username": s.username,
                    "createdAt": s.created_at.map(|d| d.to_rfc3339()),
                    "expiresAt": s.expires_at.to_rfc3339(),
                })
            })
            .collect()
    }
}
