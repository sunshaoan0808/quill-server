//! zone **格式健康度的按轮回报与留痕**（格式健康度三件套之三）。
//!
//! # 这一件套补的是哪一环
//!
//! 另两件已在库：状态栏「整理」`/panels/tidy`（洗脏 zone）与
//! 「查看真实上下文」`/panels/context`（眼见为实）。三件套成闭环的关键不是这两个
//! 「修」的端点，而是中间那个**先发现问题**的一环——dianziji 的
//! `POST .../messages/{msgId}/format-health`：前端把本轮各 zone 的解析健康度
//! **回报后端**。
//!
//! # 之前为什么这块是空的
//!
//! [`crate::zone_protocol::parse_zones`] 早就能算出 [`ZoneParseReport`]（含
//! `health` 0.0~1.0），`/panels/apply` 也确实把它**返回**给前端了。但那是**单向**
//! 的：算完就丢。后端没有任何表存过它，于是：
//!
//! - 「这轮格式好不好」只能靠用户眼睛盯当下那一条，没有**历史**；
//! - 模型（或某次改 schema）导致格式持续退化时，**趋势看不见**——坏得慢，
//!   每轮都只是「还行」，攒不出「最近 20 轮均值 0.62、且集中在 memory zone」这种结论；
//! - 服务端也就无从按健康度挑模型 / 告警。
//!
//! # 两条上报路径，都留痕
//!
//! 1. **服务端自记**：`/panels/apply` 手里**已经有**那份 [`ZoneParseReport`]，
//!    顺手落一行，零客户端配合、覆盖每一轮（包括非浏览器调用方）。
//! 2. **客户端回报**：`POST /panels/format-health`。对齐 dianziji 的原始设计——
//!    只有客户端完整持有这一轮的原文与**渲染上下文**，它算的 `health` 才是用户
//!    实际看到的那一份；重渲染/分块导致的多次上报以客户端为准更贴近「所见」。
//!
//! 两条路径写**同一张表**，`seq` 相同即覆盖（见 `FormatHealthStore::record` 的
//! `INSERT OR REPLACE`）——所以客户端补报不会在服务端自记之外多留一条。
//!
//! > 这里比 dianziji 多做了一步：原设计只有客户端回报，意味着「模型格式持续退化」
//! > 这种趋势要等每个前端都老老实实上报才看得见。只靠服务端自记覆盖面更大，两者并存最稳。
//!
//! # 留痕的形状
//!
//! 表 [`format_health_log`] 按 `(workspace_id, seq)` 递增追加，`seq` 是 workspace 内
//! 的**单调轮次序号**（客户端可显式给，缺省由服务端自增）——这样 `GET` 能做
//! 「最近 N 轮」与均值，且不必依赖时间戳排序（同毫秒多轮也不会错序）。

use crate::db::{DbError, DbPool};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug)]
pub enum HealthError {
    Db(DbError),
    BadRequest(String),
}

impl std::fmt::Display for HealthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthError::Db(e) => write!(f, "format-health db: {e}"),
            HealthError::BadRequest(m) => write!(f, "bad request: {m}"),
        }
    }
}

impl std::error::Error for HealthError {}

impl From<DbError> for HealthError {
    fn from(e: DbError) -> Self {
        HealthError::Db(e)
    }
}

/// 一轮的格式健康度记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatHealthRecord {
    /// workspace 内的轮次序号（单调递增）。
    pub seq: u64,
    /// 健康度 0.0~1.0（成功 zone / 总 zone）。
    pub health: f32,
    /// 成功解析的 zone 数。
    pub ok_zones: usize,
    /// 解析失败/空的 zone 数。
    pub bad_zones: usize,
    /// 本轮出现的 zone 名（按出现顺序）。
    pub zones: Vec<String>,
    /// 每 zone 的问题描述（空 = 没问题）。
    pub issues: Vec<String>,
    /// 是否本轮已自动清洗过（主链路 `ingest_zone_output` 的 auto_tidy）。
    #[serde(default)]
    pub tidied: bool,
    /// 客户端上报时刻（RFC3339）；客户端可自带，缺省服务端填。
    #[serde(default)]
    pub at: String,
}

