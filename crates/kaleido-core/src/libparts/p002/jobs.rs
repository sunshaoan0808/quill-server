//! p002/jobs —— JobStore 定义 + 生命周期（创建/启动/终态/取消/控制）。
//! [R6-2③ 2026-10-07] 自 p002.rs 机械迁入；事件与运维方法见 jobs_ops.rs（同 impl 拆两块）。
#![allow(unused_imports)]

use super::*;

#[derive(Clone)]
pub struct JobStore {
    pub(crate) data: DataRoot,
    /// In-memory index of non-terminal jobs (queued + running). Terminal jobs live on disk only.
    pub(crate) active: Arc<Mutex<HashMap<String, JobRecord>>>,
    pub(crate) max_concurrent: usize,
    /// After a restart, re-schedules recovered non-terminal jobs. kaleido-core stays
    /// server-agnostic: the hook only receives the JobRecord and the AppState side decides
    /// whether/how to restart the underlying worker (default no-op).
    pub(crate) recover_hook: Arc<dyn Fn(&JobRecord) + Send + Sync>,
    /// P8: 并发水位/终态计数（见 [`JobMetrics`]）。
    pub metrics: Arc<JobMetrics>,
}

/// P8: 进程生命周期内的作业并发水位与终态计数（Atomic，无锁采样）。
/// P10: totals（created/succeeded/failed/cancelled）落盘 `jobs_dir/metrics.json`
/// 并在 boot 恢复 —— 跨重启累计；peak_running / boot_at_unix 保持进程作用域。
/// /health 暴露为 `jobs_metrics`。
#[derive(Debug, Default)]
pub struct JobMetrics {
    pub boot_at_unix: AtomicU64,
    pub peak_running: AtomicU64,
    pub total_created: AtomicU64,
    pub total_succeeded: AtomicU64,
    pub total_failed: AtomicU64,
    pub total_cancelled: AtomicU64,
}

/// P10: metrics.json 落盘快照（仅累计 totals；peak/boot 是进程作用域，不落盘）。
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct JobMetricsSnapshot {
    pub(crate) total_created: u64,
    pub(crate) total_succeeded: u64,
    total_failed: u64,
    pub(crate) total_cancelled: u64,
}

impl JobMetrics {
    /// 与 job 记录同目录；reload/prune 均按文件名显式排除本文件。
    pub(crate) const FILE_NAME: &'static str = "metrics.json";

    fn snapshot(&self) -> JobMetricsSnapshot {
        JobMetricsSnapshot {
            total_created: self.total_created.load(Ordering::Relaxed),
            total_succeeded: self.total_succeeded.load(Ordering::Relaxed),
            total_failed: self.total_failed.load(Ordering::Relaxed),
            total_cancelled: self.total_cancelled.load(Ordering::Relaxed),
        }
    }

    /// P10: totals 落盘（tmp+rename 原子替换）。失败静默 —— 指标采集不得影响主流程。
    pub(crate) fn persist(&self, dir: &Path) {
        let body = match serde_json::to_string(&self.snapshot()) {
            Ok(b) => b,
            Err(_) => return,
        };
        let tmp = dir.join(format!("{}.tmp", Self::FILE_NAME));
        let dst = dir.join(Self::FILE_NAME);
        if fs::write(&tmp, body).is_ok() {
            let _ = fs::rename(&tmp, &dst);
        }
    }

