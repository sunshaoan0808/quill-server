//! 悬而未决记忆的**持久化 + 异步整合**（记忆三要素之二、之三的落地面）。
//!
//! # 为什么要有这一层
//!
//! [`crate::pending_memory`] 已经能把摘要文本解析成 [`PendingItem`] 并跨轮归并，
//! 但那是纯函数——宿主此前每次请求都重新解析一遍原文，既不落盘也不跨轮，
//! 「异步整合 `integrating` 标志」更是一个恒为 `false` 的摆设字段。
//!
//! 对齐 dianziji 实测行为：摘要整合是**异步**的，UI 在整合期间读到
//! `integrating: true` 就能显示「整合中」并且**不重复提交**（否则并发提交会
//! 把同一件事写进两次）。所以这里把三件事做实：
//!
//! 1. **落盘**：按 workspace 分片存当前条目，重启不丢（原来只有请求内存）。
//! 2. **跨轮**：`begin_merge` 走 [`merge_pending`]，上一轮未兑现项自动延续。
//! 3. **异步标志**：`begin_merge` **同步**把 `integrating` 置 true 再返回，
//!    合并与写盘由调用方 `tokio::spawn` 出去做，完成后调 [`PendingStore::finish_merge`]。
//!    这样 HTTP 响应不必等磁盘，且读者能明确区分「整合中」与「整合完成」。
//!
//! # 并发语义
//!
//! `begin_merge` 在**同一 workspace 已有整合在飞**时返回
//! [`PendingError::Busy`]——这就是 `integrating` 标志真正的用途：挡住重复提交，
//! 而不是给前端一个摆设字段看。

use crate::db::{DbError, DbPool};
use crate::pending_memory::{merge_pending, PendingItem};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug)]
pub enum PendingError {
    Db(DbError),
    /// 该 workspace 已有整合在飞（`integrating = true`），拒绝重复提交。
    Busy,
    BadRequest(String),
}

impl std::fmt::Display for PendingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PendingError::Db(e) => write!(f, "pending db: {e}"),
            PendingError::Busy => write!(f, "该 workspace 正在整合中，请稍候"),
            PendingError::BadRequest(m) => write!(f, "bad request: {m}"),
        }
    }
}

impl std::error::Error for PendingError {}

impl From<DbError> for PendingError {
    fn from(e: DbError) -> Self {
        PendingError::Db(e)
    }
}

/// 一个 workspace 的悬而未决快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PendingSnapshot {
    /// 当前未兑现条目（按 seq 排序）。
    pub items: Vec<PendingItem>,
    /// 规范化后的摘要文本（`render_pending` 的结果），便于直接回喂 prompt。
    pub summary: String,
    /// 是否正在异步整合。
    pub integrating: bool,
    /// 整合完成时刻（RFC3339）；从未整合过为 `None`。
    pub updated_at: Option<String>,
}

/// 按 workspace 分片的悬而未决记忆仓库（SQLite）。
///
/// `Clone` 是共享池的浅拷贝（`DbPool` 内部 `Arc`），刻意如此：异步整合任务
/// 需要在 `tokio::spawn` 里继续用同一个仓库写盘。
#[derive(Clone)]
pub struct PendingStore {
    pool: DbPool,
}

impl PendingStore {
    pub fn open(path: &Path) -> Result<Self, PendingError> {
        let pool = DbPool::open(path, 4).map_err(PendingError::Db)?;
        Self::ensure_schema(&pool)?;
        Ok(PendingStore { pool })
    }

    pub fn open_in_memory() -> Result<Self, PendingError> {
        let pool = DbPool::open_in_memory(4).map_err(PendingError::Db)?;
        Self::ensure_schema(&pool)?;
        Ok(PendingStore { pool })
    }