/// 一段历史的汇总（`GET` 的返回值）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatHealthSummary {
    /// 明细（最近 N 轮，按 seq **升序**，老的在前——便于直接画趋势）。
    pub records: Vec<FormatHealthRecord>,
    /// 参与统计的轮数（可能小于 `records.len()`：无 zone 的轮次不计入均值）。
    pub turns: usize,
    /// 均值健康度（`turns == 0` 时为 `None`）。
    pub avg_health: Option<f32>,
    /// 最差的一轮（`None` 表示无记录）。
    pub worst: Option<FormatHealthRecord>,
}

/// zone 格式健康度留痕仓库（SQLite，按 workspace 分片）。
#[derive(Clone)]
pub struct FormatHealthStore {
    pool: DbPool,
}

impl FormatHealthStore {
    pub fn open(path: &Path) -> Result<Self, HealthError> {
        let pool = DbPool::open(path, 4).map_err(HealthError::Db)?;
        Self::ensure_schema(&pool)?;
        Ok(FormatHealthStore { pool })
    }

    pub fn open_in_memory() -> Result<Self, HealthError> {
        let pool = DbPool::open_in_memory(4).map_err(HealthError::Db)?;
        Self::ensure_schema(&pool)?;
        Ok(FormatHealthStore { pool })
    }

