//! p002/jobs_ops —— JobStore 事件流/记账/恢复/清理（impl JobStore 第二块）。
//! [R6-2③ 2026-10-07] 自 p002.rs 机械迁入。
#![allow(unused_imports)]

use super::*;

impl JobStore {
    pub fn push_event(
        &self,
        run_id: &str,
        event: JobEvent,
        progress: Option<f64>,
        cursor: Option<String>,
    ) -> CoreResult<JobRecord> {
        let mut active = self.active.lock();
        if let Some(j) = active.get_mut(run_id) {
            if let Some(p) = progress {
                j.progress = Some(p.clamp(0.0, 1.0));
            }
            if let Some(msg) = event.message.clone() {
                j.progress_message = Some(msg);
            }
            if let Some(c) = cursor {
                j.cursor = Some(c);
            }
            j.updated_at = Utc::now();
            j.events.push(event);
            trim_events_with_delta_tail(&mut j.events);
            let rec = j.clone();
            self.persist(&rec)?;
            return Ok(rec);
        }
        drop(active);
        if let Some(mut rec) = self.load_from_disk(run_id) {
            if let Some(p) = progress {
                rec.progress = Some(p.clamp(0.0, 1.0));
            }
            if let Some(msg) = event.message.clone() {
                rec.progress_message = Some(msg);
            }
            if let Some(c) = cursor {
                rec.cursor = Some(c);
            }
            rec.updated_at = Utc::now();
            rec.events.push(event);
            trim_events_with_delta_tail(&mut rec.events);
            if is_active_job_status(&rec.status) {
                self.active.lock().insert(rec.run_id.clone(), rec.clone());
            }
            self.persist(&rec)?;
            return Ok(rec);
        }
        Err(CoreError::NotFound(format!("job {run_id}")))
    }

    /// Complete a jobs-v2 job with optional result payload.
    pub fn complete(
        &self,
        run_id: &str,
        status: &str,
        result: Option<Value>,
        error: Option<String>,
    ) -> CoreResult<JobRecord> {
        let canonical = normalize_job_status(status);
        if !is_terminal_job_status(&canonical) {
            return Err(CoreError::BadRequest(format!(
                "complete requires terminal status, got {canonical}"
            )));
        }
        let mut active = self.active.lock();
        let mut rec = if let Some(j) = active.remove(run_id) {
            j
        } else if let Some(j) = self.load_from_disk(run_id) {
            j
        } else {
            return Err(CoreError::NotFound(format!("job {run_id}")));
        };
        // Cancel wins: a late complete(succeeded) after stop must not resurrect the job.
        let prior = normalize_job_status(&rec.status);
        if prior == "cancelled" && canonical != "cancelled" {
            rec.updated_at = Utc::now();
            rec.events.push(JobEvent::event(
                format!("complete ignored (already cancelled; wanted {canonical})"),
                None,
            ));
            trim_events_with_delta_tail(&mut rec.events);
            self.persist(&rec)?;
            drop(active);
            self.metrics.persist(&self.data.jobs_dir());
            let _ = self.promote_queued();
            return Ok(rec);
        }
        if is_terminal_job_status(&prior) && prior != canonical {
            // Keep first terminal status (failed stays failed, etc.)
            rec.updated_at = Utc::now();
            rec.events.push(JobEvent::event(
                format!("complete ignored (already {prior}; wanted {canonical})"),
                None,
            ));
            trim_events_with_delta_tail(&mut rec.events);
            self.persist(&rec)?;
            drop(active);
            self.metrics.persist(&self.data.jobs_dir());
            let _ = self.promote_queued();
            return Ok(rec);
        }
        rec.status = canonical.clone();
        rec.updated_at = Utc::now();
        // P10: complete() 是 director-plan 等后台任务的正式终态路径 —— P8 漏了此处计数，
        // 导致 /health totals 漏记该类 job（冒烟发现：director_plan succeeded 后 total_succeeded=0）。
        // 注意：必须放在「cancel wins / already-terminal」早退之后 —— 迟到/重复的 complete
        // 不复活记录、也不重复计数。
        self.metrics.note_terminal(&canonical);
        if let Some(r) = result {
            rec.result = Some(r);
        }
        if let Some(e) = error {
            rec.error = Some(e.clone());
            rec.events.push(JobEvent::error(e));
        } else if canonical == "succeeded" {
            rec.progress = Some(1.0);
            rec.events.push(JobEvent::done(Some("succeeded".into())));
        } else if canonical == "cancelled" {
            rec.events.push(JobEvent::event("cancelled", None));
        }
        trim_events_with_delta_tail(&mut rec.events);
        self.persist(&rec)?;
        drop(active);
        // P10: 终态计数落盘。
        self.metrics.persist(&self.data.jobs_dir());
        let _ = self.promote_queued();
        Ok(rec)
    }

