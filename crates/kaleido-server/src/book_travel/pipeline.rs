//! book_travel/pipeline —— LLM 流水线：分步流式调用、prompts、结果规范化、步骤执行。（R6④a 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn try_book_travel_llm_stream(
    state: &AppState,
    jobs: &kaleido_core::JobStore,
    run_id: &str,
    step: &str,
    title: &str,
    premise: &str,
    user_input: &str,
    labels: &Value,
    context: &Value,
) -> Option<(Value, Value)> {
    let llm = state.app_state.resolve_llm(
        state.llm_base.as_deref(),
        state.llm_key.as_deref(),
        &state.llm_model,
    );
    let prov_kind2 = crate::llm_stream::runtime_provider_kind(&llm, &state.provider_kind);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return None;
    }
    let model = if llm.model.is_empty() {
        state.llm_model.clone()
    } else {
        llm.model.clone()
    };

    let (system, user, max_tokens, timeout_secs, temperature) =
        book_travel_prompts(step, title, premise, user_input, labels, context);

    let mut chars: usize = 0;
    let jobs_c = jobs.clone();
    let run_c = run_id.to_string();
    let step_c = step.to_string();
    let full = match crate::llm_stream::stream_chat_completions_dispatch(
        &llm.base_url,
        &llm.api_key,
        &model,
        &prov_kind2,
        &system,
        &user,
        temperature,
        max_tokens,
        timeout_secs,
        |chunk| {
            if cancelled(&jobs_c, &run_c) {
                return false;
            }
            chars = chars.saturating_add(chunk.chars().count());
            let p = (0.25 + (chars as f64 / 5000.0).min(0.5)).min(0.8);
            let _ = jobs_c.push_event(
                &run_c,
                JobEvent {
                    event_type: "delta".into(),
                    ts: chrono::Utc::now(),
                    message: None,
                    progress: Some(p),
                    code: None,
                    data: Some(json!({"delta": chunk, "step": step_c})),
                },
                Some(p),
                Some(format!("{step_c}:delta")),
            );
            true
        },
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            if e != "cancelled" {
                let _ = jobs.push_event(
                    run_id,
                    JobEvent::event(
                        format!("llm soft-fail: {e}"),
                        Some(json!({"step": step, "error": e})),
                    ),
                    None,
                    Some(format!("{step}:llm_fail")),
                );
            }
            return None;
        }
    };

    if cancelled(jobs, run_id) {
        return None;
    }

    normalize_book_travel_llm(step, title, labels, &full)
}