    fn ensure_schema(pool: &DbPool) -> Result<(), HealthError> {
        let mut c = pool.get().map_err(HealthError::Db)?;
        c.conn()
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS format_health_log (
                     workspace_id TEXT NOT NULL,
                     seq          INTEGER NOT NULL,
                     health       REAL NOT NULL,
                     ok_zones     INTEGER NOT NULL DEFAULT 0,
                     bad_zones    INTEGER NOT NULL DEFAULT 0,
                     zones        TEXT NOT NULL DEFAULT '[]',
                     issues       TEXT NOT NULL DEFAULT '[]',
                     tidied       INTEGER NOT NULL DEFAULT 0,
                     at           TEXT NOT NULL DEFAULT '',
                     PRIMARY KEY (workspace_id, seq)
                 );
                 CREATE INDEX IF NOT EXISTS idx_fhl_ws_at
                     ON format_health_log(workspace_id, at);",
            )
            .map_err(|e| HealthError::Db(DbError::Migrate(e)))?;
        Ok(())
    }

    /// 追加一轮。`seq` 为 `None` 时由服务端自增（接上次 +1）。
    ///
    /// 自增必须**在同一个事务里**读上次值并写入，否则同一 workspace 的两次并发
    /// 上报会算出同一个 seq，撞主键。
    pub fn record(
        &self,
        ws: &str,
        rec: &FormatHealthRecord,
    ) -> Result<FormatHealthRecord, HealthError> {
        if !(0.0..=1.0).contains(&rec.health) {
            return Err(HealthError::BadRequest(format!(
                "health 必须在 0.0~1.0，收到 {}",
                rec.health
            )));
        }
        let mut c = self.pool.get().map_err(HealthError::Db)?;
        let conn = c.conn();
        let tx = conn.transaction().map_err(|e| HealthError::Db(DbError::Migrate(e)))?;

        // seq = 0 表示「服务端自增」（客户端没带序号时的默认）。
        let seq: u64 = if rec.seq > 0 {
            rec.seq
        } else {
                // MAX(seq) 空表时返回 SQL NULL，必须在闭包里就接成 Option<i64>——
                // 类型检查发生在 optional() 之前，声明成 i64 会先炸 InvalidColumnType。
                let last: Option<i64> = tx
                    .query_row(
                        "SELECT MAX(seq) FROM format_health_log WHERE workspace_id=?1",
                        params![ws],
                        |r| r.get::<_, Option<i64>>(0),
                    )
                    .map_err(|e| HealthError::Db(DbError::Migrate(e)))?;
                last.unwrap_or(0).max(0) as u64 + 1
        };

        let at = if rec.at.trim().is_empty() {
            chrono::Utc::now().to_rfc3339()
        } else {
            rec.at.clone()
        };
        let out = FormatHealthRecord { seq, at, ..rec.clone() };
        tx.execute(
            "INSERT OR REPLACE INTO format_health_log
                 (workspace_id, seq, health, ok_zones, bad_zones, zones, issues, tidied, at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                ws,
                out.seq as i64,
                out.health as f64,
                out.ok_zones as i64,
                out.bad_zones as i64,
                serde_json::to_string(&out.zones).unwrap_or_else(|_| "[]".into()),
                serde_json::to_string(&out.issues).unwrap_or_else(|_| "[]".into()),
                out.tidied as i64,
                out.at,
            ],
        )
        .map_err(|e| HealthError::Db(DbError::Migrate(e)))?;
        tx.commit().map_err(|e| HealthError::Db(DbError::Migrate(e)))?;
        Ok(out)
    }

    /// 读最近 `limit` 轮（按 seq 升序）。`limit == 0` 视作 20。
    pub fn recent(&self, ws: &str, limit: usize) -> Result<FormatHealthSummary, HealthError> {
        let limit = if limit == 0 { 20 } else { limit };
        let mut c = self.pool.get().map_err(HealthError::Db)?;
        // 先取最大 seq，再按 seq 取最近 N 条——这样 SQLite 才可能走主键倒序扫描，
        // 而不是全表扫 + 排序。
        // 同 record()：MAX(seq) 在无记录时是 SQL NULL。
        let max_seq: i64 = c
            .conn()
            .query_row(
                "SELECT MAX(seq) FROM format_health_log WHERE workspace_id=?1",
                params![ws],
                |r| r.get::<_, Option<i64>>(0),
            )
            .map_err(|e| HealthError::Db(DbError::Migrate(e)))?
            .unwrap_or(0);
        if max_seq == 0 {
            return Ok(FormatHealthSummary {
                records: vec![],
                turns: 0,
                avg_health: None,
                worst: None,
            });
        }

        let mut stmt = c
            .conn()
            .prepare(
                "SELECT seq, health, ok_zones, bad_zones, zones, issues, tidied, at
                 FROM format_health_log
                 WHERE workspace_id=?1 AND seq > ?2
                 ORDER BY seq ASC",
            )
            .map_err(|e| HealthError::Db(DbError::Migrate(e)))?;
        let rows = stmt
            .query_map(
                params![ws, max_seq - limit as i64],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, i64>(6)?,
                        r.get::<_, String>(7)?,
                    ))
                },
            )
            .map_err(|e| HealthError::Db(DbError::Migrate(e)))?;

        let mut records = Vec::new();
        for row in rows {
            let (seq, health, ok, bad, zones, issues, tidied, at) =
                row.map_err(|e| HealthError::Db(DbError::Migrate(e)))?;
            records.push(FormatHealthRecord {
                seq: seq.max(0) as u64,
                health: health as f32,
                ok_zones: ok.max(0) as usize,
                bad_zones: bad.max(0) as usize,
                zones: serde_json::from_str(&zones).unwrap_or_default(),
                issues: serde_json::from_str(&issues).unwrap_or_default(),
                tidied: tidied != 0,
                at,
            });
        }
        Ok(Self::summarize(records))
    }

    /// 汇总：均值只计「有 zone」的轮次——模型这轮压根没吐 zone（`bad == 0 && ok == 0`）
    /// 是另一种病（漏写），混进均值会把格式健康度的锅算到「凑不齐 zone」上。
    fn summarize(records: Vec<FormatHealthRecord>) -> FormatHealthSummary {
        let scored: Vec<&FormatHealthRecord> = records.iter().filter(|r| r.ok_zones > 0).collect();
        let turns = scored.len();
        let avg_health = if turns == 0 {
            None
        } else {
            Some((scored.iter().map(|r| r.health as f64).sum::<f64>() / turns as f64) as f32)
        };
        let worst = scored
            .iter()
            .min_by(|a, b| {
                a.health
                    .partial_cmp(&b.health)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|r| (*r).clone());
        FormatHealthSummary {
            records,
            turns,
            avg_health,
            worst,
        }
    }

    /// 清空某 workspace 的全部留痕（换书/重置用）。
    pub fn clear(&self, ws: &str) -> Result<usize, HealthError> {
        let mut c = self.pool.get().map_err(HealthError::Db)?;
        let n = c
            .conn()
            .execute(
                "DELETE FROM format_health_log WHERE workspace_id=?1",
                params![ws],
            )
            .map_err(|e| HealthError::Db(DbError::Migrate(e)))?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(health: f32, ok: usize, bad: usize) -> FormatHealthRecord {
        FormatHealthRecord {
            seq: 0,
            health,
            ok_zones: ok,
            bad_zones: bad,
            zones: vec!["scene".into(), "player".into()],
            issues: vec!["zone `player` 内容为空".into()],
            tidied: false,
            at: String::new(),
        }
    }

    #[test]
    fn seq_autoincrements_per_workspace() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        assert_eq!(s.record("w1", &rec(1.0, 2, 0)).unwrap().seq, 1);
        assert_eq!(s.record("w1", &rec(0.5, 1, 1)).unwrap().seq, 2);
        // 另一个 workspace 独立计数
        assert_eq!(s.record("w2", &rec(1.0, 2, 0)).unwrap().seq, 1);
        assert_eq!(s.record("w1", &rec(1.0, 2, 0)).unwrap().seq, 3);
    }

    #[test]
    fn explicit_seq_is_honored_and_replaceable() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        let mut r = rec(0.5, 1, 1);
        r.seq = 7;
        assert_eq!(s.record("w1", &r).unwrap().seq, 7);
        // 同 seq 再报 = 覆盖那一轮（客户端重渲染重报，不该留两条）
        let mut r2 = rec(1.0, 2, 0);
        r2.seq = 7;
        s.record("w1", &r2).unwrap();
        let sum = s.recent("w1", 20).unwrap();
        assert_eq!(sum.records.len(), 1, "同 seq 应覆盖: {sum:?}");
        assert_eq!(sum.records[0].health, 1.0);
    }

    #[test]
    fn health_out_of_range_rejected() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        assert!(s.record("w1", &rec(1.5, 2, 0)).is_err());
        assert!(s.record("w1", &rec(-0.1, 0, 2)).is_err());
        assert!(s.record("w1", &rec(0.0, 0, 2)).is_ok(), "0.0 是合法值（全灭）");
    }

    #[test]
    fn recent_returns_ascending_and_trims_to_limit() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        for i in 0..10 {
            s.record("w1", &rec(1.0 - (i as f32) * 0.1, 2, 0)).unwrap();
        }
        let sum = s.recent("w1", 3).unwrap();
        assert_eq!(sum.records.len(), 3, "只取最近 3 轮");
        let seqs: Vec<u64> = sum.records.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![8, 9, 10], "应按 seq 升序（老的在前）：{seqs:?}");
    }

    #[test]
    fn empty_workspace_yields_empty_summary() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        let sum = s.recent("没人", 20).unwrap();
        assert!(sum.records.is_empty());
        assert_eq!(sum.turns, 0);
        assert_eq!(sum.avg_health, None);
        assert!(sum.worst.is_none());
    }

    #[test]
    fn avg_skips_turns_with_no_zone_at_all() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        s.record("w1", &rec(1.0, 2, 0)).unwrap(); // seq1 有 zone
        s.record("w1", &rec(0.0, 0, 0)).unwrap(); // seq2 一个 zone 都没吐（漏写，不能算格式差）
        let sum = s.recent("w1", 20).unwrap();
        assert_eq!(sum.records.len(), 2, "明细仍含该轮");
        assert_eq!(sum.turns, 1, "均值只计有 zone 的轮次");
        assert_eq!(sum.avg_health, Some(1.0));
    }

    #[test]
    fn worst_picks_lowest_scored_turn() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        s.record("w1", &rec(1.0, 2, 0)).unwrap();
        s.record("w1", &rec(0.5, 1, 1)).unwrap();
        s.record("w1", &rec(0.75, 3, 1)).unwrap();
        let sum = s.recent("w1", 20).unwrap();
        assert_eq!(sum.turns, 3);
        let avg = (1.0 + 0.5 + 0.75) / 3.0;
        assert!((sum.avg_health.unwrap() - avg).abs() < 1e-5, "{:?}", sum.avg_health);
        assert_eq!(sum.worst.unwrap().seq, 2);
    }

    #[test]
    fn clear_empties_only_target_workspace() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        s.record("w1", &rec(1.0, 2, 0)).unwrap();
        s.record("w2", &rec(1.0, 2, 0)).unwrap();
        assert_eq!(s.clear("w1").unwrap(), 1);
        assert!(s.recent("w1", 20).unwrap().records.is_empty());
        assert_eq!(s.recent("w2", 20).unwrap().records.len(), 1);
    }

    #[test]
    fn at_defaults_when_client_omits() {
        let s = FormatHealthStore::open_in_memory().unwrap();
        let got = s.record("w1", &rec(1.0, 2, 0)).unwrap();
        assert!(!got.at.is_empty(), "客户端不带时刻时服务端应补");
        // 自带时刻则原样保留
        let mut r = rec(1.0, 2, 0);
        r.at = "2026-10-05T00:00:00Z".into();
        assert_eq!(s.record("w1", &r).unwrap().at, "2026-10-05T00:00:00Z");
    }
}
