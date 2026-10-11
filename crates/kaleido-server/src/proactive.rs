//! [主动关怀 P2 2026-10-11, 吞噬自 AIcove 主动关怀系统]
//! 调度器 + 手动 API + 发火落库。core 模型/判定 = `kaleido_core::proactive_trigger`。
//!
//! v1 范围（对齐 docs/plans/2026-10-11-proactive-care.md P2）：
//! - 触发源：手动 API 创建（high 优先级，用户显式提醒语义；allowNight 可选）
//! - 调度器：60s 扫描 `$KALEIDO_DATA/proactive/` 的 due 触发器 → 作废判定 → 发火
//! - 发火：`cachedContent` 优先；否则 LLM 角色化生成（`call_llm_nonstream`）；
//!   LLM 失败 → fireAt 顺延 5 分钟有界重试，不无限打上游
//! - 送达：消息落库为正常 assistant 消息（前端下次拉会话自然可见）；
//!   主动推送后置（StreamHub 是 run_id 键，无常驻会话通道；Web Push 在移动端阶段）
//! - 作废：high 永不 / medium 锚点动 / low 锚点动或在线（v1 无在线探测，恒 false）
//! - 审计：发火/作废/取消写 `$KALEIDO_DATA/audit/proactive.log`
//!
//! 已知 v1 缺口：medium/low 的管家自动分析（P3，需 LLM 分析器）、SSE 推送、冷却期。

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use kaleido_core::proactive_trigger::{
    effective_fire_at, should_invalidate, ProactivePriority, ProactiveStore, ProactiveTrigger,
    TriggerState,
};
use serde::Deserialize;
use serde_json::json;

use crate::auth_mw::session_from;
use crate::error_codes::{bad_request, not_found};
use crate::error_map::map_core_err;
use crate::state::AppState;

const SCHEDULER_INTERVAL_SECS: u64 = 60;
/// LLM 生成失败后的重试间隔（fire_at 顺延量）。
const LLM_RETRY_DELAY_SECS: i64 = 300;
/// prompt 缺省时的 Proactive Agent 契约。
const DEFAULT_PROACTIVE_PROMPT: &str = "你是故事中的角色。现在是合适的时机，请主动向玩家发一条自然的消息：延续当前剧情与关系状态，语气符合人设，1-3 句，不要复述剧情原文，不要输出选项或解释，直接给消息正文。";

