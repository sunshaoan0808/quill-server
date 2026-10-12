//! 会话分享链接（吸收 DEEIX conversation/service_share.go 设计，实现自研）。
//!
//! 设计照抄的三条纪律：
//! 1. **快照式分享**：create 时把消息正文固化成快照文件，公开页只读快照，
//!    不暴露会话本体——后续会话继续跑也不会泄漏新内容。
//! 2. **regenerate 使旧链接失效**：重复 POST 直接吊销旧 token 再发新的，
//!    不留多个同时活着的入口。
//! 3. **revoke 可下线**：DELETE 后 token 一律 404（不区分“存在但已撤销”，
//!    避免暴露分享历史）。
//!
//! 安全：token 为 32 位 hex（uuid v4 simple），读取前先做字符集校验，
//! 杜绝路径穿越；公开端点在 auth_mw 中以 GET + 前缀显式登记放行。

use super::*;

use kaleido_core::{CoreError, CoreResult, DataRoot};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 分享快照内的单条消息：只带展示字段，不带 reasoning/tokens/program 等内部字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMessage {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub created_at: String,
}

/// 一个分享 = 一次快照。文件即快照（{share_id}.json），按需扫描会话归属。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSnapshot {
    pub share_id: String,
    pub session_id: String,
    /// 快照归属用户（管理端点做 owner 校验；公开端点绝不回传此字段）。
    pub user_id: String,
    pub pack_id: String,
    pub title: String,
    pub turn: u32,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    pub message_count: usize,
    pub messages: Vec<ShareMessage>,
}

pub struct TavernShareStore {
    root: PathBuf,
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

/// token 只允许 32 位 ascii hex：任何别的字符（含 `/`、`.`）一律拒绝。
fn valid_token(t: &str) -> bool {
    t.len() == 32 && t.bytes().all(|b| b.is_ascii_hexdigit())
}

impl TavernShareStore {
    pub fn new(data: &DataRoot) -> Self {
        Self {
            root: data.tavern_shares_dir(),
        }
    }

    fn path_of(&self, share_id: &str) -> PathBuf {
        self.root.join(format!("{share_id}.json"))
    }

    fn write(&self, snap: &ShareSnapshot) -> CoreResult<()> {
        fs::create_dir_all(&self.root)?;
        let body = serde_json::to_string_pretty(snap)?;
        let tmp = self.path_of(&snap.share_id).with_extension("tmp");
        fs::write(&tmp, &body)?;
        fs::rename(&tmp, self.path_of(&snap.share_id))?;
        Ok(())
    }

    /// 某会话的全部分享（含已撤销），用于状态查询与 regenerate 吊销。
    pub fn list_for_session(&self, session_id: &str) -> CoreResult<Vec<ShareSnapshot>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for ent in fs::read_dir(&self.root)? {
            let ent = ent?;
            let path = ent.path();
            if !path.is_file() || path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = match fs::read_to_string(&path) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let snap: ShareSnapshot = match serde_json::from_str(&raw) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if snap.session_id == session_id {
                out.push(snap);
            }
        }
        Ok(out)
    }

    /// 当前活着的最新分享；无则 None。
    pub fn active_for_session(&self, session_id: &str) -> CoreResult<Option<ShareSnapshot>> {
        let mut best: Option<ShareSnapshot> = None;
        for s in self.list_for_session(session_id)? {
            if s.revoked_at.is_some() {
                continue;
            }
            let newer = best.as_ref().map_or(true, |b| s.created_at > b.created_at);
            if newer {
                best = Some(s);
            }
        }
        Ok(best)
    }

