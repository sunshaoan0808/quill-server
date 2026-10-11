//! book_travel/runs —— 运行记录 API：start/start_pipeline/start_step/list_runs/get_run/start_inner。（R6④a 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn start_inner(
    state: AppState,
    session: kaleido_core::SessionRecord,
    body: BookTravelStartBody,
    step_override: Option<String>,
) -> axum::response::Response {
    let raw = step_override
        .or_else(|| body.step.clone())
        .or_else(|| body.mode.clone())
        .unwrap_or_else(|| "assemble".into());
    let Some(step) = normalize_step(&raw) else {
        return bad_request("BT_BAD_REQUEST", format!("step/mode must be one of: {}", STEPS.join(", ")));
    };

    let text = body
        .text
        .clone()
        .or_else(|| body.premise.clone())
        .unwrap_or_default();
    let user_input = body.user_input.clone().unwrap_or_default();
    if text.trim().is_empty() {
        return bad_request("BT_BAD_REQUEST", "text/premise is required");
    }

    // Prefer explicit context; else inject previous job result when previousRunId set.
    let mut context = body.context.clone();
    if context_is_empty(&context) {
        if let Some(prev) = body
            .previous_run_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match load_previous_result(&state, &session, prev) {
                Ok(r) => context = Some(r),
                Err(resp) => return resp,
            }
        }
    }

    let mut payload = body.payload.unwrap_or_else(|| json!({}));
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("mode".into(), json!(step));
        obj.insert("step".into(), json!(step));
        if !text.is_empty() {
            obj.insert("premise".into(), json!(text.clone()));
            obj.insert("text".into(), json!(text.clone()));
        }
        if let Some(t) = body.title.clone() {
            obj.insert("title".into(), json!(t));
        }
        if !user_input.is_empty() {
            obj.insert("userInput".into(), json!(user_input));
        }
        if let Some(labels) = body.labels.clone() {
            obj.insert("labels".into(), labels);
        }
        if let Some(ctx) = context.clone() {
            obj.insert("context".into(), ctx);
        }
        if let Some(prev) = body.previous_run_id.clone() {
            obj.insert("previousRunId".into(), json!(prev));
        }
        if body.prefer_heuristic.unwrap_or(false) {
            obj.insert("preferHeuristic".into(), json!(true));
        }
        obj.insert("feature".into(), json!("book_travel"));
    }

    let meta = json!({
        "feature": "book_travel",
        "mode": step,
        "step": step,
        "title": body.title,
    });

    let job = match state.jobs.create(
        "book_travel",
        &session.user_id,
        &session.workspace_id,
        payload,
        body.model.or_else(|| Some(state.llm_model.clone())),
        Some(meta),
    ) {
        Ok(j) => j,
        Err(e) => return map_core_err(e),
    };

    spawn_book_travel_worker(state.clone(), job.run_id.clone());

    let pipeline = step == "pipeline";
    (
        StatusCode::CREATED,
        Json(json!({
            "id": job.run_id,
            "runId": job.run_id,
            "kind": job.kind,
            "step": step,
            "mode": step,
            "pipeline": pipeline,
            "status": normalize_job_status(&job.status),
            "stream": format!("/api/v1/book-travel/stream?id={}", job.run_id),
            "jobsStream": format!("/api/v1/jobs/{}/stream", job.run_id),
            "run": format!("/api/v1/book-travel/runs/{}", job.run_id),
            "progress": job.progress,
            "progressMessage": job.progress_message,
            "payload": job.payload,
            "retryable": true,
        })),
    )
        .into_response()
}


/// POST /api/v1/book-travel/pipeline — alias for start with mode=pipeline.
pub async fn start_pipeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut body): Json<BookTravelStartBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    body.mode = Some("pipeline".into());
    body.step = Some("pipeline".into());
    start_inner(state, session, body, Some("pipeline".into()))
}

/// POST /api/v1/book-travel/start
pub async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BookTravelStartBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    start_inner(state, session, body, None)
}

/// POST /api/v1/book-travel/{step}
pub async fn start_step(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(step): Path<String>,
    Json(body): Json<BookTravelStartBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let Some(norm) = normalize_step(&step) else {
        return bad_request("BT_BAD_REQUEST", format!("unknown step '{step}'; expected: {}", STEPS.join(", ")));
    };
    start_inner(state, session, body, Some(norm))
}


/// GET /api/v1/book-travel/runs — workspace-scoped book_travel jobs (newest first).
pub async fn list_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<BookTravelRunsQuery>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let limit = q.limit.unwrap_or(30).clamp(1, 100) as usize;
    let filter = kaleido_core::JobListFilter {
        kind: Some("book_travel".into()),
        status: q.status.clone(),
        user_id: None,
        workspace_id: Some(session.workspace_id.clone()),
        limit,
    };
    match state.jobs.list(filter) {
        Ok(jobs) => {
            let items: Vec<Value> = jobs.iter().map(|j| j.to_api_json()).collect();
            Json(json!({
                "ok": true,
                "items": items,
                "count": items.len(),
                "workspaceId": session.workspace_id,
            }))
            .into_response()
        }
        Err(e) => map_core_err(e),
    }
}

/// GET /api/v1/book-travel/runs/{id}
pub async fn get_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let run_id = id.trim();
    if run_id.is_empty() {
        return bad_request("BT_ID", "需要 run id");
    }
    match state.jobs.get(run_id) {
        None => return err_with_code(
            StatusCode::NOT_FOUND,
            "BT_NOT_FOUND", "任务不存在",
            serde_json::json!({"runId": run_id})),
        Some(j) if j.kind != "book_travel" => return bad_request("BT_KIND", "不是 book_travel 任务"),
        Some(j) if j.workspace_id != session.workspace_id && j.user_id != session.user_id => return forbidden("BT_FORBIDDEN", "任务不属于当前工作区"),
        Some(j) => Json(j.to_api_json()).into_response(),
    }
}
