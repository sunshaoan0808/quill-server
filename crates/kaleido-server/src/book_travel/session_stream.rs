//! book_travel/session_stream —— 会话打开/停止/SSE 流 + 后台 worker。（R6④a 机械迁入）
#![allow(unused_imports)]

use super::*;

pub async fn open_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let run_id = id.trim();
    if run_id.is_empty() {
        return bad_request("BT_ID", "需要 run id");
    }
    let job = match state.jobs.get(run_id) {
        None => return not_found("BT_NOT_FOUND", "任务不存在"),
        Some(j) if j.kind != "book_travel" => return bad_request("BT_KIND", "不是 book_travel 任务"),
        Some(j) if j.workspace_id != session.workspace_id && j.user_id != session.user_id => {
            return forbidden("BT_FORBIDDEN", "任务不属于当前工作区")
        }
        Some(j) => j,
    };
    let result = match job.result.clone() {
        Some(r) => r,
        None => return bad_request("BT_NO_RESULT", "任务尚无结果（未完成 pipeline）"),
    };
    // 提取 stages
    let stages = result.get("stages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let stage_result = |step: &str| -> Value {
        stages.iter().find(|s| s.get("step").and_then(|v| v.as_str()) == Some(step))
            .and_then(|s| s.get("result")).cloned().unwrap_or(json!({}))
    };
    let title = result.get("title").and_then(|v| v.as_str()).unwrap_or("漫游连载").to_string();
    let mem_raw = stage_result("summarize_memory");
    // 兼容两层：LLM 直挂 {summary/keyChoices/unresolvedConflicts} / heuristic 嵌套 {memory:{...}}
    let mem = mem_raw.get("memory").cloned().unwrap_or(mem_raw.clone());
    let summary = mem.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let key_choices: Vec<String> = mem.get("keyChoices").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let conflicts: Vec<String> = mem.get("unresolvedConflicts").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let plan = stage_result("plan_scene");
    let scene_plan = plan.get("scenePlan").cloned().unwrap_or(json!({}));
    let active_chars: Vec<String> = scene_plan.get("activeCharacters").and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default();
    let assemble = stage_result("assemble");
    let plan_text = assemble.get("plan").and_then(|v| v.as_str())
        .or_else(|| assemble.get("plan_text").and_then(|v| v.as_str())).unwrap_or("").to_string();
    let pack_id = body.get("packId").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty())
        .unwrap_or("demo-rain-alley").to_string();
    let sess_title = body.get("title").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string()).unwrap_or_else(|| format!("{title}·连载"));
    // 建 P3 穿书会话
    let req = kaleido_core::CreateSessionRequest {
        pack_id: pack_id.clone(),
        playable: kaleido_core::Playable::P3,
        play_mode: kaleido_core::PlayMode::Mainline,
        user_tier: kaleido_core::ContentTier::Standard,
        global_tier: None,
        entry: None,
        player_display_name: None,
        adult_confirmed: true,
        title: Some(sess_title),
        owner: Some(session.user_id.clone()),
        quality: kaleido_core::Quality::Lite,
        author_project_id: None,
        work_id: None,
    };
    let sess = match state.sessions_tavern.create_from_pack(&state.packs, req) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    let sid = sess.session_id.clone();
    // 原子预填初始状态
    let summary_c = summary.clone();
    let key_choices_c = key_choices.clone();
    let conflicts_c = conflicts.clone();
    let active_chars_c = active_chars.clone();
    let plan_text_c = plan_text.clone();
    let save_r = state.sessions_tavern.update_session(&sid, |existing| {
        if !summary_c.trim().is_empty() {
            existing.memory_l3.facts.push(format!("【漫游前史】{summary_c}"));
            existing.memory_l3.updated_at_turn = 0;
        }
        for kc in &key_choices_c {
            existing.milestones.push(kaleido_core::relationship_tiers::Milestone {
                id: uuid::Uuid::new_v4().to_string(),
                character: String::new(),
                label: kc.clone(),
                kind: "choice".into(),
                tier: 0,
                turn: 0,
            });
        }
        for cf in &conflicts_c {
            existing.promises.push(kaleido_core::promise::Promise::new("", "char", cf.clone(), 0));
        }
        if !active_chars_c.is_empty() {
            existing.present_character_ids = active_chars_c.clone();
        }
        // Exit beat/plan 首段 → compass.currentFocus（手写优先：建完即空，直接种）
        let focus_src = if !plan_text_c.trim().is_empty() { plan_text_c.clone() } else { String::new() };
        if !focus_src.trim().is_empty() {
            let focus: String = focus_src.chars().take(300).collect();
            existing.actor_states.mount_compass(kaleido_core::Compass::new("", focus));
        }
        Ok(())
    });
    if let Err(e) = save_r {
        return map_core_err(e);
    }
    let sess2 = state.sessions_tavern.get(&sid).unwrap_or(sess);
    Json(json!({
        "ok": true,
        "session": sess2,
        "seeded": {
            "l3Facts": if summary.trim().is_empty() { 0 } else { 1 },
            "milestones": key_choices.len(),
            "promises": conflicts.len(),
            "present": active_chars,
        },
    })).into_response()
}