    /// Promote oldest queued job(s) into running until at capacity.
    /// Returns newly promoted jobs (callers may start workers).
    pub fn promote_queued(&self) -> CoreResult<Vec<JobRecord>> {
        let mut active = self.active.lock();
        let mut promoted = Vec::new();
        loop {
            let running = active
                .values()
                .filter(|j| normalize_job_status(&j.status) == "running")
                .count();
            if running >= self.max_concurrent {
                break;
            }
            let next_id = active
                .values()
                .filter(|j| normalize_job_status(&j.status) == "queued")
                .min_by_key(|j| j.created_at)
                .map(|j| j.run_id.clone());
            let Some(id) = next_id else {
                break;
            };
            if let Some(j) = active.get_mut(&id) {
                j.status = "running".into();
                j.updated_at = Utc::now();
                j.progress_message = Some("started".into());
                j.events
                    .push(JobEvent::event("promoted from queue", None));
                trim_events_with_delta_tail(&mut j.events);
                let rec = j.clone();
                let _ = self.persist(&rec);
                promoted.push(rec);
            } else {
                break;
            }
        }
        Ok(promoted)
    }

    /// Invoke the registered hook for every currently active non-terminal job.
    /// Called once after startup wiring completes (AppState has built its worker closures).
    pub fn dispatch_recovered(&self) {
        let recs: Vec<JobRecord> = self
            .active
            .lock()
            .values()
            .filter(|j| is_active_job_status(&j.status))
            .cloned()
            .collect();
        for rec in &recs {
            (self.recover_hook)(rec);
        }
    }

    /// Register the restart-resume callback (set once during AppState construction,
    /// before `dispatch_recovered` is called). Default is a no-op.
    pub fn set_recover_hook(&mut self, hook: Arc<dyn Fn(&JobRecord) + Send + Sync>) {
        self.recover_hook = hook;
    }

    /// After process restart, re-mark a disk-orphaned active job as `queued` so it no
    /// longer counts against the running concurrency ceiling, and append a resume notice
    /// event. The caller then re-schedules the actual worker.
    pub fn rearm_interrupted(&self, run_id: &str, notice: &str) -> CoreResult<JobRecord> {
        let mut active = self.active.lock();
        let mut rec = if let Some(j) = active.get(run_id).cloned() {
            j
        } else if let Some(j) = self.load_from_disk(run_id) {
            j
        } else {
            return Err(CoreError::NotFound(format!("job {run_id}")));
        };
        if is_terminal_job_status(&rec.status) {
            active.insert(rec.run_id.clone(), rec.clone());
            return Err(CoreError::BadRequest(
                "job already terminal; nothing to resume".into(),
            ));
        }
        rec.status = "queued".into();
        rec.updated_at = Utc::now();
        rec.progress_message = Some(notice.into());
        rec.events.push(JobEvent::event(notice, None));
        trim_events_with_delta_tail(&mut rec.events);
        active.insert(rec.run_id.clone(), rec.clone());
        self.persist(&rec)?;
        Ok(rec)
    }