    fn ensure_schema(pool: &DbPool) -> Result<(), PendingError> {
        let mut c = pool.get().map_err(PendingError::Db)?;
        c.conn()
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS pending_memory (
                     workspace_id TEXT PRIMARY KEY,
                     items       TEXT NOT NULL DEFAULT '[]',
                     summary     TEXT NOT NULL DEFAULT '',
                     integrating INTEGER NOT NULL DEFAULT 0,
                     updated_at  TEXT
                 );",
            )
            .map_err(|e| PendingError::Db(DbError::Migrate(e)))?;
        Ok(())
    }

    /// 读当前快照（无记录时返回空快照，`integrating = false`）。
    pub fn get(&self, ws: &str) -> Result<PendingSnapshot, PendingError> {
        let mut c = self.pool.get().map_err(PendingError::Db)?;
        let row = c
            .conn()
            .query_row(
                "SELECT items, summary, integrating, updated_at
                 FROM pending_memory WHERE workspace_id=?1",
                params![ws],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| PendingError::Db(DbError::Migrate(e)))?;

        let Some((items_json, summary, integrating, updated_at)) = row else {
            return Ok(PendingSnapshot::default());
        };
        let items: Vec<PendingItem> = serde_json::from_str(&items_json).unwrap_or_default();
        Ok(PendingSnapshot {
            items,
            summary,
            integrating: integrating != 0,
            updated_at,
        })
    }

    /// 开始一次异步整合：把 `incoming` 与上一轮条目归并，**同步**把
    /// `integrating` 置 true 后立刻返回，**不**在这里写合并结果。
    ///
    /// 调用方应在 `tokio::spawn` 的任务里调用 [`PendingStore::finish_merge`]，
    /// 把 `merged` 落盘并清标志。这样 HTTP 响应不必等磁盘。
    ///
    /// 该 workspace 已在整合中时返回 [`PendingError::Busy`]（挡住重复提交）。
    /// `turn` 缺省 0，用于给没写 `第N轮` 的条目补轮次戳。
    pub fn begin_merge(&self, ws: &str, incoming: &str, turn: u64) -> Result<PendingSnapshot, PendingError> {
        let incoming_items = crate::pending_memory::parse_pending(incoming, turn);
        let mut c = self.pool.get().map_err(PendingError::Db)?;

        let prev_json: Option<String> = c
            .conn()
            .query_row(
                "SELECT items FROM pending_memory WHERE workspace_id=?1",
                params![ws],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| PendingError::Db(DbError::Migrate(e)))?;
        let prev_items: Vec<PendingItem> = prev_json
            .as_deref()
            .and_then(|j| serde_json::from_str(j).ok())
            .unwrap_or_default();

        // integrating 标志的真正用途：挡住重复提交。若上一轮整合还在飞
        // （begin 置了 true 但还没 finish），再 begin 会把同一件事并两次。
        let integrating: Option<i64> = c
            .conn()
            .query_row(
                "SELECT integrating FROM pending_memory WHERE workspace_id=?1",
                params![ws],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(|e| PendingError::Db(DbError::Migrate(e)))?;
        if integrating.unwrap_or(0) != 0 {
            return Err(PendingError::Busy);
        }

        let merged = merge_pending(&prev_items, &incoming_items);

        c.conn()
            .execute(
                "INSERT INTO pending_memory (workspace_id, items, summary, integrating, updated_at)
                 VALUES (?1, ?2, ?3, 1, NULL)
                 ON CONFLICT(workspace_id) DO UPDATE SET integrating=1",
                params![ws, "[]", ""],
            )
            .map_err(|e| PendingError::Db(DbError::Migrate(e)))?;

        Ok(PendingSnapshot {
            items: merged,
            summary: crate::pending_memory::render_pending(&incoming_items),
            integrating: true,
            updated_at: None,
        })
    }

    /// 整合完成：落盘合并结果并清 `integrating` 标志。
    ///
    /// 由 `begin_merge` 返回的快照里的 `items` 传进来（保持「算在哪、存在哪」一致）。
    pub fn finish_merge(&self, ws: &str, merged: &[PendingItem]) -> Result<PendingSnapshot, PendingError> {
        let json = serde_json::to_string(merged)
            .map_err(|e| PendingError::BadRequest(format!("序列化条目失败: {e}")))?;
        let summary = crate::pending_memory::render_pending(merged);
        let now = chrono::Utc::now().to_rfc3339();

        let mut c = self.pool.get().map_err(PendingError::Db)?;
        c.conn()
            .execute(
                "INSERT INTO pending_memory (workspace_id, items, summary, integrating, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4)
                 ON CONFLICT(workspace_id) DO UPDATE SET
                     items=?2, summary=?3, integrating=0, updated_at=?4",
                params![ws, json, summary, now],
            )
            .map_err(|e| PendingError::Db(DbError::Migrate(e)))?;

        Ok(PendingSnapshot {
            items: merged.to_vec(),
            summary,
            integrating: false,
            updated_at: Some(now),
        })
    }

    /// 直接替换条目（宿主判定哪些已兑现后调用），同步落盘并清标志。
    pub fn replace(&self, ws: &str, items: &[PendingItem]) -> Result<PendingSnapshot, PendingError> {
        self.finish_merge(ws, items)
    }

    /// 清掉某 workspace 的全部未兑现项（已全部兑现时调用）。
    pub fn clear(&self, ws: &str) -> Result<(), PendingError> {
        let mut c = self.pool.get().map_err(PendingError::Db)?;
        c.conn()
            .execute(
                "DELETE FROM pending_memory WHERE workspace_id=?1",
                params![ws],
            )
            .map_err(|e| PendingError::Db(DbError::Migrate(e)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = "【悬而未决】#1 第6轮 下周帮苏韵搬茶具\n【悬而未决】#2 第6轮 她说会主动来找你";

    #[test]
    fn get_on_empty_returns_default() {
        let s = PendingStore::open_in_memory().expect("open");
        let snap = s.get("ws_none").expect("get");
        assert_eq!(snap.items.len(), 0);
        assert!(!snap.integrating, "空快照不该在整合中");
    }

    /// `begin_merge` 同步置 integrating=true，`finish_merge` 清掉——这正是
    /// 前端「整合中不要重复提交」的依据。
    #[test]
    fn begin_marks_integrating_then_finish_clears() {
        let s = PendingStore::open_in_memory().expect("open");
        let snap = s.begin_merge("ws", REAL, 0).expect("begin");
        assert!(snap.integrating, "begin_merge 应同步置 integrating=true");
        assert_eq!(snap.items.len(), 2);

        // 合并结果此刻还没落盘。
        assert_eq!(s.get("ws").expect("get").items.len(), 0, "begin 不应写合并结果");

        let done = s.finish_merge("ws", &snap.items).expect("finish");
        assert!(!done.integrating, "finish_merge 应清标志");
        assert!(done.updated_at.is_some());

        let stored = s.get("ws").expect("get");
        assert_eq!(stored.items.len(), 2, "合并结果应已落盘");
        assert_eq!(stored.summary, REAL);
    }

    /// 整合在飞时重复提交 → Busy（这是 integrating 标志的真正用途）。
    #[test]
    fn busy_rejects_double_submit() {
        let s = PendingStore::open_in_memory().expect("open");
        s.begin_merge("ws", REAL, 0).expect("first");
        let second = s.begin_merge("ws", REAL, 0);
        assert!(
            matches!(second, Err(PendingError::Busy)),
            "重复提交应被挡，实际 {:?}",
            second.map(|_| "ok")
        );
        // finish 之后又能提交。
        let snap = s.get("ws").expect("get");
        s.finish_merge("ws", &snap.items).expect("finish");
        // get 出来的 items 是空的（begin 没写结果），再 begin 一次不 Busy。
        assert!(s.begin_merge("ws", REAL, 0).is_ok());
    }

    /// 跨轮：第二轮的新项与第一轮未兑现项合并（`merge_pending` 真正被调上）。
    #[test]
    fn merges_across_turns() {
        let s = PendingStore::open_in_memory().expect("open");
        let t1 = "【悬而未决】#1 第6轮 下周帮苏韵搬茶具";
        let a = s.begin_merge("ws", t1, 6).expect("t1");
        s.finish_merge("ws", &a.items).expect("finish t1");

        let t2 = "【悬而未决】#1 第7轮 明天来苏韵家吃晚饭";
        let b = s.begin_merge("ws", t2, 7).expect("t2");
        assert_eq!(b.items.len(), 2, "上一轮未兑现项应延续：{:?}", b.items);
        assert_eq!(b.items[0].turn, 6, "旧项保留原轮次戳");
        assert_eq!(b.items[1].turn, 7, "新项带本轮轮次戳");
        let done = s.finish_merge("ws", &b.items).expect("finish t2");
        assert_eq!(done.items.len(), 2);
    }

    /// 已兑现的项：宿主用 `replace` 剔掉后，下一轮不该复活。
    #[test]
    fn fulfilled_items_do_not_resurrect() {
        let s = PendingStore::open_in_memory().expect("open");
        let a = s.begin_merge("ws", REAL, 6).expect("begin");
        s.finish_merge("ws", &a.items).expect("finish");

        // 宿主判定第 1 条已兑现，只留第 2 条。
        let kept = vec![a.items[1].clone()];
        s.replace("ws", &kept).expect("replace");
        assert_eq!(s.get("ws").expect("get").items.len(), 1);

        // 下一轮没有新项。
        let b = s.begin_merge("ws", "", 7).expect("begin empty");
        assert_eq!(b.items.len(), 1, "已兑现项不该复活");
        assert!(!b.items.iter().any(|i| i.text.contains("搬茶具")));
    }

    #[test]
    fn clear_removes_everything() {
        let s = PendingStore::open_in_memory().expect("open");
        let a = s.begin_merge("ws", REAL, 6).expect("begin");
        s.finish_merge("ws", &a.items).expect("finish");
        s.clear("ws").expect("clear");
        assert_eq!(s.get("ws").expect("get").items.len(), 0);
    }

    /// 不同 workspace 互不干扰（按 workspace 分片）。
    #[test]
    fn workspaces_are_isolated() {
        let s = PendingStore::open_in_memory().expect("open");
        let a = s.begin_merge("ws_a", REAL, 6).expect("a");
        s.finish_merge("ws_a", &a.items).expect("finish a");
        assert_eq!(s.get("ws_b").expect("get b").items.len(), 0);
        assert_eq!(s.get("ws_a").expect("get a").items.len(), 2);
    }
}