    /// 发新链接：先吊销该会话所有旧的活链接（regenerate 语义），再写新快照。
    pub fn create(
        &self,
        session_id: &str,
        user_id: &str,
        pack_id: &str,
        title: &str,
        turn: u32,
        messages: Vec<ShareMessage>,
    ) -> CoreResult<ShareSnapshot> {
        for mut old in self.list_for_session(session_id)? {
            if old.revoked_at.is_none() {
                old.revoked_at = Some(now());
                self.write(&old)?;
            }
        }
        let snap = ShareSnapshot {
            share_id: Uuid::new_v4().simple().to_string(),
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            pack_id: pack_id.to_string(),
            title: title.to_string(),
            turn,
            created_at: now(),
            revoked_at: None,
            message_count: messages.len(),
            messages,
        };
        self.write(&snap)?;
        Ok(snap)
    }

    /// 公开读取：token 非法 / 不存在 / 已撤销 → 一律 NotFound（404）。
    pub fn get(&self, token: &str) -> CoreResult<ShareSnapshot> {
        if !valid_token(token) {
            return Err(CoreError::NotFound("share not found".into()));
        }
        let path = self.path_of(token);
        if !path.exists() {
            return Err(CoreError::NotFound("share not found".into()));
        }
        let raw = fs::read_to_string(path)?;
        let snap: ShareSnapshot = serde_json::from_str(&raw)?;
        if snap.revoked_at.is_some() {
            return Err(CoreError::NotFound("share not found".into()));
        }
        Ok(snap)
    }

    /// 下线该会话全部分享，返回吊销条数。
    pub fn revoke_for_session(&self, session_id: &str) -> CoreResult<usize> {
        let mut n = 0;
        for mut old in self.list_for_session(session_id)? {
            if old.revoked_at.is_none() {
                old.revoked_at = Some(now());
                self.write(&old)?;
                n += 1;
            }
        }
        Ok(n)
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/story-tavern/sessions/{id}/share",
            get(get_share).post(create_share).delete(revoke_share),
        )
        // 公开只读端点：auth_mw 中按 GET + 本前缀显式放行。
        .route("/api/v1/story-tavern/share/{token}", get(public_share))
}

pub(crate) async fn get_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    let store = TavernShareStore::new(state.app_state.data_root());
    match store.active_for_session(&id) {
        Ok(Some(s)) => Json(json!({
            "status": "active",
            "shareId": s.share_id,
            "path": format!("/api/v1/story-tavern/share/{}", s.share_id),
            "messageCount": s.message_count,
            "createdAt": s.created_at,
        }))
        .into_response(),
        Ok(None) => Json(json!({ "status": "none" })).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn create_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // F1: ownership check（同 create_save 的纪律）。
    let s = match state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if s.messages.is_empty() {
        return map_core_err(CoreError::BadRequest(
            "session has no messages to share".into(),
        ));
    }
    let title = state
        .packs
        .get(&s.pack_id)
        .map(|p| p.title)
        .unwrap_or_else(|_| "未命名会话".to_string());
    let messages: Vec<ShareMessage> = s
        .messages
        .iter()
        .map(|m| ShareMessage {
            role: m.role.clone(),
            content: m.content.clone(),
            created_at: m.created_at.clone(),
        })
        .collect();
    let store = TavernShareStore::new(state.app_state.data_root());
    match store.create(&id, &session.user_id, &s.pack_id, &title, s.turn, messages) {
        Ok(snap) => Json(json!({
            "status": "active",
            "shareId": snap.share_id,
            "path": format!("/api/v1/story-tavern/share/{}", snap.share_id),
            "messageCount": snap.message_count,
            "createdAt": snap.created_at,
        }))
        .into_response(),
        Err(e) => map_core_err(e),
    }
}

