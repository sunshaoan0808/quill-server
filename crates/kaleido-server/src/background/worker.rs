//! background/worker —— 后台 worker：spawn + 单阶段执行 + 管线编排。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn spawn_background_worker(state: AppState, run_id: String) {
    tokio::spawn(async move {
        let jobs = state.jobs.clone();

        // Wait until promoted to running, or cancelled while queued.
        for _ in 0..600 {
            match jobs.get(&run_id) {
                Some(j) if is_terminal_job_status(&j.status) => return,
                Some(j) if normalize_job_status(&j.status) == "running" => break,
                Some(_) => tokio::time::sleep(StdDuration::from_millis(50)).await,
                None => return,
            }
        }
        if jobs
            .get(&run_id)
            .map(|j| normalize_job_status(&j.status) != "running")
            .unwrap_or(true)
        {
            return;
        }

        let payload = jobs
            .get(&run_id)
            .and_then(|j| j.payload.clone())
            .unwrap_or_else(|| json!({}));
        let mode = payload
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("stage_one")
            .to_string();
        let premise = payload
            .get("text")
            .or_else(|| payload.get("premise"))
            .and_then(|v| v.as_str())
            .unwrap_or("An original story world.")
            .to_string();
        let title = payload
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled Background")
            .to_string();
        let character_name = payload
            .get("characterName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let world_book_context = payload
            .get("worldBookContext")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let include_names = payload
            .get("includeCharacterNames")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        if cancelled(&jobs, &run_id) {
            return;
        }

        let _ = jobs.push_event(
            &run_id,
            JobEvent::progress(format!("{mode}: generating"), 0.15),
            Some(0.15),
            Some(format!("{mode}:start")),
        );
        tokio::time::sleep(StdDuration::from_millis(30)).await;
        if cancelled(&jobs, &run_id) {
            return;
        }

        // Full pipeline: stage_one → items → deepen characters (checkpointed, W1+)
        if mode == "pipeline" {
            let prefer_heuristic = payload
                .get("preferHeuristic")
                .and_then(|v| v.as_bool())
                .or_else(|| {
                    payload
                        .get("checkpoint")
                        .and_then(|c| c.get("preferHeuristic"))
                        .and_then(|v| v.as_bool())
                })
                .unwrap_or(false);
            let deepen_max = payload
                .get("deepenMax")
                .and_then(|v| v.as_u64())
                .or_else(|| {
                    payload
                        .get("checkpoint")
                        .and_then(|c| c.get("deepenMax"))
                        .and_then(|v| v.as_u64())
                })
                .unwrap_or(5) as usize;
            let deepen_mode = payload
                .get("deepenMode")
                .and_then(|v| v.as_str())
                .unwrap_or("all")
                .to_string();
            let existing_cp = payload.get("checkpoint").cloned();
            let result = run_pipeline(
                &state,
                &jobs,
                &run_id,
                &title,
                &premise,
                include_names,
                prefer_heuristic,
                deepen_max,
                &deepen_mode,
                existing_cp,
            )
            .await;
            if cancelled(&jobs, &run_id) {
                return;
            }
            if let Some(result) = result {
                let _ = jobs.complete(&run_id, "succeeded", Some(result), None);
            } else if !cancelled(&jobs, &run_id) {
                let _ = jobs.complete(
                    &run_id,
                    "failed",
                    Some(json!({
                        "kind": "background",
                        "stage": "pipeline",
                        "ok": false,
                        "error": "pipeline aborted without result",
                    })),
                    Some("pipeline failed".into()),
                );
            }
            return;
        }

        // Stream-parity: prefer upstream-quality LLM stream; soft-fail to heuristic.
        let prefer_heuristic = payload
            .get("preferHeuristic")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let llm_try = if prefer_heuristic {
            None
        } else {
            try_background_llm_stream(
                &state,
                &jobs,
                &run_id,
                &mode,
                &title,
                &premise,
                &character_name,
                &world_book_context,
                include_names,
            )
            .await
        };
        let result = if let Some(v) = llm_try {
            let _ = jobs.push_event(
                &run_id,
                JobEvent::event(
                    "llm stage complete",
                    Some(json!({"stage": mode, "generationMode": "llm"})),
                ),
                Some(0.85),
                Some(format!("{mode}:llm_done")),
            );
            v
        } else {
            match mode.as_str() {
                "items" => {
                    let data = template_items(&title, &premise);
                    let _ = jobs.push_event(
                        &run_id,
                        JobEvent::event(
                            "items complete",
                            Some(json!({
                                "stage": "items",
                                "worldBooks": data.get("worldBooks"),
                                "characterCards": data.get("characterCards"),
                            })),
                        ),
                        Some(0.85),
                        Some("items:done".into()),
                    );
                    json!({
                        "kind": "background",
                        "schemaVersion": 1,
                        "stage": "items",
                        "mode": "items",
                        "title": title,
                        "worldBooks": data.get("worldBooks"),
                        "characterCards": data.get("characterCards"),
                        "ok": true,
                        "mvp": true,
                        "fallback": true,
                        "generationMode": "heuristic",
                    })
                }
                "character_card" => {
                    let card =
                        template_character_card(&character_name, &premise, &world_book_context);
                    let _ = jobs.push_event(
                        &run_id,
                        JobEvent::event(
                            "character_card complete",
                            Some(json!({
                                "stage": "character_card",
                                "characterCard": card,
                            })),
                        ),
                        Some(0.85),
                        Some("character_card:done".into()),
                    );
                    json!({
                        "kind": "background",
                        "stage": "character_card",
                        "mode": "character_card",
                        "title": title,
                        "characterName": character_name,
                        "characterCard": card,
                        "ok": true,
                        "mvp": true,
                        "fallback": true,
                        "generationMode": "heuristic",
                    })
                }
                // stage_one (default)
                _ => {
                    let data = template_stage_one(&title, &premise, include_names);
                    let _ = jobs.push_event(
                        &run_id,
                        JobEvent::event(
                            "stage_one complete",
                            Some(json!({
                                "stage": "stage_one",
                                "title": title,
                                "worldBooks": data.get("worldBooks"),
                                "characterNames": data.get("characterNames"),
                            })),
                        ),
                        Some(0.85),
                        Some("stage_one:done".into()),
                    );
                    json!({
                        "kind": "background",
                        "stage": "stage_one",
                        "mode": "stage_one",
                        "title": title,
                        "worldBooks": data.get("worldBooks"),
                        "characterNames": data.get("characterNames"),
                        "ok": true,
                        "mvp": true,
                        "fallback": true,
                        "generationMode": "heuristic",
                    })
                }
            }
        };

        tokio::time::sleep(StdDuration::from_millis(20)).await;
        if cancelled(&jobs, &run_id) {
            return;
        }
        let _ = jobs.complete(&run_id, "succeeded", Some(result), None);
    });
}