pub async fn stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BookTravelStopBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let run_id = body.id.trim();
    if run_id.is_empty() {
        return bad_request("BT_ID", "需要 id/runId");
    }

    match state.jobs.get(run_id) {
        Some(j) if j.workspace_id != session.workspace_id && j.user_id != session.user_id => {
            return forbidden("BT_FORBIDDEN_SCOPE", "job not in your workspace");
        }
        Some(j) if j.kind != "book_travel" => {
            return bad_request("BT_BAD_REQUEST", "not a book_travel job");
        }
        None => {
            return not_found("BT_NOT_FOUND", "任务不存在");
        }
        _ => {}
    }

    match state.jobs.cancel(run_id) {
        Ok(j) => Json(j.to_api_json()).into_response(),
        Err(e) => map_core_err(e),
    }
}

pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<BookTravelStreamQuery>,
    Query(params): Query<HashMap<String, String>>,
) -> axum::response::Response {
    // 审计修复（IDOR）：与 jobs_stream 一致——EventSource 无法带 header，先经
    // session_from_any（Bearer 或一次性 ?ticket=，M-3）解析会话，再校验 job 归属。
    let session = match session_from_any(&state, &headers, Some(&params)) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let run_id = q.id.trim().to_string();
    if run_id.is_empty() {
        return bad_request("BT_MISSING_FIELD", "id query param is required");
    }
    match state.jobs.get(&run_id) {
        None => return not_found("BT_NOT_FOUND", "任务不存在"),
        Some(j) if j.kind != "book_travel" => return bad_request("BT_BAD_REQUEST", "not a book_travel job"),
        Some(j) if j.workspace_id != session.workspace_id && j.user_id != session.user_id => {
            return forbidden("BT_FORBIDDEN_SCOPE", "job not in your workspace")
        }
        Some(_) => stream_job_sse(state, run_id).await,
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

pub(crate) fn spawn_book_travel_worker(state: AppState, run_id: String) {
    tokio::spawn(async move {
        let jobs = state.jobs.clone();

        // 审计修复（幽灵 running job）：不再限时 30s（600×50ms）。旧实现超时后静默
        // return，而 promote_queued 只把 queued job 的状态翻成 running、不会重新 spawn
        // 执行体 → 永久 running 的幽灵任务占满一个并发槽直到重启。这里循环等待直到
        // 终态或 running；cancel 会把 status 置为 cancelled（终态），循环随即退出。
        loop {
            match jobs.get(&run_id) {
                Some(j) if is_terminal_job_status(&j.status) => return,
                Some(j) if normalize_job_status(&j.status) == "running" => break,
                Some(_) => tokio::time::sleep(StdDuration::from_millis(50)).await,
                None => return,
            }
        }

        let payload = jobs
            .get(&run_id)
            .and_then(|j| j.payload.clone())
            .unwrap_or_else(|| json!({}));
        let step = payload
            .get("step")
            .or_else(|| payload.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("assemble")
            .to_string();
        let premise = payload
            .get("text")
            .or_else(|| payload.get("premise"))
            .and_then(|v| v.as_str())
            .unwrap_or("A journey through an unwritten book.")
            .to_string();
        let title = payload
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled Book Travel")
            .to_string();
        let user_input = payload
            .get("userInput")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let context = payload.get("context").cloned().unwrap_or_else(|| json!({}));
        let prefer_heuristic = payload
            .get("preferHeuristic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let labels = payload.get("labels").cloned().unwrap_or_else(|| {
            let h = heuristic_classify(&premise, Some(&title), None);
            serde_json::to_value(h).unwrap_or_else(|_| json!({}))
        });

        if cancelled(&jobs, &run_id) {
            return;
        }

        // W2: full journey in one job
        if step == "pipeline" {
            let result = run_book_travel_pipeline(
                &state,
                &jobs,
                &run_id,
                &title,
                &premise,
                &user_input,
                &labels,
                &context,
                prefer_heuristic,
            )
            .await;
            if cancelled(&jobs, &run_id) {
                return;
            }
            if let Some(mut result) = result {
                persist_pipeline_to_works(&state, &run_id, &mut result);
                complete_if_not_cancelled(&jobs, &run_id, "succeeded", Some(result), None);
            } else if !cancelled(&jobs, &run_id) {
                complete_if_not_cancelled(
                    &jobs,
                    &run_id,
                    "failed",
                    Some(json!({
                        "kind": "book_travel",
                        "step": "pipeline",
                        "mode": "pipeline",
                        "pipeline": true,
                        "ok": false,
                        "error": "流水线中止，无结果",
                        "code": "BT_PIPELINE_EMPTY",
                        "retryable": true,
                    })),
                    Some("book travel pipeline failed".into()),
                );
            }
            return;
        }

        let _ = jobs.push_event(
            &run_id,
            JobEvent::progress(format!("{step}: generating"), 0.2),
            Some(0.2),
            Some(format!("{step}:start")),
        );
        tokio::time::sleep(StdDuration::from_millis(30)).await;
        if cancelled(&jobs, &run_id) {
            return;
        }

        // Stream-parity: try LLM first; soft-fail to templates.
        let llm_try = if prefer_heuristic {
            None
        } else {
            try_book_travel_llm_stream(
                &state,
                &jobs,
                &run_id,
                &step,
                &title,
                &premise,
                &user_input,
                &labels,
                &context,
            )
            .await
        };

        let (stage_data, result) = if let Some((sd, res)) = llm_try {
            (sd, res)
        } else {
            match step.as_str() {
                "plan_scene" => {
                    let plan =
                        template_scene_plan(&title, &premise, &user_input, &labels, &context);
                    (
                        json!({"stage":"plan_scene","scenePlan": plan}),
                        json!({
                            "kind":"book_travel",
                            "step":"plan_scene",
                            "mode":"plan_scene",
                            "title": title,
                            "scenePlan": plan,
                            "labels": labels,
                            "ok": true,
                            "mvp": true,
                            "fallback": true,
                            "generationMode": "heuristic",
                        }),
                    )
                }
                "write_change_scene" => {
                    let out = template_writer_output(
                        &title,
                        &premise,
                        &user_input,
                        "change-scene",
                        &context,
                    );
                    (
                        json!({"stage":"write_change_scene","writerOutput": out}),
                        json!({
                            "kind":"book_travel",
                            "step":"write_change_scene",
                            "mode":"write_change_scene",
                            "title": title,
                            "writerOutput": out,
                            "flow":"change-scene",
                            "ok": true,
                            "mvp": true,
                            "fallback": true,
                            "generationMode": "heuristic",
                        }),
                    )
                }
                "write_insert_beat" => {
                    let out = template_writer_output(
                        &title,
                        &premise,
                        &user_input,
                        "insert-beat",
                        &context,
                    );
                    (
                        json!({"stage":"write_insert_beat","writerOutput": out}),
                        json!({
                            "kind":"book_travel",
                            "step":"write_insert_beat",
                            "mode":"write_insert_beat",
                            "title": title,
                            "writerOutput": out,
                            "flow":"insert-beat",
                            "ok": true,
                            "mvp": true,
                            "fallback": true,
                            "generationMode": "heuristic",
                        }),
                    )
                }
                "judge_ending" => {
                    let ending = template_ending(&title, &premise, &labels, &context);
                    (
                        json!({"stage":"judge_ending","ending": ending}),
                        json!({
                            "kind":"book_travel",
                            "step":"judge_ending",
                            "mode":"judge_ending",
                            "title": title,
                            "ending": ending,
                            "labels": labels,
                            "ok": true,
                            "mvp": true,
                            "fallback": true,
                            "generationMode": "heuristic",
                        }),
                    )
                }
                "summarize_memory" => {
                    let memory = template_memory(&title, &premise, &context);
                    (
                        json!({"stage":"summarize_memory","memory": memory}),
                        json!({
                            "kind":"book_travel",
                            "step":"summarize_memory",
                            "mode":"summarize_memory",
                            "title": title,
                            "memory": memory,
                            "ok": true,
                            "mvp": true,
                            "fallback": true,
                            "generationMode": "heuristic",
                        }),
                    )
                }
                // assemble (default)
                _ => {
                    let plan_text = template_assemble_plan(&title, &premise, &labels);
                    (
                        json!({
                            "stage":"assemble",
                            "title": title,
                            "plan": plan_text,
                            "labels": labels,
                        }),
                        json!({
                            "kind":"book_travel",
                            "step":"assemble",
                            "mode":"assemble",
                            "title": title,
                            "plan": plan_text,
                            "labels": labels,
                            "ok": true,
                            "mvp": true,
                            "fallback": true,
                            "generationMode": "heuristic",
                        }),
                    )
                }
            }
        };

        let _ = jobs.push_event(
            &run_id,
            JobEvent::event(format!("{step} complete"), Some(stage_data)),
            Some(0.85),
            Some(format!("{step}:done")),
        );
        tokio::time::sleep(StdDuration::from_millis(20)).await;
        if cancelled(&jobs, &run_id) {
            return;
        }
        let mut result = result;
        if let Some(path) = persist_step_snapshot_to_works(&state, &run_id, &step, &title, &result) {
            if let Some(obj) = result.as_object_mut() {
                obj.insert("workPath".into(), json!(path));
                obj.insert("persisted".into(), json!(true));
            }
        }
        complete_if_not_cancelled(&jobs, &run_id, "succeeded", Some(result), None);
    });
}
