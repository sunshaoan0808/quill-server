//! background/runs —— 运行 API：start/start_stage/stop/stream/get_run/resume。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn start_inner(
    state: AppState,
    session: kaleido_core::SessionRecord,
    body: BackgroundStartBody,
    stage_override: Option<String>,
) -> axum::response::Response {
    let mode = stage_override
        .or_else(|| body.mode.clone())
        .as_deref()
        .map(normalize_stage)
        .unwrap_or_else(|| Some("stage_one".into()));
    let Some(mode) = mode else {
        return bad_request("BG_BAD_REQUEST", format!("mode must be one of: {}", STAGES.join(", ")));
    };

    if mode == "character_card" {
        let name = body
            .character_name
            .as_deref()
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            return bad_request("BG_MISSING_FIELD", "characterName is required for character_card stage");
        }
    }

    let text = body
        .text
        .clone()
        .or_else(|| body.premise.clone())
        .unwrap_or_default();
    if text.trim().is_empty() {
        return bad_request("BG_MISSING_FIELD", "premise or text is required");
    }
    let include_names = body.include_character_names.unwrap_or(true);

    let mut payload = body.payload.unwrap_or_else(|| json!({}));
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("mode".into(), json!(mode));
        obj.insert("stage".into(), json!(mode));
        if !text.is_empty() {
            obj.insert("premise".into(), json!(text.clone()));
            obj.insert("text".into(), json!(text.clone()));
        }
        if let Some(t) = body.title.clone() {
            obj.insert("title".into(), json!(t));
        }
        if let Some(n) = body.character_name.clone() {
            obj.insert("characterName".into(), json!(n));
        }
        if let Some(w) = body.world_book_context.clone() {
            obj.insert("worldBookContext".into(), json!(w));
        }
        obj.insert("includeCharacterNames".into(), json!(include_names));
        obj.insert("feature".into(), json!("background"));
        if let Some(h) = body.prefer_heuristic {
            obj.insert("preferHeuristic".into(), json!(h));
        }
        let deepen_mode = body
            .deepen_mode
            .as_deref()
            .unwrap_or("all")
            .trim()
            .to_ascii_lowercase();
        obj.insert("deepenMode".into(), json!(deepen_mode));
        let deepen_max = body.deepen_max.unwrap_or(5);
        obj.insert("deepenMax".into(), json!(deepen_max));
        // Fresh start clears any accidental checkpoint in extra payload
        obj.remove("checkpoint");
        obj.insert("resume".into(), json!(false));
    }

    let meta = json!({
        "feature": "background",
        "mode": mode,
        "stage": mode,
        "title": body.title,
        "characterName": body.character_name,
    });

    let job = match state.jobs.create(
        "background",
        &session.user_id,
        &session.workspace_id,
        payload,
        body.model.or_else(|| Some(state.llm_model.clone())),
        Some(meta),
    ) {
        Ok(j) => j,
        Err(e) => return map_core_err(e),
    };

    spawn_background_worker(state.clone(), job.run_id.clone());

    (
        StatusCode::CREATED,
        Json(json!({
            "id": job.run_id,
            "runId": job.run_id,
            "kind": job.kind,
            "stage": mode,
            "status": normalize_job_status(&job.status),
            "stream": format!("/api/v1/background/stream?id={}", job.run_id),
            "jobsStream": format!("/api/v1/jobs/{}/stream", job.run_id),
            "progress": job.progress,
            "progressMessage": job.progress_message,
            "payload": job.payload,
        })),
    )
        .into_response()
}

/// `POST /api/v1/background/start`
pub async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BackgroundStartBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    start_inner(state, session, body, None)
}

/// `POST /api/v1/background/{stage}` — S5-W2 T2 stage router.
pub async fn start_stage(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(stage): Path<String>,
    Json(body): Json<BackgroundStartBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(norm) = normalize_stage(&stage) else {
        return bad_request("BG_BAD_REQUEST", format!("unknown stage '{stage}'; expected: {}", STAGES.join(", ")));
    };
    start_inner(state, session, body, Some(norm))
}

/// `POST /api/v1/background/stop`
pub async fn stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BackgroundStopBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let run_id = body.id.trim();
    if run_id.is_empty() {
        return bad_request("BG_MISSING_FIELD", "id is required");
    }

    match state.jobs.get(run_id) {
        Some(j) if j.workspace_id != session.workspace_id && j.user_id != session.user_id => {
            return forbidden("BG_FORBIDDEN_SCOPE", "job not in your workspace");
        }
        Some(j) if j.kind != "background" => {
            return bad_request("BG_BAD_REQUEST", "not a background job");
        }
        None => {
            return not_found("BG_NOT_FOUND", "job not found");
        }
        _ => {}
    }

    match state.jobs.cancel(run_id) {
        Ok(j) => Json(j.to_api_json()).into_response(),
        Err(e) => map_core_err(e),
    }
}

/// `GET /api/v1/background/stream?id=...`
pub async fn stream(
    State(state): State<AppState>,
    Query(q): Query<BackgroundStreamQuery>,
) -> axum::response::Response {
    let run_id = q.id.trim().to_string();
    if run_id.is_empty() {
        return bad_request("BG_MISSING_FIELD", "id query param is required");
    }
    match state.jobs.get(&run_id) {
        None => return not_found("BG_NOT_FOUND", "job not found"),
        Some(j) if j.kind != "background" => return bad_request("BG_BAD_REQUEST", "not a background job"),
        Some(_) => stream_job_sse(state, run_id).await,
    }
}