// ---------------------------------------------------------------------------
// 路由
// ---------------------------------------------------------------------------

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/story-tavern/sessions/{id}/proactive",
            post(create_trigger).get(list_triggers),
        )
        .route(
            "/api/v1/story-tavern/sessions/{id}/proactive/{trigger_id}",
            delete(cancel_trigger),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateTriggerBody {
    /// 主动消息内容指令（如「明早8点提醒我修炼」）；发火时作为 Proactive Agent 的输入。
    #[serde(default)]
    prompt: String,
    /// 显式发火时刻（ISO8601）；与 delaySeconds 二选一，都缺省 = 1 小时后。
    #[serde(default)]
    fire_at: Option<DateTime<Utc>>,
    #[serde(default)]
    delay_seconds: Option<u64>,
    /// 夜间(23:00-07:00 本地时区)也允许发火；缺省 false = 静默顺延到 07:00。
    #[serde(default)]
    allow_night: Option<bool>,
}

/// `POST /api/v1/story-tavern/sessions/{id}/proactive`
/// ——用户显式提醒（对齐 AIcove create_reminder，high 优先级，永不作废）。
async fn create_trigger(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    body: Json<CreateTriggerBody>,
) -> Response {
    let sess = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // 所有权校验 + 会话存在性（同时拿锚点）
    let tavern = match state.sessions_tavern.get_for_owner(&session_id, &sess.user_id) {
        Ok(t) => t,
        Err(e) => return map_core_err(e),
    };
    if tavern.pack_missing {
        return bad_request("PROACTIVE_PACK_MISSING", "session is read-only (pack missing)");
    }
    if body.prompt.trim().is_empty() {
        return bad_request("PROACTIVE_EMPTY_PROMPT", "prompt is required");
    }
    let fire_at = match (body.fire_at, body.delay_seconds) {
        (Some(f), _) => f,
        (None, Some(d)) => Utc::now() + chrono::Duration::seconds(d as i64),
        (None, None) => Utc::now() + chrono::Duration::hours(1),
    };
    let anchor = last_user_message_id(&tavern.messages);
    let t = ProactiveTrigger {
        id: format!("pt-{}", uuid::Uuid::new_v4()),
        session_id: session_id.clone(),
        state: TriggerState::Created,
        priority: ProactivePriority::High,
        fire_at,
        allow_night: body.allow_night.unwrap_or(false),
        prompt: body.prompt.trim().to_string(),
        cached_content: None,
        anchor_last_user_message_id: anchor,
        anchor_last_user_message_at: None,
        created_at: Utc::now(),
        fired_at: None,
    };
    let store = ProactiveStore::new(state.app_state.data_root().clone());
    if let Err(e) = store.upsert(t.clone()) {
        return map_core_err(e);
    }
    let _ = state.app_state.data_root().append_audit(
        "proactive.log",
        &format!(
            "{} create tid={} sid={} fire_at={}",
            Utc::now().to_rfc3339(),
            t.id,
            session_id,
            t.fire_at.to_rfc3339()
        ),
    );
    Json(json!({ "ok": true, "trigger": t })).into_response()
}

/// `GET /api/v1/story-tavern/sessions/{id}/proactive`
async fn list_triggers(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let sess = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(e) = state
        .sessions_tavern
        .get_for_owner(&session_id, &sess.user_id)
    {
        return map_core_err(e);
    }
    let store = ProactiveStore::new(state.app_state.data_root().clone());
    match store.list_for_session(&session_id) {
        Ok(list) => Json(json!({ "ok": true, "triggers": list })).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// `DELETE /api/v1/story-tavern/sessions/{id}/proactive/{trigger_id}` —— 显式取消。
async fn cancel_trigger(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, trigger_id)): Path<(String, String)>,
) -> Response {
    let sess = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    if let Err(e) = state
        .sessions_tavern
        .get_for_owner(&session_id, &sess.user_id)
    {
        return map_core_err(e);
    }
    let store = ProactiveStore::new(state.app_state.data_root().clone());
    let mut list = match store.list_for_session(&session_id) {
        Ok(l) => l,
        Err(e) => return map_core_err(e),
    };
    // 索引定位而非 iter_mut 句柄：避免可变借用横跨 save_for_session 的不可变借用(E0502)
    let Some(pos) = list.iter().position(|t| t.id == trigger_id) else {
        return not_found("PROACTIVE_TRIGGER_NOT_FOUND", &trigger_id);
    };
    if !list[pos].mark_cancelled() {
        return bad_request(
            "PROACTIVE_NOT_PENDING",
            "trigger is not pending (already fired/expired/cancelled)",
        );
    }
    if let Err(e) = store.save_for_session(&session_id, &list) {
        return map_core_err(e);
    }
    let _ = state.app_state.data_root().append_audit(
        "proactive.log",
        &format!("{} cancel tid={} sid={}", Utc::now().to_rfc3339(), trigger_id, session_id),
    );
    Json(json!({ "ok": true, "trigger": &list[pos] })).into_response()
}

// ---------------------------------------------------------------------------
// 调度器
// ---------------------------------------------------------------------------

/// 启动后台调度器（main 启动期调用一次；45s 冷启动延迟，60s 扫描间隔）。
pub fn spawn_scheduler(state: AppState) {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(45)).await;
        loop {
            if let Err(e) = tick(&state).await {
                tracing::warn!(error = %e, "proactive scheduler tick failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(SCHEDULER_INTERVAL_SECS)).await;
        }
    });
}

async fn tick(state: &AppState) -> Result<(), kaleido_core::CoreError> {
    let store = ProactiveStore::new(state.app_state.data_root().clone());
    let due = store.due_triggers(Utc::now())?;
    for t in due {
        handle_due_trigger(state, &store, t).await;
    }
    Ok(())
}

/// 发火判定（纯函数，单测覆盖）：作废优先于发火。
fn decide(
    t: &ProactiveTrigger,
    last_user_message_id: &str,
    user_connected: bool,
    now: DateTime<Utc>,
) -> FireDecision {
    if should_invalidate(t, last_user_message_id, user_connected) {
        return FireDecision::Invalidate;
    }
    if effective_fire_at(t.fire_at, t.allow_night) <= now {
        FireDecision::Fire
    } else {
        FireDecision::NotYet
    }
}