pub(crate) fn book_travel_prompts(
    step: &str,
    title: &str,
    premise: &str,
    user_input: &str,
    labels: &Value,
    context: &Value,
) -> (String, String, u32, u64, f64) {
    let premise_l = crate::llm_stream::limit_text(premise, 4000);
    let user_l = crate::llm_stream::limit_text(user_input, 1500);
    let labels_s = serde_json::to_string(labels).unwrap_or_else(|_| "{}".into());
    let context_s = crate::llm_stream::limit_text(
        &serde_json::to_string(context).unwrap_or_else(|_| "{}".into()),
        2000,
    );
    match step {
        "plan_scene" => {
            let system = "你是穿书场景规划师（ScenePlanner）。根据前提、标签与上下文规划下一场景。务必返回严格纯 JSON，不要 Markdown。".to_string();
            let user = format!(
                "作品：《{title}》\n前提：{premise_l}\n用户意图：{user_l}\n标签：{labels_s}\n上下文：{context_s}\n\
返回 JSON：\n\
{{\"id\":\"uuid-or-string\",\"title\":\"场景标题\",\"summary\":\"...\",\"currentSituation\":\"...\",\"time\":\"...\",\"location\":\"...\",\"activeCharacters\":[\"...\"],\"stateChanges\":{{\"tension\":\"...\",\"information\":\"...\"}},\"divergence\":\"...\",\"storyProgress\":0-100,\"endingStatus\":null,\"sceneGoals\":[\"...\"],\"entryBeatGuidance\":\"...\",\"writerInstructions\":\"...\"}}\n仅返回纯 JSON。"
            );
            (system, user, 2048, 180, 0.6)
        }
        "write_change_scene" => {
            let system = "你是穿书场景写手（SceneWriter）。按用户意图改写当前场景，保持角色声线，动作优先，避免总结腔。返回严格纯 JSON。".to_string();
            let user = format!(
                "作品：《{title}》\n流程：change-scene\n前提：{premise_l}\n用户意图：{user_l}\n上下文：{context_s}\n\
返回 JSON：\n\
{{\"beat\":{{\"id\":\"...\",\"content\":\"场景正文（中文，多段落）\"}},\"stableMemoryPatch\":{{\"note\":\"...\"}},\"volatileMemoryPatch\":{{\"lastBeat\":\"change-scene\",\"directive\":\"...\"}}}}\n仅返回纯 JSON。"
            );
            (system, user, 4096, 240, 0.8)
        }
        "write_insert_beat" => {
            let system = "你是穿书场景写手（SceneWriter）。在当前场景缝隙插入一拍，信息量增加但不跳过冲突。返回严格纯 JSON。".to_string();
            let user = format!(
                "作品：《{title}》\n流程：insert-beat\n前提：{premise_l}\n用户意图：{user_l}\n上下文：{context_s}\n\
返回 JSON：\n\
{{\"beat\":{{\"id\":\"...\",\"content\":\"插入拍正文（中文）\"}},\"stableMemoryPatch\":{{\"note\":\"...\"}},\"volatileMemoryPatch\":{{\"lastBeat\":\"insert-beat\",\"directive\":\"...\"}}}}\n仅返回纯 JSON。"
            );
            (system, user, 3072, 220, 0.8)
        }
        "judge_ending" => {
            let system = "你是穿书结局评判官（EndingJudge）。根据标签与前提给出结局判定。返回严格纯 JSON。".to_string();
            let user = format!(
                "作品：《{title}》\n前提：{premise_l}\n标签：{labels_s}\n上下文：{context_s}\n\
返回 JSON：\n\
{{\"finalEnding\":\"...\",\"userKeyChoices\":[\"...\"],\"originalOutlineComparison\":\"...\",\"characterOutcomes\":[\"...\"],\"worldlineName\":\"...\",\"divergenceScore\":0-100}}\n仅返回纯 JSON。"
            );
            (system, user, 1536, 150, 0.5)
        }
        "summarize_memory" => {
            let system = "你是穿书记忆管家（MemoryKeeper）。压缩关键记忆与未决冲突。返回严格纯 JSON。".to_string();
            let user = format!(
                "作品：《{title}》\n前提：{premise_l}\n上下文：{context_s}\n\
返回 JSON：\n\
{{\"summary\":\"...\",\"keyChoices\":[\"...\"],\"unresolvedConflicts\":[\"...\"],\"divergenceFromOutline\":\"...\"}}\n仅返回纯 JSON。"
            );
            (system, user, 1536, 150, 0.4)
        }
        // assemble
        _ => {
            let system = "你是穿书素材装配师（MaterialAssembler）。输出可执行的 Book Travel 计划（Markdown 正文即可，也可用 JSON {\"plan\":\"...\"}）。".to_string();
            let user = format!(
                "作品：《{title}》\n前提：{premise_l}\n标签：{labels_s}\n\
请输出旅行计划，包含：前提摘要、标签、5 个 Travel Beats（Entrance/First turn/Midpoint/Crisis/Exit）。\n\
优先返回 JSON：{{\"plan\":\"markdown 正文\"}}；若无法 JSON，也可直接输出 Markdown。"
            );
            (system, user, 2048, 150, 0.5)
        }
    }
}