/// Run one logical stage; returns structured result value or None if cancelled/hard-fail.
pub(crate) async fn run_one_stage(
    state: &AppState,
    jobs: &kaleido_core::JobStore,
    run_id: &str,
    mode: &str,
    title: &str,
    premise: &str,
    character_name: &str,
    world_book_context: &str,
    include_names: bool,
    prefer_heuristic: bool,
    progress_lo: f64,
    progress_hi: f64,
) -> Option<Value> {
    if cancelled(jobs, run_id) {
        return None;
    }
    let _ = jobs.push_event(
        run_id,
        JobEvent::progress(format!("pipeline:{mode}"), progress_lo),
        Some(progress_lo),
        Some(format!("pipeline:{mode}:start")),
    );

    // Yield so stop/resume smoke can observe mid-pipeline checkpoints (heuristic is otherwise instant).
    if prefer_heuristic {
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        if cancelled(jobs, run_id) {
            return None;
        }
    }

    let llm_try = if prefer_heuristic {
        None
    } else {
        try_background_llm_stream(
            state,
            jobs,
            run_id,
            mode,
            title,
            premise,
            character_name,
            world_book_context,
            include_names,
        )
        .await
    };

    if cancelled(jobs, run_id) {
        return None;
    }

    let result = if let Some(v) = llm_try {
        v
    } else {
        match mode {
            "items" => {
                let data = template_items(title, premise);
                json!({
                    "kind": "background",
                    "stage": "items",
                    "mode": "items",
                    "title": title,
                    "worldBooks": data.get("worldBooks"),
                    "characterCards": data.get("characterCards"),
                    "ok": true,
                    "mvp": true,
                    "fallback": true,
                    "generationMode": "heuristic",
                })
            }
            "character_card" => {
                let card = template_character_card(character_name, premise, world_book_context);
                json!({
                    "kind": "background",
                    "stage": "character_card",
                    "mode": "character_card",
                    "title": title,
                    "characterName": character_name,
                    "characterCard": card,
                    "ok": true,
                    "mvp": true,
                    "fallback": true,
                    "generationMode": "heuristic",
                })
            }
            _ => {
                let data = template_stage_one(title, premise, include_names);
                json!({
                    "kind": "background",
                    "stage": "stage_one",
                    "mode": "stage_one",
                    "title": title,
                    "worldBooks": data.get("worldBooks"),
                    "characterNames": data.get("characterNames"),
                    "ok": true,
                    "mvp": true,
                    "fallback": true,
                    "generationMode": "heuristic",
                })
            }
        }
    };

    let _ = jobs.push_event(
        run_id,
        JobEvent::event(
            format!("{mode} complete"),
            Some(json!({
                "stage": mode,
                "pipeline": true,
                "result": result,
            })),
        ),
        Some(progress_hi),
        Some(format!("pipeline:{mode}:done")),
    );
    Some(result)
}