    pub(crate) fn load_from_disk(&self, run_id: &str) -> Option<JobRecord> {
        let path = self.data.jobs_dir().join(format!("{run_id}.json"));
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<JobRecord>(&s).ok())
            .map(|mut rec| {
                rec.status = normalize_job_status(&rec.status);
                rec
            })
    }


    /// Persist pipeline checkpoint into `payload.checkpoint` + optional cursor/progress.
    /// Used by Background (W1+) and similar multi-stage jobs for resume-after-restart.
    pub fn set_checkpoint(
        &self,
        run_id: &str,
        checkpoint: Value,
        cursor: Option<String>,
        progress: Option<f64>,
        progress_message: Option<String>,
    ) -> CoreResult<JobRecord> {
        let mut active = self.active.lock();
        let mut rec = if let Some(j) = active.get(run_id).cloned() {
            j
        } else if let Some(j) = self.load_from_disk(run_id) {
            j
        } else {
            return Err(CoreError::NotFound(format!("job {run_id}")));
        };
        let mut payload = rec.payload.unwrap_or_else(|| json!({}));
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("checkpoint".into(), checkpoint);
        } else {
            payload = json!({ "checkpoint": checkpoint });
        }
        rec.payload = Some(payload);
        if let Some(c) = cursor {
            rec.cursor = Some(c);
        }
        if let Some(p) = progress {
            rec.progress = Some(p.clamp(0.0, 1.0));
        }
        if let Some(m) = progress_message {
            rec.progress_message = Some(m);
        }
        rec.updated_at = Utc::now();
        if is_active_job_status(&rec.status) {
            active.insert(rec.run_id.clone(), rec.clone());
        }
        self.persist(&rec)?;
        Ok(rec)
    }

    /// Merge keys into existing job.payload (top-level). Creates payload object if missing.
    pub fn merge_job_payload(
        &self,
        run_id: &str,
        extra: Value,
    ) -> CoreResult<JobRecord> {
        let mut active = self.active.lock();
        let mut rec = if let Some(j) = active.get(run_id).cloned() {
            j
        } else if let Some(j) = self.load_from_disk(run_id) {
            j
        } else {
            return Err(CoreError::NotFound(format!("job {run_id}")));
        };
        let mut payload = rec.payload.unwrap_or_else(|| json!({}));
        if let Some(extra_obj) = extra.as_object() {
            if let Some(obj) = payload.as_object_mut() {
                for (k, v) in extra_obj {
                    obj.insert(k.clone(), v.clone());
                }
            } else {
                payload = extra;
            }
        }
        rec.payload = Some(payload);
        rec.updated_at = Utc::now();
        if is_active_job_status(&rec.status) {
            active.insert(rec.run_id.clone(), rec.clone());
        } else if active.contains_key(run_id) {
            active.insert(rec.run_id.clone(), rec.clone());
        }
        // Always keep disk in sync; also re-insert cancelled jobs into active only if rearm later.
        if active.contains_key(&rec.run_id) {
            active.insert(rec.run_id.clone(), rec.clone());
        }
        self.persist(&rec)?;
        Ok(rec)
    }

    /// Force a non-terminal status (e.g. resume: cancelled/failed/orphaned-running → running).
    pub fn rearm_running(&self, run_id: &str) -> CoreResult<JobRecord> {
        let mut active = self.active.lock();
        let mut rec = if let Some(j) = active.remove(run_id) {
            j
        } else if let Some(j) = self.load_from_disk(run_id) {
            j
        } else {
            return Err(CoreError::NotFound(format!("job {run_id}")));
        };
        let prior = normalize_job_status(&rec.status);
        if prior == "succeeded" {
            active.insert(rec.run_id.clone(), rec.clone());
            return Err(CoreError::BadRequest(
                "job already succeeded; nothing to resume".into(),
            ));
        }
        // If genuinely running in-memory with a live worker, caller should 409 — we still allow
        // rearm for disk-orphaned running after process restart.
        rec.status = "running".into();
        rec.updated_at = Utc::now();
        rec.error = None;
        rec.progress_message = Some("resuming".into());
        rec.events.push(JobEvent::event("job rearmed for resume", None));
        trim_events_with_delta_tail(&mut rec.events);
        active.insert(rec.run_id.clone(), rec.clone());
        self.persist(&rec)?;
        Ok(rec)
    }

    pub(crate) fn persist(&self, rec: &JobRecord) -> CoreResult<()> {
        let path = self.data.jobs_dir().join(format!("{}.json", rec.run_id));
        fs::write(path, serde_json::to_string_pretty(rec)?)?;
        Ok(())
    }

    /// P0-3(审计): 裁剪终态 job 文件——jobs 目录无界增长修复。
    /// 保留最新 `keep` 个文件（按 mtime），删除更旧的；active 内存索引不受影响。
    /// 在服务启动时调用一次（main.rs），也可按需调用。
    pub fn prune_terminal(&self, keep: usize) {
        let dir = self.data.jobs_dir();
        let Ok(entries) = fs::read_dir(&dir) else { return };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            // P10: 指标快照常驻 jobs 目录，不参与 LRU 裁剪。
            .filter(|e| e.file_name() != JobMetrics::FILE_NAME)
            .filter_map(|e| {
                let m = e.metadata().ok()?.modified().ok()?;
                Some((m, e.path()))
            })
            .collect();
        files.sort_by(|a, b| b.0.cmp(&a.0)); // 最新在前
        if files.len() > keep {
            for (_, path) in files.into_iter().skip(keep) {
                let _ = fs::remove_file(path);
            }
        }
    }
}