pub(crate) fn normalize_book_travel_llm(
    step: &str,
    title: &str,
    labels: &Value,
    full: &str,
) -> Option<(Value, Value)> {
    match step {
        "plan_scene" => {
            let plan = crate::llm_stream::extract_json_value(full)?;
            let plan = if plan.get("summary").is_some() || plan.get("sceneGoals").is_some() {
                plan
            } else {
                plan.get("scenePlan").cloned()?
            };
            Some((
                json!({"stage":"plan_scene","scenePlan": plan, "generationMode":"llm"}),
                json!({
                    "kind":"book_travel",
                    "step":"plan_scene",
                    "mode":"plan_scene",
                    "title": title,
                    "scenePlan": plan,
                    "labels": labels,
                    "ok": true,
                    "fallback": false,
                    "generationMode": "llm",
                }),
            ))
        }
        "write_change_scene" | "write_insert_beat" => {
            let flow = if step == "write_insert_beat" {
                "insert-beat"
            } else {
                "change-scene"
            };
            let out = if let Some(v) = crate::llm_stream::extract_json_value(full) {
                if v.get("beat").is_some() {
                    v
                } else if let Some(w) = v.get("writerOutput").cloned() {
                    w
                } else {
                    // wrap free text
                    json!({
                        "beat": {"id": Uuid::new_v4().to_string(), "content": full},
                        "stableMemoryPatch": {"note": format!("flow={flow} llm freeform")},
                        "volatileMemoryPatch": {"lastBeat": flow, "directive": ""}
                    })
                }
            } else {
                json!({
                    "beat": {"id": Uuid::new_v4().to_string(), "content": full},
                    "stableMemoryPatch": {"note": format!("flow={flow} llm freeform")},
                    "volatileMemoryPatch": {"lastBeat": flow, "directive": ""}
                })
            };
            Some((
                json!({"stage": step, "writerOutput": out, "generationMode":"llm"}),
                json!({
                    "kind":"book_travel",
                    "step": step,
                    "mode": step,
                    "title": title,
                    "writerOutput": out,
                    "flow": flow,
                    "ok": true,
                    "fallback": false,
                    "generationMode": "llm",
                }),
            ))
        }
        "judge_ending" => {
            let ending = crate::llm_stream::extract_json_value(full)?;
            let ending = if ending.get("finalEnding").is_some() {
                ending
            } else {
                ending.get("ending").cloned().unwrap_or(ending)
            };
            Some((
                json!({"stage":"judge_ending","ending": ending, "generationMode":"llm"}),
                json!({
                    "kind":"book_travel",
                    "step":"judge_ending",
                    "mode":"judge_ending",
                    "title": title,
                    "ending": ending,
                    "labels": labels,
                    "ok": true,
                    "fallback": false,
                    "generationMode": "llm",
                }),
            ))
        }
        "summarize_memory" => {
            let memory = crate::llm_stream::extract_json_value(full)?;
            let memory = if memory.get("summary").is_some() {
                memory
            } else {
                memory.get("memory").cloned().unwrap_or(memory)
            };
            Some((
                json!({"stage":"summarize_memory","memory": memory, "generationMode":"llm"}),
                json!({
                    "kind":"book_travel",
                    "step":"summarize_memory",
                    "mode":"summarize_memory",
                    "title": title,
                    "memory": memory,
                    "ok": true,
                    "fallback": false,
                    "generationMode": "llm",
                }),
            ))
        }
        _ => {
            // assemble: plan string
            let plan_text = if let Some(v) = crate::llm_stream::extract_json_value(full) {
                v.get("plan")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| full.trim().to_string())
            } else {
                full.trim().to_string()
            };
            if plan_text.is_empty() {
                return None;
            }
            Some((
                json!({
                    "stage":"assemble",
                    "title": title,
                    "plan": plan_text,
                    "labels": labels,
                    "generationMode":"llm",
                }),
                json!({
                    "kind":"book_travel",
                    "step":"assemble",
                    "mode":"assemble",
                    "title": title,
                    "plan": plan_text,
                    "labels": labels,
                    "ok": true,
                    "fallback": false,
                    "generationMode": "llm",
                }),
            ))
        }
    }
}