pub(crate) async fn run_pipeline(
    state: &AppState,
    jobs: &kaleido_core::JobStore,
    run_id: &str,
    title: &str,
    premise: &str,
    include_names: bool,
    prefer_heuristic: bool,
    deepen_max: usize,
    deepen_mode: &str,
    existing_cp: Option<Value>,
) -> Option<Value> {
    let mut completed: Vec<String> = existing_cp
        .as_ref()
        .and_then(|c| c.get("completed"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut stage_one = existing_cp
        .as_ref()
        .and_then(|c| c.get("stageOne"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut items = existing_cp
        .as_ref()
        .and_then(|c| c.get("items"))
        .cloned()
        .unwrap_or(Value::Null);
    let mut deepened_cards: Vec<Value> = existing_cp
        .as_ref()
        .and_then(|c| c.get("deepenedCards"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut deepened_names: Vec<String> = existing_cp
        .as_ref()
        .and_then(|c| c.get("deepenedNames"))
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let persist_cp = |jobs: &kaleido_core::JobStore,
                      run_id: &str,
                      completed: &[String],
                      stage_one: &Value,
                      items: &Value,
                      deepened_cards: &[Value],
                      deepened_names: &[String],
                      next: &str,
                      progress: f64| {
        let cp = json!({
            "schemaVersion": 1,
            "completed": completed,
            "next": next,
            "stageOne": stage_one,
            "items": items,
            "deepenedCards": deepened_cards,
            "deepenedNames": deepened_names,
            "title": title,
            "premise": premise,
        });
        let _ = jobs.set_checkpoint(
            run_id,
            cp,
            Some(next.to_string()),
            Some(progress),
            Some(format!("checkpoint:{next}")),
        );
        let _ = jobs.push_event(
            run_id,
            JobEvent::event(
                format!("checkpoint:{next}"),
                Some(json!({"next": next, "completed": completed})),
            ),
            Some(progress),
            Some(format!("checkpoint:{next}")),
        );
    };

    // Stage 1
    if !completed.iter().any(|s| s == "stage_one") {
        stage_one = run_one_stage(
            state,
            jobs,
            run_id,
            "stage_one",
            title,
            premise,
            "",
            "",
            include_names,
            prefer_heuristic,
            0.08,
            0.32,
        )
        .await?;
        if cancelled(jobs, run_id) {
            return None;
        }
        completed.push("stage_one".into());
        persist_cp(
            jobs,
            run_id,
            &completed,
            &stage_one,
            &items,
            &deepened_cards,
            &deepened_names,
            "items",
            0.35,
        );
    }

    // Stage 2 items
    if !completed.iter().any(|s| s == "items") {
        let mut items_premise = premise.to_string();
        if let Some(names) = stage_one.get("characterNames").and_then(|v| v.as_array()) {
            let list: Vec<&str> = names.iter().filter_map(|x| x.as_str()).collect();
            if !list.is_empty() {
                items_premise.push_str("\n\n角色名候选：");
                items_premise.push_str(&list.join("、"));
            }
        }
        if let Some(wbs) = stage_one.get("worldBooks").and_then(|v| v.as_array()) {
            if let Some(name) = wbs
                .first()
                .and_then(|w| w.get("name"))
                .and_then(|n| n.as_str())
            {
                items_premise.push_str("\n世界书草案：");
                items_premise.push_str(name);
            }
        }
        items = run_one_stage(
            state,
            jobs,
            run_id,
            "items",
            title,
            &items_premise,
            "",
            "",
            include_names,
            prefer_heuristic,
            0.40,
            0.62,
        )
        .await?;
        if cancelled(jobs, run_id) {
            return None;
        }
        completed.push("items".into());
        persist_cp(
            jobs,
            run_id,
            &completed,
            &stage_one,
            &items,
            &deepened_cards,
            &deepened_names,
            "character_cards",
            0.65,
        );
    }

    // Stage 3: deepen characters (multi, not first-only)
    let deepen_mode_l = deepen_mode.trim().to_ascii_lowercase();
    let do_deepen = deepen_mode_l != "none" && deepen_max > 0;
    if do_deepen && !completed.iter().any(|s| s == "character_cards") {
        let mut names: Vec<String> = Vec::new();
        if let Some(arr) = items.get("characterCards").and_then(|v| v.as_array()) {
            for c in arr {
                if let Some(n) = c.get("name").and_then(|x| x.as_str()) {
                    let n = n.trim();
                    if !n.is_empty() && !names.iter().any(|x| x == n) {
                        names.push(n.to_string());
                    }
                }
            }
        }
        if let Some(arr) = stage_one.get("characterNames").and_then(|v| v.as_array()) {
            for n in arr {
                if let Some(n) = n.as_str() {
                    let n = n.trim();
                    if !n.is_empty() && !names.iter().any(|x| x == n) {
                        names.push(n.to_string());
                    }
                }
            }
        }
        if deepen_mode_l == "first" {
            names.truncate(1);
        } else {
            names.truncate(deepen_max);
        }

        let wb_ctx = world_book_context_summary(
            items
                .get("worldBooks")
                .or_else(|| stage_one.get("worldBooks"))
                .unwrap_or(&json!([])),
            premise,
        );

        let n_total = names.len().max(1) as f64;
        for (i, name) in names.iter().enumerate() {
            if deepened_names.iter().any(|d| d == name) {
                continue;
            }
            if cancelled(jobs, run_id) {
                return None;
            }
            let lo = 0.68 + (i as f64 / n_total) * 0.22;
            let hi = 0.68 + ((i as f64 + 1.0) / n_total) * 0.22;
            if let Some(card_res) = run_one_stage(
                state,
                jobs,
                run_id,
                "character_card",
                title,
                premise,
                name,
                &wb_ctx,
                false,
                prefer_heuristic,
                lo,
                hi,
            )
            .await
            {
                let card = card_res
                    .get("characterCard")
                    .cloned()
                    .unwrap_or(card_res);
                deepened_cards.push(card);
                deepened_names.push(name.clone());
                persist_cp(
                    jobs,
                    run_id,
                    &completed,
                    &stage_one,
                    &items,
                    &deepened_cards,
                    &deepened_names,
                    "character_cards",
                    hi,
                );
            }
        }
        completed.push("character_cards".into());
        persist_cp(
            jobs,
            run_id,
            &completed,
            &stage_one,
            &items,
            &deepened_cards,
            &deepened_names,
            "done",
            0.94,
        );
    } else if !do_deepen && !completed.iter().any(|s| s == "character_cards") {
        completed.push("character_cards".into());
        persist_cp(
            jobs,
            run_id,
            &completed,
            &stage_one,
            &items,
            &deepened_cards,
            &deepened_names,
            "done",
            0.94,
        );
    }

    if cancelled(jobs, run_id) {
        return None;
    }

    let world_books = items
        .get("worldBooks")
        .cloned()
        .or_else(|| stage_one.get("worldBooks").cloned())
        .unwrap_or(json!([]));
    let mut character_cards = items
        .get("characterCards")
        .cloned()
        .unwrap_or(json!([]));
    if let Some(arr) = character_cards.as_array_mut() {
        for card in &deepened_cards {
            let name = card
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some(pos) = arr
                .iter()
                .position(|c| c.get("name").and_then(|n| n.as_str()) == Some(name))
            {
                arr[pos] = card.clone();
            } else {
                arr.push(card.clone());
            }
        }
    }

    let gen_mode = if prefer_heuristic {
        "heuristic"
    } else {
        "pipeline"
    };

    // Final checkpoint: complete
    let final_cp = json!({
        "schemaVersion": 1,
        "completed": ["stage_one", "items", "character_cards", "done"],
        "next": null,
        "stageOne": stage_one,
        "items": items,
        "deepenedCards": deepened_cards,
        "deepenedNames": deepened_names,
    });
    let _ = jobs.set_checkpoint(
        run_id,
        final_cp.clone(),
        Some("done".into()),
        Some(1.0),
        Some("pipeline complete".into()),
    );

    Some(json!({
        "kind": "background",
        "schemaVersion": 1,
        "stage": "pipeline",
        "mode": "pipeline",
        "title": title,
        "ok": true,
        "pipeline": true,
        "resumable": false,
        "checkpoint": final_cp,
        "stages": {
            "stage_one": stage_one,
            "items": items,
            "character_cards": deepened_cards,
        },
        "worldBooks": world_books,
        "characterCards": character_cards,
        "characterNames": stage_one.get("characterNames"),
        "deepenedCount": deepened_names.len(),
        "deepenedNames": deepened_names,
        "deepenMax": deepen_max,
        "deepenMode": deepen_mode,
        "generationMode": gen_mode,
        "preferHeuristic": prefer_heuristic,
    }))
}