/// `GET /api/v1/background/runs/{id}` — job + checkpoint summary (W1+).
pub async fn get_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let run_id = run_id.trim();
    let Some(j) = state.jobs.get(run_id) else {
        return not_found("BG_NOT_FOUND", "job not found");
    };
    if j.workspace_id != session.workspace_id && j.user_id != session.user_id {
        return forbidden("BG_FORBIDDEN_SCOPE", "job not in your workspace");
    }
    if j.kind != "background" {
        return bad_request("BG_WRONG_KIND", "not a background job");
    }
    let checkpoint = j
        .payload
        .as_ref()
        .and_then(|p| p.get("checkpoint"))
        .cloned();
    let resumable = {
        let st = normalize_job_status(&j.status);
        st != "succeeded"
            && checkpoint
                .as_ref()
                .and_then(|c| c.get("completed"))
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false)
    };
    Json(json!({
        "ok": true,
        "schemaVersion": 1,
        "id": j.run_id,
        "runId": j.run_id,
        "kind": j.kind,
        "status": normalize_job_status(&j.status),
        "progress": j.progress,
        "progressMessage": j.progress_message,
        "cursor": j.cursor,
        "checkpoint": checkpoint,
        "resumable": resumable,
        "result": j.result,
        "error": j.error,
        "payload": j.payload,
        "stream": format!("/api/v1/background/stream?id={}", j.run_id),
        "createdAt": j.created_at,
        "updatedAt": j.updated_at,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundResumeBody {
    /// Optional override deepenMax on resume.
    #[serde(default)]
    pub deepen_max: Option<usize>,
    #[serde(default)]
    pub prefer_heuristic: Option<bool>,
}

/// `POST /api/v1/background/runs/{id}/resume` — continue pipeline from checkpoint (W1+).
pub async fn resume(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<String>,
    body: Option<Json<BackgroundResumeBody>>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let body = body.map(|j| j.0).unwrap_or(BackgroundResumeBody {
        deepen_max: None,
        prefer_heuristic: None,
    });
    let run_id = run_id.trim().to_string();
    let Some(j) = state.jobs.get(&run_id) else {
        return not_found("BG_NOT_FOUND", "job not found");
    };
    if j.workspace_id != session.workspace_id && j.user_id != session.user_id {
        return forbidden("BG_FORBIDDEN_SCOPE", "job not in your workspace");
    }
    if j.kind != "background" {
        return bad_request("BG_WRONG_KIND", "not a background job");
    }
    let st = normalize_job_status(&j.status);
    if st == "succeeded" {
        return err_with_code(
            StatusCode::BAD_REQUEST,
            "BG_ALREADY_DONE", "job already succeeded",
            serde_json::json!({"runId": run_id,
                "result": j.result}),
        );
    }
    // Refuse live running job (worker still attached). Smoke should stop first.
    // After process restart, disk-orphaned "running" is rearm-able.
    if st == "running" {
        // If progress_message is "resuming" just after rearm, allow; otherwise 409.
        let msg = j.progress_message.as_deref().unwrap_or("");
        if msg != "resuming" {
            return err_with_code(
            StatusCode::CONFLICT,
            "BG_STILL_RUNNING", "job still running; stop first or wait",
            serde_json::json!({"runId": run_id,
                    "hint": "POST /api/v1/background/stop {\"id\": runId} then resume"}),
        );
        }
    }

    let mode = j
        .payload
        .as_ref()
        .and_then(|p| p.get("mode"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let has_cp = j
        .payload
        .as_ref()
        .and_then(|p| p.get("checkpoint"))
        .is_some();
    if mode != "pipeline" && !has_cp {
        return err_with_code(
            StatusCode::BAD_REQUEST,
            "BG_NO_CHECKPOINT", "no checkpoint to resume (only pipeline jobs checkpoint)",
            serde_json::json!({"runId": run_id}),
        );
    }

    let mut extra = json!({
        "resume": true,
        "mode": "pipeline",
        "stage": "pipeline",
    });
    if let Some(obj) = extra.as_object_mut() {
        if let Some(h) = body.prefer_heuristic {
            obj.insert("preferHeuristic".into(), json!(h));
        }
        if let Some(d) = body.deepen_max {
            obj.insert("deepenMax".into(), json!(d));
        }
    }
    if let Err(e) = state.jobs.merge_job_payload(&run_id, extra) {
        return map_core_err(e);
    }
    // Touch checkpoint progress_message for observability
    if let Some(cp) = state
        .jobs
        .get(&run_id)
        .and_then(|jj| jj.payload.as_ref().and_then(|p| p.get("checkpoint").cloned()))
    {
        let _ = state.jobs.set_checkpoint(
            &run_id,
            cp,
            Some("resume".into()),
            None,
            Some("resume requested".into()),
        );
    }

    match state.jobs.rearm_running(&run_id) {
        Ok(j3) => {
            spawn_background_worker(state.clone(), run_id.clone());
            (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "resumed": true,
                    "id": j3.run_id,
                    "runId": j3.run_id,
                    "status": normalize_job_status(&j3.status),
                    "checkpoint": j3.payload.as_ref().and_then(|p| p.get("checkpoint")),
                    "stream": format!("/api/v1/background/stream?id={}", j3.run_id),
                    "schemaVersion": 1,
                })),
            )
                .into_response()
        }
        Err(e) => map_core_err(e),
    }
}
