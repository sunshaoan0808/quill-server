//! [主动关怀 P1 2026-10-11, 吞噬自 AIcove 主动关怀系统]
//! 触发器模型 + JSON 文件存储 + 判定纯函数。
//! 调度器(扫描发火)、SSE 送达、预生成 job 在 P2 落 kaleido-server(见
//! docs/plans/2026-10-11-proactive-care.md);本模块保持纯函数 + 零 tokio 依赖。
//!
//! 存储:`$KALEIDO_DATA/proactive/{session_id}.json`(兼容回退 MUSEAI_DATA)
//! = `Vec<ProactiveTrigger>`。
//! 锚点语义(ADR-0004):作废判定只依赖落盘消息的最后一条**用户**消息 id,
//! 不依赖内存态;用户在线与否由 server 层传 `user_connected`(SSE 订阅活性)。

use crate::{CoreError, CoreResult, DataRoot};
use chrono::{DateTime, Local, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

pub const PROACTIVE_DIR: &str = "proactive";

// ---------------------------------------------------------------------------
// 模型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProactivePriority {
    /// 用户显式要求;永不作废。
    High,
    /// 管家自动分析;用户来过新消息则作废。
    Medium,
    /// 低打扰;用户来过新消息或正在会话页则作废。
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TriggerState {
    /// 已创建(可投预生成 job)。
    Created,
    /// 预生成内容就绪。
    Prepared,
    /// 已发火落库。
    Fired,
    /// 被作废规则或超时拦截。
    Expired,
    /// 用户/系统显式取消。
    Cancelled,
}

impl TriggerState {
    pub fn is_pending(self) -> bool {
        matches!(self, TriggerState::Created | TriggerState::Prepared)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProactiveTrigger {
    pub id: String,
    pub session_id: String,
    pub state: TriggerState,
    pub priority: ProactivePriority,
    /// 计划发火时间(UTC)。夜间静默由 [`effective_fire_at`] 在发火判定时顺延,
    /// 不改写本字段(保留用户原始意图)。
    pub fire_at: DateTime<Utc>,
    pub allow_night: bool,
    /// 主动消息的 system prompt(Proactive Agent 契约)。
    pub prompt: String,
    /// 预生成内容;Prepared 后有值,发火优先使用。
    pub cached_content: Option<String>,
    /// 作废比对锚点:创建时的最后一条用户消息 id(落盘消息,真相源)。
    pub anchor_last_user_message_id: String,
    pub anchor_last_user_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub fired_at: Option<DateTime<Utc>>,
}

impl Default for ProactiveTrigger {
    fn default() -> Self {
        Self {
            id: String::new(),
            session_id: String::new(),
            state: TriggerState::Created,
            priority: ProactivePriority::Low,
            fire_at: Utc::now(),
            allow_night: false,
            prompt: String::new(),
            cached_content: None,
            anchor_last_user_message_id: String::new(),
            anchor_last_user_message_at: None,
            created_at: Utc::now(),
            fired_at: None,
        }
    }
}

impl ProactiveTrigger {
    /// 状态机:进入 Prepared(仅 Created 可转)。
    pub fn mark_prepared(&mut self, content: impl Into<String>) -> bool {
        if self.state == TriggerState::Created {
            self.state = TriggerState::Prepared;
            self.cached_content = Some(content.into());
            true
        } else {
            false
        }
    }

    /// 状态机:发火(仅 pending 可转)。
    pub fn mark_fired(&mut self, now: DateTime<Utc>) -> bool {
        if self.state.is_pending() {
            self.state = TriggerState::Fired;
            self.fired_at = Some(now);
            true
        } else {
            false
        }
    }

    /// 状态机:作废/过期(仅 pending 可转)。
    pub fn mark_expired(&mut self) -> bool {
        if self.state.is_pending() {
            self.state = TriggerState::Expired;
            true
        } else {
            false
        }
    }

    /// 状态机:显式取消(仅 pending 可转)。
    pub fn mark_cancelled(&mut self) -> bool {
        if self.state.is_pending() {
            self.state = TriggerState::Cancelled;
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// 判定纯函数
// ---------------------------------------------------------------------------

/// 作废判定(对齐 AIcove 优先级语义):
/// - high:永不作废;
/// - medium:锚点变化(用户来过新消息)→ 作废;
/// - low:锚点变化 **或** 用户在线 → 作废。
/// 仅对 pending 触发器有意义;非 pending 一律 false。
pub fn should_invalidate(
    t: &ProactiveTrigger,
    last_user_message_id: &str,
    user_connected: bool,
) -> bool {
    if !t.state.is_pending() {
        return false;
    }
    let anchor_moved = !last_user_message_id.is_empty()
        && last_user_message_id != t.anchor_last_user_message_id;
    match t.priority {
        ProactivePriority::High => false,
        ProactivePriority::Medium => anchor_moved,
        ProactivePriority::Low => anchor_moved || user_connected,
    }
}

/// 夜间静默(23:00–07:00,服务器本地时区):`allow_night=false` 的触发器
/// 落在夜间窗内时顺延到当天/次日 07:00。不改写触发器自身字段。
pub fn effective_fire_at(fire_at: DateTime<Utc>, allow_night: bool) -> DateTime<Utc> {
    if allow_night {
        return fire_at;
    }
    let local = fire_at.with_timezone(&Local);
    let hour = local.hour();
    if (7..23).contains(&hour) {
        return fire_at;
    }
    // 顺延到 07:00(23 点及以后 → 次日 07:00;0-6 点 → 当天 07:00)
    let date = local.date_naive();
    let naive = if hour >= 23 {
        date.succ_opt()
            .map(|d| d.and_hms_opt(7, 0, 0))
            .unwrap_or(None)
    } else {
        date.and_hms_opt(7, 0, 0)
    };
    match naive {
        Some(n) => Local
            .from_local_datetime(&n)
            .single()
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or(fire_at),
        None => fire_at,
    }
}

// ---------------------------------------------------------------------------
// 存储
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct ProactiveStore {
    data: DataRoot,
}

impl ProactiveStore {
    pub fn new(data: DataRoot) -> Self {
        Self { data }
    }

    fn dir(&self) -> PathBuf {
        self.data.root().join(PROACTIVE_DIR)
    }

    fn path_for(&self, session_id: &str) -> CoreResult<PathBuf> {
        // 与 AgentSessionStore::validate_id 同款的严苛字符集,防目录逃逸
        let ok = !session_id.is_empty()
            && session_id.len() <= 128
            && session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && !session_id.contains("..");
        if !ok {
            return Err(CoreError::BadRequest(
                "invalid session id (alnum/_/- only)".into(),
            ));
        }
        Ok(self.dir().join(format!("{session_id}.json")))
    }

    pub fn list_for_session(&self, session_id: &str) -> CoreResult<Vec<ProactiveTrigger>> {
        let path = self.path_for(session_id)?;
        if !path.exists() {
            return Ok(Vec::new());
        }
        let raw = fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save_for_session(
        &self,
        session_id: &str,
        triggers: &[ProactiveTrigger],
    ) -> CoreResult<()> {
        let path = self.path_for(session_id)?;
        fs::create_dir_all(self.dir())?;
        fs::write(&path, serde_json::to_string_pretty(triggers)?)?;
        Ok(())
    }

    /// 按 id 替换/追加(同 session 文件内)。
    pub fn upsert(&self, trigger: ProactiveTrigger) -> CoreResult<()> {
        let mut list = self.list_for_session(&trigger.session_id)?;
        if let Some(slot) = list.iter_mut().find(|t| t.id == trigger.id) {
            *slot = trigger;
        } else {
            list.push(trigger);
        }
        let sid = list
            .first()
            .map(|t| t.session_id.clone())
            .unwrap_or_default();
        self.save_for_session(&sid, &list)
    }

    /// 用户发消息时同步作废该会话全部 pending(返回作废数)。
    pub fn cancel_pending_for_session(&self, session_id: &str) -> CoreResult<usize> {
        let mut list = self.list_for_session(session_id)?;
        let mut n = 0usize;
        for t in list.iter_mut() {
            if t.mark_cancelled() {
                n += 1;
            }
        }
        if n > 0 {
            self.save_for_session(session_id, &list)?;
        }
        Ok(n)
    }

    /// 调度器扫描:全部会话文件里 pending 且 effective_fire_at<=now 的触发器。
    /// (预生成内容随触发器一并返回,发火优先用 cached_content。)
    pub fn due_triggers(&self, now: DateTime<Utc>) -> CoreResult<Vec<ProactiveTrigger>> {
        let dir = self.dir();
        let mut out = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => return Ok(out), // 目录未建 = 无触发器
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(list) = serde_json::from_str::<Vec<ProactiveTrigger>>(&raw) else {
                continue; // 坏文件跳过,不阻断扫描
            };
            for t in list {
                if t.state.is_pending() && effective_fire_at(t.fire_at, t.allow_night) <= now {
                    out.push(t);
                }
            }
        }
        out.sort_by(|a, b| a.fire_at.cmp(&b.fire_at));
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn trig(priority: ProactivePriority, anchor: &str) -> ProactiveTrigger {
        ProactiveTrigger {
            id: format!("pt-{}", Utc::now().timestamp_nanos_opt().unwrap_or(0)),
            session_id: "tavern-session-test".into(),
            state: TriggerState::Created,
            priority,
            fire_at: Utc::now(),
            allow_night: false,
            prompt: "主动关怀契约".into(),
            cached_content: None,
            anchor_last_user_message_id: anchor.into(),
            anchor_last_user_message_at: None,
            created_at: Utc::now(),
            fired_at: None,
        }
    }

    #[test]
    fn serde_roundtrip_camel_case() {
        let t = trig(ProactivePriority::Medium, "msg-1");
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"sessionId\""));
        assert!(json.contains("\"anchorLastUserMessageId\""));
        let back: ProactiveTrigger = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn invalidation_follows_priority_semantics() {
        let mut high = trig(ProactivePriority::High, "m1");
        let mut medium = trig(ProactivePriority::Medium, "m1");
        let mut low = trig(ProactivePriority::Low, "m1");
        high.state = TriggerState::Prepared;

        // 锚点未动:medium 不作废,low 因用户在线作废
        assert!(!should_invalidate(&medium, "m1", false));
        assert!(!should_invalidate(&low, "m1", false));
        assert!(should_invalidate(&low, "m1", true));
        // 锚点动了
        assert!(should_invalidate(&medium, "m2", false));
        assert!(should_invalidate(&low, "m2", false));
        // high 永不作废
        assert!(!should_invalidate(&high, "m2", true));
        // 非 pending 不参与判定
        medium.mark_fired(Utc::now());
        assert!(!should_invalidate(&medium, "m2", true));
    }

    #[test]
    fn night_quiet_shifts_to_7am_local() {
        // 构造本地时区的 23:30 与 03:00(与本机时区无关的确定性断言)
        let late = Local
            .with_ymd_and_hms(2026, 10, 11, 23, 30, 0)
            .unwrap()
            .with_timezone(&Utc);
        let early = Local
            .with_ymd_and_hms(2026, 10, 11, 3, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let noon = Local
            .with_ymd_and_hms(2026, 10, 11, 12, 0, 0)
            .unwrap()
            .with_timezone(&Utc);

        let shifted_late = effective_fire_at(late, false).with_timezone(&Local);
        assert_eq!(shifted_late.format("%m-%d %H:%M").to_string(), "10-12 07:00");

        let shifted_early = effective_fire_at(early, false).with_timezone(&Local);
        assert_eq!(shifted_early.format("%m-%d %H:%M").to_string(), "10-11 07:00");

        // 白天不动;allow_night 旁路
        assert_eq!(effective_fire_at(noon, false), noon);
        assert_eq!(effective_fire_at(late, true), late);
    }

    #[test]
    fn store_upsert_cancel_and_due_scan() {
        let tmp = tempfile_dir();
        let data = DataRoot::new(tmp.clone()).unwrap();
        let store = ProactiveStore::new(data);

        let mut t1 = trig(ProactivePriority::Medium, "m0");
        t1.session_id = "tavern-session-a".into();
        t1.fire_at = Utc::now() - chrono::Duration::minutes(5); // 已到期
        let mut t2 = trig(ProactivePriority::Low, "m0");
        t2.session_id = "tavern-session-a".into();
        t2.fire_at = Utc::now() + chrono::Duration::hours(1); // 未到期
        let mut t3 = trig(ProactivePriority::High, "m0");
        t3.session_id = "tavern-session-b".into();
        t3.fire_at = Utc::now() - chrono::Duration::minutes(1);

        store.upsert(t1.clone()).unwrap();
        store.upsert(t2.clone()).unwrap();
        store.upsert(t3.clone()).unwrap();

        // upsert 幂等(同 id 替换不重复)
        store.upsert(t1.clone()).unwrap();
        assert_eq!(store.list_for_session("tavern-session-a").unwrap().len(), 2);

        // due 扫描:会话 a 的 t1 到期,会话 b 的 t3 到期,t2 未到期
        let due = store.due_triggers(Utc::now()).unwrap();
        let ids: Vec<_> = due.iter().map(|t| t.id.clone()).collect();
        assert!(ids.contains(&t1.id) && ids.contains(&t3.id) && !ids.contains(&t2.id));

        // 预生成 → Prepared;取消 pending(t1 已 Prepared、t2 仍 Created,共 2 个)
        let mut t1b = t1.clone();
        assert!(t1b.mark_prepared("想你了"));
        store.upsert(t1b).unwrap();
        assert_eq!(store.cancel_pending_for_session("tavern-session-a").unwrap(), 2);
        let after = store.list_for_session("tavern-session-a").unwrap();
        let a1 = after.iter().find(|t| t.id == t1.id).unwrap();
        let a2 = after.iter().find(|t| t.id == t2.id).unwrap();
        assert_eq!(a1.state, TriggerState::Cancelled); // t1 已被取消(等价于取消 pending)
        assert_eq!(a2.state, TriggerState::Cancelled);

        // id 校验拒目录逃逸
        assert!(store.list_for_session("../evil").is_err());
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn state_machine_rejects_illegal_transitions() {
        let mut t = trig(ProactivePriority::Low, "m");
        assert!(t.mark_fired(Utc::now()));
        assert!(!t.mark_fired(Utc::now()));
        assert!(!t.mark_prepared("x"));
        assert!(!t.mark_expired());
        assert_eq!(t.state, TriggerState::Fired);
        assert!(t.fired_at.is_some());
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "museai-proactive-test-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