enum FireDecision {
    Fire,
    Invalidate,
    NotYet,
}

fn last_user_message_id(messages: &[kaleido_core::TavernMessage]) -> String {
    messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.id.clone())
        .unwrap_or_default()
}

async fn handle_due_trigger(state: &AppState, store: &ProactiveStore, t: ProactiveTrigger) {
    let sid = t.session_id.clone();
    let sess = match state.sessions_tavern.get(&sid) {
        Ok(s) => s,
        Err(_) => {
            // 会话已被删除：触发器作废收尾
            let mut list = store.list_for_session(&sid).unwrap_or_default();
            for x in list.iter_mut() {
                if x.id == t.id {
                    x.mark_expired();
                }
            }
            let _ = store.save_for_session(&sid, &list);
            return;
        }
    };
    let last_user_id = last_user_message_id(&sess.messages);
    // v1 无按会话在线探测（StreamHub 是 run_id 键）：user_connected 恒 false，
    // low 触发器仅靠锚点作废（玩家来过新消息）。
    match decide(&t, &last_user_id, false, Utc::now()) {
        FireDecision::Invalidate => {
            let mut list = store.list_for_session(&sid).unwrap_or_default();
            for x in list.iter_mut() {
                if x.id == t.id {
                    x.mark_expired();
                }
            }
            let _ = store.save_for_session(&sid, &list);
            let _ = state.app_state.data_root().append_audit(
                "proactive.log",
                &format!("{} expire tid={} sid={} reason=anchor_moved", Utc::now().to_rfc3339(), t.id, sid),
            );
        }
        FireDecision::NotYet => {
            // due_triggers 已按 effective_fire_at 过滤，理论不可达；防御性忽略
        }
        FireDecision::Fire => fire(state, store, t, &sess).await,
    }
}

async fn fire(state: &AppState, store: &ProactiveStore, t: ProactiveTrigger, sess: &kaleido_core::TavernSession) {
    let sid = t.session_id.clone();
    let content = match t.cached_content.clone().filter(|s| !s.trim().is_empty()) {
        Some(c) => c,
        None => match generate_proactive_message(state, &t, sess).await {
            Ok(c) => c,
            Err(e) => {
                // 有界重试：fire_at 顺延 5 分钟，不无限打上游
                let mut t2 = t.clone();
                t2.fire_at = Utc::now() + chrono::Duration::seconds(LLM_RETRY_DELAY_SECS);
                let _ = store.upsert(t2);
                tracing::warn!(error = %e, sid = %sid, tid = %t.id, "proactive: LLM 生成失败，fire_at 顺延重试");
                return;
            }
        },
    };
    // 与回合 P2 同口径：剥协议块、块落消息（模型可能仍输出 <UpdateVariable>）
    let mvu_blocks = kaleido_core::st_mvu::extract_update_blocks(&content);
    let story_body = kaleido_core::st_mvu::strip_update_blocks(&content);
    let msg = kaleido_core::TavernMessage {
        id: format!("msg-{}", uuid::Uuid::new_v4()),
        role: "assistant".into(),
        content: story_body,
        created_at: Utc::now().to_rfc3339(),
        options: vec![],
        engine_tag: None,
        program: None,
        reasoning: None,
        swipes: vec![],
        swipe_index: 0,
        tokens: 0,
        mvu_blocks,
    };
    let mut fired = t.clone();
    let mut persisted = false;
    // CAS 原子落库（update_session 锁内读改写，ADR-0004；不覆盖并发回合写入）
    if let Err(e) = state.sessions_tavern.update_session(&sid, |live| {
        live.messages.push(msg.clone());
        Ok(())
    }) {
        tracing::warn!(error = %e, sid = %sid, "proactive: 落库失败");
    } else {
        persisted = true;
    }
    if persisted {
        fired.mark_fired(Utc::now());
        let _ = store.upsert(fired);
        let _ = state.app_state.data_root().append_audit(
            "proactive.log",
            &format!("{} fire tid={} sid={}", Utc::now().to_rfc3339(), t.id, sid),
        );
        tracing::info!(sid = %sid, tid = %t.id, "proactive: 主动消息已落库");
    }
}