pub(crate) async fn run_one_travel_step(
    state: &AppState,
    jobs: &kaleido_core::JobStore,
    run_id: &str,
    step: &str,
    title: &str,
    premise: &str,
    user_input: &str,
    labels: &Value,
    context: &Value,
    progress_lo: f64,
    progress_hi: f64,
    prefer_heuristic: bool,
) -> Option<Value> {
    if cancelled(jobs, run_id) {
        return None;
    }
    let _ = jobs.push_event(
        run_id,
        JobEvent::progress(format!("pipeline:{step}"), progress_lo),
        Some(progress_lo),
        Some(format!("pipeline:{step}:start")),
    );

    let llm_try = if prefer_heuristic {
        None
    } else {
        try_book_travel_llm_stream(
            state, jobs, run_id, step, title, premise, user_input, labels, context,
        )
        .await
    };

    if cancelled(jobs, run_id) {
        return None;
    }

    let (stage_data, result) = if let Some((sd, res)) = llm_try {
        (sd, res)
    } else {
        match step {
            "plan_scene" => {
                let plan = template_scene_plan(title, premise, user_input, labels, context);
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
                let out = template_writer_output(title, premise, user_input, "change-scene", context);
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
                let out = template_writer_output(title, premise, user_input, "insert-beat", context);
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
                let ending = template_ending(title, premise, labels, context);
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
                let memory = template_memory(title, premise, context);
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
            _ => {
                let plan_text = template_assemble_plan(title, premise, labels);
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
        run_id,
        JobEvent::event(
            format!("{step} complete"),
            Some(json!({
                "stage": step,
                "pipeline": true,
                "result": result,
                "stageData": stage_data,
            })),
        ),
        Some(progress_hi),
        Some(format!("pipeline:{step}:done")),
    );
    Some(result)
}

pub(crate) async fn run_book_travel_pipeline(
    state: &AppState,
    jobs: &kaleido_core::JobStore,
    run_id: &str,
    title: &str,
    premise: &str,
    user_input: &str,
    labels: &Value,
    initial_context: &Value,
    prefer_heuristic: bool,
) -> Option<Value> {
    let n = PIPELINE_STEPS.len() as f64;
    let mut stages: Vec<Value> = Vec::new();
    let mut context = if initial_context.is_null() {
        json!({})
    } else {
        initial_context.clone()
    };
    let mut any_llm = false;

    for (i, step) in PIPELINE_STEPS.iter().enumerate() {
        if cancelled(jobs, run_id) {
            return None;
        }
        let lo = 0.05 + (i as f64 / n) * 0.85;
        let hi = 0.05 + ((i as f64 + 1.0) / n) * 0.85;
        let step_result = run_one_travel_step(
            state,
            jobs,
            run_id,
            step,
            title,
            premise,
            user_input,
            labels,
            &context,
            lo,
            hi.min(0.92),
            prefer_heuristic,
        )
        .await?;
        if step_result
            .get("generationMode")
            .and_then(|v| v.as_str())
            == Some("llm")
        {
            any_llm = true;
        }
        // Chain: next step sees accumulated prior results
        context = json!({
            "pipeline": true,
            "previousStep": step,
            "previousResult": step_result,
            "stagesSoFar": stages,
        });
        stages.push(json!({
            "step": step,
            "ok": step_result.get("ok").and_then(|v| v.as_bool()).unwrap_or(true),
            "generationMode": step_result.get("generationMode"),
            "result": step_result,
        }));
        let _ = jobs.push_event(
            run_id,
            JobEvent::progress(
                format!("pipeline progress {}/{}", i + 1, PIPELINE_STEPS.len()),
                hi.min(0.92),
            ),
            Some(hi.min(0.92)),
            Some(format!("pipeline:progress:{}", i + 1)),
        );
    }

    if cancelled(jobs, run_id) {
        return None;
    }

    Some(json!({
        "kind": "book_travel",
        "step": "pipeline",
        "mode": "pipeline",
        "pipeline": true,
        "title": title,
        "labels": labels,
        "stages": stages,
        "stageCount": stages.len(),
        "ok": true,
        "mvp": true,
        "fallback": !any_llm,
        "generationMode": if any_llm { "pipeline+llm" } else { "pipeline+heuristic" },
        "progress": 1.0,
    }))
}