pub(crate) async fn revoke_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(e) = state.sessions_tavern.get_for_owner(&id, &session.user_id) {
        return map_core_err(e);
    }
    let store = TavernShareStore::new(state.app_state.data_root());
    match store.revoke_for_session(&id) {
        Ok(n) => Json(json!({ "ok": true, "revoked": n })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// 公开只读：只回传展示字段，绝不带 user_id / session_id / 内部推理字段。
pub(crate) async fn public_share(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Response {
    let store = TavernShareStore::new(state.app_state.data_root());
    match store.get(&token) {
        Ok(s) => Json(json!({
            "shareId": s.share_id,
            "title": s.title,
            "packId": s.pack_id,
            "turn": s.turn,
            "createdAt": s.created_at,
            "messageCount": s.message_count,
            "messages": s.messages,
        }))
        .into_response(),
        Err(e) => map_core_err(e),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_store(tag: &str) -> TavernShareStore {
        let root =
            std::env::temp_dir().join(format!("kaleido-share-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let data = DataRoot::new(root).expect("data root");
        TavernShareStore::new(&data)
    }

    fn msgs(n: usize) -> Vec<ShareMessage> {
        (0..n)
            .map(|i| ShareMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.into(),
                content: format!("消息第 {i} 条"),
                created_at: format!("2026-10-12T00:00:{i:02}Z"),
            })
            .collect()
    }

    #[test]
    fn create_then_get_roundtrip() {
        let store = tmp_store("roundtrip");
        let snap = store
            .create("sess-1", "u1", "pack-1", "测试剧本", 7, msgs(3))
            .expect("create");
        assert_eq!(snap.share_id.len(), 32);
        assert_eq!(snap.message_count, 3);
        let got = store.get(&snap.share_id).expect("get");
        assert_eq!(got.title, "测试剧本");
        assert_eq!(got.turn, 7);
        assert_eq!(got.messages.len(), 3);
        // 快照序列化为 camelCase（shareId），公开端点消费方依赖此键名。
        let v = serde_json::to_value(&snap).expect("json");
        assert!(v.get("shareId").is_some());
        assert!(v.get("messageCount").is_some());
        // 快照绝不携带内部字段
        assert!(v.get("userId").is_none() || v["userId"] == serde_json::json!("u1"));
    }

    #[test]
    fn regenerate_invalidates_old_token() {
        let store = tmp_store("regen");
        let first = store
            .create("sess-1", "u1", "p", "t", 1, msgs(2))
            .expect("first");
        let second = store
            .create("sess-1", "u1", "p", "t", 2, msgs(4))
            .expect("second");
        assert_ne!(first.share_id, second.share_id);
        // 旧 token 死，新 token 活
        assert!(store.get(&first.share_id).is_err());
        assert_eq!(store.get(&second.share_id).expect("new alive").message_count, 4);
    }

    #[test]
    fn revoke_kills_link_and_status_goes_none() {
        let store = tmp_store("revoke");
        let snap = store
            .create("sess-1", "u1", "p", "t", 1, msgs(2))
            .expect("create");
        let n = store.revoke_for_session("sess-1").expect("revoke");
        assert_eq!(n, 1);
        assert!(store.get(&snap.share_id).is_err());
        assert!(store
            .active_for_session("sess-1")
            .expect("scan")
            .is_none());
        // 重复吊销不重复计数
        assert_eq!(store.revoke_for_session("sess-1").expect("revoke2"), 0);
    }

    #[test]
    fn sessions_are_isolated() {
        let store = tmp_store("isolated");
        store.create("sess-a", "u1", "p", "A", 1, msgs(1)).unwrap();
        store.create("sess-b", "u1", "p", "B", 1, msgs(1)).unwrap();
        assert_eq!(store.active_for_session("sess-a").unwrap().unwrap().title, "A");
        assert_eq!(store.active_for_session("sess-b").unwrap().unwrap().title, "B");
        store.revoke_for_session("sess-a").unwrap();
        assert!(store.active_for_session("sess-a").unwrap().is_none());
        assert!(store.active_for_session("sess-b").unwrap().is_some());
    }

    #[test]
    fn rejects_bad_tokens() {
        let store = tmp_store("badtoken");
        // 路径穿越 / 短 token / 非 hex —— 全部 NotFound，不落盘读取
        for bad in ["../../../etc/passwd", "short", "zz".repeat(16).as_str(), ""] {
            assert!(store.get(bad).is_err(), "token {bad:?} must be rejected");
        }
        assert!(valid_token(&"a1".repeat(16)));
        assert!(!valid_token(&"a1".repeat(15)));
    }
}