async fn generate_proactive_message(
    state: &AppState,
    t: &ProactiveTrigger,
    sess: &kaleido_core::TavernSession,
) -> Result<String, String> {
    let system = if t.prompt.trim().is_empty() {
        DEFAULT_PROACTIVE_PROMPT
    } else {
        t.prompt.as_str()
    };
    let recent: Vec<String> = sess
        .messages
        .iter()
        .rev()
        .take(6)
        .rev()
        .map(|m| {
            format!(
                "{}: {}",
                m.role,
                m.content.chars().take(300).collect::<String>()
            )
        })
        .collect();
    let user = format!(
        "最近对话记录：\n{}\n\n现在到了合适的时机，请以角色身份主动向玩家发一条消息（直接给消息正文）。",
        recent.join("\n")
    );
    crate::story_tavern::call_llm_nonstream(state, system, &user).await
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use kaleido_core::TavernMessage;

    fn trig(priority: ProactivePriority, anchor: &str) -> ProactiveTrigger {
        ProactiveTrigger {
            id: "pt-test".into(),
            session_id: "tavern-session-t".into(),
            state: TriggerState::Created,
            priority,
            fire_at: Utc::now() - chrono::Duration::minutes(1), // 已到期
            allow_night: true,                                  // 排除夜间顺延干扰
            prompt: "p".into(),
            cached_content: None,
            anchor_last_user_message_id: anchor.into(),
            anchor_last_user_message_at: None,
            created_at: Utc::now(),
            fired_at: None,
        }
    }

    fn m(id: &str, role: &str) -> TavernMessage {
        TavernMessage {
            id: id.into(),
            role: role.into(),
            content: "c".into(),
            created_at: String::new(),
            options: vec![],
            engine_tag: None,
            program: None,
            reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
            mvu_blocks: Vec::new(),
        }
    }

    #[test]
    fn last_user_message_id_takes_most_recent_user_msg() {
        let msgs = vec![m("u1", "user"), m("a1", "assistant"), m("u2", "user")];
        assert_eq!(last_user_message_id(&msgs), "u2");
        assert_eq!(last_user_message_id(&[]), "");
        let only_ai = vec![m("a1", "assistant")];
        assert_eq!(last_user_message_id(&only_ai), "");
    }

    #[test]
    fn decide_prefers_invalidate_over_fire() {
        let mut high = trig(ProactivePriority::High, "m1");
        let mut low = trig(ProactivePriority::Low, "m1");
        high.state = TriggerState::Prepared;
        let now = Utc::now();
        // 到期 + 锚点未动 → Fire
        assert!(matches!(decide(&high, "m1", false, now), FireDecision::Fire));
        // low：锚点动了 → 作废优先（即使已到期）
        assert!(matches!(decide(&low, "m2", false, now), FireDecision::Invalidate));
        // low：用户在线 → 作废（v1 恒 false，此处验证语义接线正确）
        assert!(matches!(decide(&low, "m1", true, now), FireDecision::Invalidate));
        // high：永不作废
        assert!(matches!(decide(&high, "m2", true, now), FireDecision::Fire));
        // 未到期 → NotYet（夜间静默：allow_night=false 落在夜里）
        let mut quiet = trig(ProactivePriority::High, "m1");
        quiet.allow_night = false;
        quiet.fire_at = Utc::now();
        // effective_fire_at 可能顺延到 07:00 → NotYet 或刚好白天 → Fire，二者皆合法；
        // 这里只验证不 Invalidate
        assert!(!matches!(decide(&quiet, "m1", false, now), FireDecision::Invalidate));
    }

    #[test]
    fn create_body_parses_camel_case_and_flags_empty_prompt() {
        // handler 层校验（prompt 必填）是纯字符串判断，锁语义防漂移
        let body: CreateTriggerBody =
            serde_json::from_str(r#"{"prompt": "  "}"#).unwrap();
        assert!(body.prompt.trim().is_empty());
        let body2: CreateTriggerBody =
            serde_json::from_str(r#"{"prompt": "叫我", "delaySeconds": 60, "allowNight": true}"#)
                .unwrap();
        assert_eq!(body2.prompt, "叫我");
        assert_eq!(body2.delay_seconds, Some(60));
        assert_eq!(body2.allow_night, Some(true));
    }
}