    /// P10: boot 时恢复累计 totals（store 语义：无文件/损坏文件 → 从 0 起算）。
    fn restore_totals(&self, dir: &Path) {
        if let Ok(raw) = fs::read_to_string(dir.join(Self::FILE_NAME)) {
            if let Ok(snap) = serde_json::from_str::<JobMetricsSnapshot>(&raw) {
                self.total_created.store(snap.total_created, Ordering::Relaxed);
                self.total_succeeded.store(snap.total_succeeded, Ordering::Relaxed);
                self.total_failed.store(snap.total_failed, Ordering::Relaxed);
                self.total_cancelled.store(snap.total_cancelled, Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn note_terminal(&self, status: &str) {
        match status {
            "succeeded" | "done" => self.total_succeeded.fetch_add(1, Ordering::Relaxed),
            "failed" | "error" => self.total_failed.fetch_add(1, Ordering::Relaxed),
            "cancelled" | "stopped" => self.total_cancelled.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
    }
}

/// P10: env 解析纯函数化 —— `JobStore::new` 即 `parse_max_concurrent(env) + with_max_concurrent`。
/// 抽出来是为了让测试直测解析/clamp 语义，无需 set_var 污染进程级全局态（并行竞态根源）。
pub(crate) fn parse_max_concurrent(raw: Option<&str>) -> usize {
    raw.and_then(|s| s.parse().ok()).unwrap_or(2).clamp(1, 4)
}

impl JobStore {
    pub fn new(data: DataRoot) -> Self {
        let raw = std::env::var("KALEIDO_MAX_CONCURRENT_JOBS").ok();
        Self::with_max_concurrent(data, parse_max_concurrent(raw.as_deref()))
    }

    /// Construct with an explicit concurrency cap (clamped to 1..=2).
    /// Prefer this in tests so capacity is not affected by process-wide env races.
    pub fn with_max_concurrent(data: DataRoot, max_concurrent: usize) -> Self {
        let max_concurrent = max_concurrent.clamp(1, 4);
        let store = Self {
            data,
            active: Arc::new(Mutex::new(HashMap::new())),
            max_concurrent,
            recover_hook: Arc::new(|_| {}),
            metrics: Arc::new(JobMetrics {
                boot_at_unix: AtomicU64::new(Utc::now().timestamp().max(0) as u64),
                ..Default::default()
            }),
        };
        // Reload non-terminal jobs from disk so restart keeps queue state.
        if let Ok(entries) = fs::read_dir(store.data.jobs_dir()) {
            let mut active = store.active.lock();
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                // P10: 指标快照不是 job 记录，跳过（否则 serde 解析失败走 if-let 静默分支）。
                if path.file_name().and_then(|e| e.to_str()) == Some(JobMetrics::FILE_NAME) {
                    continue;
                }
                if let Ok(raw) = fs::read_to_string(&path) {
                    if let Ok(mut rec) = serde_json::from_str::<JobRecord>(&raw) {
                        rec.status = normalize_job_status(&rec.status);
                        if is_active_job_status(&rec.status) {
                            active.insert(rec.run_id.clone(), rec);
                        }
                    }
                }
            }
        }
        // P10: 恢复跨重启累计 totals（peak/boot_at_unix 保持本进程作用域）。
        store.metrics.restore_totals(&store.data.jobs_dir());
        store
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    pub fn running_count(&self) -> usize {
        self.active
            .lock()
            .values()
            .filter(|j| normalize_job_status(&j.status) == "running")
            .count()
    }

    pub fn queued_count(&self) -> usize {
        self.active
            .lock()
            .values()
            .filter(|j| normalize_job_status(&j.status) == "queued")
            .count()
    }

    /// Chat / immediate-start path. Fails with RateLimited when at capacity (no queue).
    /// Preserves S4 mobile chat contract.
    pub fn try_start(
        &self,
        kind: &str,
        user_id: &str,
        workspace_id: &str,
        model: Option<String>,
        meta: Value,
    ) -> CoreResult<JobRecord> {
        let mut active = self.active.lock();
        let running = active
            .values()
            .filter(|j| normalize_job_status(&j.status) == "running")
            .count();
        if running >= self.max_concurrent {
            return Err(CoreError::RateLimited(format!(
                "max concurrent jobs ({})",
                self.max_concurrent
            )));
        }
        let now = Utc::now();
        let run_id = Uuid::new_v4().to_string();
        let rec = JobRecord {
            run_id: run_id.clone(),
            kind: kind.into(),
            user_id: user_id.into(),
            workspace_id: workspace_id.into(),
            status: "running".into(),
            model,
            created_at: now,
            updated_at: now,
            meta,
            payload: None,
            progress: Some(0.0),
            progress_message: None,
            cursor: None,
            error: None,
            result: None,
            events: Vec::new(),
        };
        active.insert(run_id.clone(), rec.clone());
        let _ = self.persist(&rec);
        Ok(rec)
    }

    /// Jobs v2 create: queues when at concurrency limit (does NOT 429).
    /// Overflow policy: **queue** (`status=queued`), never reject.
    pub fn create(
        &self,
        kind: &str,
        user_id: &str,
        workspace_id: &str,
        payload: Value,
        model: Option<String>,
        meta: Option<Value>,
    ) -> CoreResult<JobRecord> {
        let kind = normalize_job_kind(kind);
        let mut active = self.active.lock();
        let running = active
            .values()
            .filter(|j| normalize_job_status(&j.status) == "running")
            .count();
        let status = if running < self.max_concurrent {
            "running"
        } else {
            "queued"
        };
        let now = Utc::now();
        let run_id = Uuid::new_v4().to_string();
        let rec = JobRecord {
            run_id: run_id.clone(),
            kind,
            user_id: user_id.into(),
            workspace_id: workspace_id.into(),
            status: status.into(),
            model,
            created_at: now,
            updated_at: now,
            meta: meta.unwrap_or_else(|| json!({})),
            payload: Some(payload),
            progress: Some(0.0),
            progress_message: Some(if status == "queued" {
                "queued".into()
            } else {
                "started".into()
            }),
            cursor: None,
            error: None,
            result: None,
            events: vec![JobEvent::event(
                if status == "queued" {
                    "job queued (at concurrency limit)"
                } else {
                    "job started"
                },
                None,
            )],
        };
        active.insert(run_id.clone(), rec.clone());
        self.metrics.total_created.fetch_add(1, Ordering::Relaxed);
        // P8: 记录并发水位峰值（含本 job）。
        let running_now = active
            .values()
            .filter(|j| normalize_job_status(&j.status) == "running")
            .count() as u64;
        self.metrics
            .peak_running
            .fetch_max(running_now, Ordering::Relaxed);
        self.persist(&rec)?;
        drop(active);
        // P10: totals 增量落盘（锁外，避免拉长 active 持锁时间）。
        self.metrics.persist(&self.data.jobs_dir());
        Ok(rec)
    }

    /// Finish a job (chat path). Accepts legacy done/error/stopped and maps them.
    /// Also promotes the oldest queued job to running when a slot frees.
    pub fn finish(&self, run_id: &str, status: &str) {
        let canonical = normalize_job_status(status);
        self.metrics.note_terminal(&canonical);
        let mut active = self.active.lock();
        if let Some(j) = active.get_mut(run_id) {
            j.status = canonical.clone();
            j.updated_at = Utc::now();
            if canonical == "succeeded" {
                j.progress = Some(1.0);
            }
            if canonical == "failed" && j.error.is_none() {
                j.error = Some("failed".into());
            }
            let _ = self.persist(j);
        } else if let Some(mut rec) = self.load_from_disk(run_id) {
            rec.status = canonical.clone();
            rec.updated_at = Utc::now();
            let _ = self.persist(&rec);
        }
        active.retain(|_, j| is_active_job_status(&j.status));
        drop(active);
        // P10: 终态计数落盘。
        self.metrics.persist(&self.data.jobs_dir());
        let _ = self.promote_queued();
    }

    /// Jobs v2 cancel — idempotent. Terminal jobs stay as-is; active → cancelled.
    pub fn cancel(&self, run_id: &str) -> CoreResult<JobRecord> {
        let mut active = self.active.lock();
        if let Some(j) = active.get_mut(run_id) {
            let status = normalize_job_status(&j.status);
            if is_terminal_job_status(&status) {
                let rec = j.clone();
                return Ok(rec);
            }
            j.status = "cancelled".into();
            j.updated_at = Utc::now();
            j.progress_message = Some("cancelled".into());
            j.events.push(JobEvent::event("cancelled", None));
            trim_events_with_delta_tail(&mut j.events);
            self.metrics.note_terminal("cancelled");
            let rec = j.clone();
            let _ = self.persist(&rec);
            active.retain(|_, j| is_active_job_status(&j.status));
            drop(active);
            // P10: 终态计数落盘。
            self.metrics.persist(&self.data.jobs_dir());
            let _ = self.promote_queued();
            return Ok(rec);
        }
        drop(active);
        if let Some(mut rec) = self.load_from_disk(run_id) {
            if is_terminal_job_status(&rec.status) {
                return Ok(rec);
            }
            rec.status = "cancelled".into();
            rec.updated_at = Utc::now();
            rec.progress_message = Some("cancelled".into());
            rec.events.push(JobEvent::event("cancelled", None));
            trim_events_with_delta_tail(&mut rec.events);
            self.metrics.note_terminal("cancelled");
            self.persist(&rec)?;
            // P10: 终态计数落盘（磁盘分支，无锁持有）。
            self.metrics.persist(&self.data.jobs_dir());
            let _ = self.promote_queued();
            return Ok(rec);
        }
        Err(CoreError::NotFound(format!("job {run_id}")))
    }

    /// Write a non-destructive control request into `payload.control` (pause/resume).
    ///
    /// Unlike `cancel` (which flips status to a terminal state immediately), pause keeps the
    /// job `running` in the active set so its exec can pause at the next checkpoint and resume
    /// from disk. Cancel is still expressed via `cancel()` + status check in the exec loop.
    /// Returns the (possibly updated) job record.
    pub fn control(&self, run_id: &str, action: &str) -> CoreResult<JobRecord> {
        let action = match action {
            "pause" | "resume" => action,
            other => {
                return Err(CoreError::BadRequest(format!(
                    "unsupported control action: {other} (pause|resume)"
                )))
            }
        };
        let mut active = self.active.lock();
        let mut rec = if let Some(j) = active.get(run_id).cloned() {
            j
        } else if let Some(j) = self.load_from_disk(run_id) {
            j
        } else {
            return Err(CoreError::NotFound(format!("job {run_id}")));
        };
        if is_terminal_job_status(&rec.status) {
            return Err(CoreError::BadRequest(format!(
                "job {run_id} is {}; cannot {}",
                normalize_job_status(&rec.status),
                action
            )));
        }
        let mut payload = rec.payload.unwrap_or_else(|| json!({}));
        if let Some(obj) = payload.as_object_mut() {
            obj.insert(
                "control".into(),
                json!({
                    "action": action,
                    "at": Utc::now().to_rfc3339(),
                }),
            );
        } else {
            payload = json!({ "control": { "action": action } });
        }
        rec.payload = Some(payload);
        rec.updated_at = Utc::now();
        rec.progress_message = Some(match action {
            "pause" => "pause 请求已发出，等待任务在检查点暂停…".into(),
            _ => "resume 请求已发出…".into(),
        });
        rec.events.push(JobEvent::event(
            format!("control: {action} requested"),
            None,
        ));
        trim_events_with_delta_tail(&mut rec.events);
        if is_active_job_status(&rec.status) {
            active.insert(rec.run_id.clone(), rec.clone());
        }
        self.persist(&rec)?;
        Ok(rec)
    }

    /// Read the control request (pause/resume) stored in `payload.control` for an exec loop.
    pub fn control_action(&self, run_id: &str) -> Option<String> {
        let rec = self.get(run_id)?;
        let action = rec
            .payload
            .as_ref()
            .and_then(|p| p.get("control"))
            .and_then(|c| c.get("action"))
            .and_then(|a| a.as_str())?;
        Some(action.to_string())
    }

    /// Clear the control request once an exec has processed it (e.g. after resume).
    pub fn clear_control(&self, run_id: &str) -> CoreResult<JobRecord> {
        let mut active = self.active.lock();
        let mut rec = if let Some(j) = active.get(run_id).cloned() {
            j
        } else if let Some(j) = self.load_from_disk(run_id) {
            j
        } else {
            return Err(CoreError::NotFound(format!("job {run_id}")));
        };
        if let Some(obj) = rec.payload.as_mut().and_then(|p| p.as_object_mut()) {
            obj.remove("control");
        }
        rec.updated_at = Utc::now();
        if is_active_job_status(&rec.status) {
            active.insert(rec.run_id.clone(), rec.clone());
        }
        self.persist(&rec)?;
        Ok(rec)
    }

    pub fn get(&self, run_id: &str) -> Option<JobRecord> {
        if let Some(j) = self.active.lock().get(run_id).cloned() {
            return Some(j);
        }
        self.load_from_disk(run_id)
    }

    /// P2: 按 id 物理删除一条终态 job（active 索引 + 磁盘 JSON）。
    ///
    /// 安全保护：只允许删除**终态** job（succeeded/failed/cancelled）。若目标仍在
    /// `active` 集合且非终态（running/queued/paused），返回 `BadRequest`，防止误删
    /// 正在执行或排队中的任务。删除是物理性的——文件从磁盘移除后无法通过 `get`
    /// 找回，也不会再出现在 `list` 中。jobs 目录的批量增长整理见 `prune_terminal`。
    pub fn delete(&self, run_id: &str) -> CoreResult<()> {
        let mut active = self.active.lock();
        let disk_exists = self.data.jobs_dir().join(format!("{run_id}.json")).exists();
        let in_active = active.contains_key(run_id);
        if !in_active && !disk_exists {
            return Err(CoreError::NotFound(format!("job {run_id}")));
        }
        if let Some(j) = active.get(run_id) {
            if !is_terminal_job_status(&normalize_job_status(&j.status)) {
                return Err(CoreError::BadRequest(format!(
                    "job {run_id} is {}; only terminal jobs can be deleted",
                    normalize_job_status(&j.status)
                )));
            }
        }
        // 先从内存 active 索引移除，再删磁盘文件；文件不存在时静默忽略（rm 语义）。
        active.retain(|id, _| id != run_id);
        drop(active);
        if let Ok(err) = fs::remove_file(self.data.jobs_dir().join(format!("{run_id}.json"))) {
            // remove_file Ok(路径) —— 实际返回 io::Result<()>；类型签名见下方
            let _ = err;
        }
        Ok(())
    }


    /// List jobs (memory active + disk), newest first. Filters optional.
    pub fn list(&self, filter: JobListFilter) -> CoreResult<Vec<JobRecord>> {
        let limit = if filter.limit == 0 {
            50
        } else {
            filter.limit.min(500)
        };
        let mut by_id: HashMap<String, JobRecord> = HashMap::new();

        // Disk first, then overlay active (fresher).
        if let Ok(entries) = fs::read_dir(self.data.jobs_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(raw) = fs::read_to_string(&path) {
                    if let Ok(mut rec) = serde_json::from_str::<JobRecord>(&raw) {
                        rec.status = normalize_job_status(&rec.status);
                        by_id.insert(rec.run_id.clone(), rec);
                    }
                }
            }
        }
        for (id, rec) in self.active.lock().iter() {
            by_id.insert(id.clone(), rec.clone());
        }

        let mut items: Vec<JobRecord> = by_id.into_values().collect();
        items.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let status_filter = filter
            .status
            .as_ref()
            .map(|s| normalize_job_status(s));
        items.retain(|j| {
            if let Some(ref st) = status_filter {
                if normalize_job_status(&j.status) != *st {
                    return false;
                }
            }
            if let Some(ref k) = filter.kind {
                if &j.kind != k {
                    return false;
                }
            }
            if let Some(ref u) = filter.user_id {
                if &j.user_id != u {
                    return false;
                }
            }
            if let Some(ref w) = filter.workspace_id {
                if &j.workspace_id != w {
                    return false;
                }
            }
            true
        });
        items.truncate(limit);
        Ok(items)
    }
}
