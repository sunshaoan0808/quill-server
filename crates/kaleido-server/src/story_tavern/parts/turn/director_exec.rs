//! turn/director_exec — [2026-10-06 二期②] 导演计划执行链合流：LLM 生成
//! （generate_director_plan_llm + 因果模拟）+ 计划执行（run_director_plan，
//! 兼 axum handler）+ 计划应用（apply_director_plan_update）。生成/应用状态机
//! 归位一处；p007 保留路由与配置面，post_parse 保留解析消费点。
#![allow(unused_imports)]

use super::super::*;
/// X2c (吞噬自 xiami story_simulation.rs): 因果推演校验 LLM 调用。
/// system prompt 用 `st_simulation::SYSTEM_PROMPT`，user prompt = 最近剧情 + 导演计划 goal/pressure。
/// 解析 JSON 为 `ChapterSimulation`（serde camelCase + vec_or_single 兼容）→ `validate_simulation`
/// 校验 → 通过后追加到导演计划（失败/LLM 出错仅 warn，降级为无推演）。
async fn run_causal_simulation_llm(
    state: &AppState,
    recent: &str,
    plan: DirectorPlan,
) -> DirectorPlan {
    let llm = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    let prov_kind = crate::llm_stream::runtime_provider_kind(&llm, &state.provider_kind);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return plan;
    }
    let goal = plan.goal.trim().to_string();
    let pressure = plan.pressure.as_deref().unwrap_or("").trim();
    let user = format!(
        "最近剧情：\n{}\n\n本章导演计划目标：{}\n当前压力：{}\n\n请按系统提示输出因果推演 JSON。",
        if recent.trim().is_empty() { "（暂无消息）" } else { recent.trim() },
        if goal.is_empty() { "（无）" } else { &goal },
        if pressure.is_empty() { "（无）" } else { pressure },
    );
    let raw = match stream_chat_completions_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        &prov_kind,
        SYSTEM_PROMPT,
        &user,
        0.1,
        8192,
        90,
        |_| true,
    )
    .await
    {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!(error = %e, %goal, "st skim: 因果推演 LLM 失败，降级跳过");
            return plan;
        }
    };
    let trimmed = raw.trim().trim_matches('`');
    let trimmed = trimmed
        .strip_prefix("json")
        .map(|s| s.trim_matches('`').trim())
        .unwrap_or(trimmed)
        .trim();
    let mut sim: ChapterSimulation = match serde_json::from_str(trimmed) {
        Ok(sim) => sim,
        Err(e) => {
            tracing::warn!(error = %e, %goal, "st skim: 因果推演 JSON 解析失败，降级跳过");
            return plan;
        }
    };
    if let Err(e) = validate_simulation(&mut sim) {
        tracing::warn!(error = %e, %goal, "st skim: 因果推演校验未通过，降级跳过");
        return plan;
    }
    tracing::info!(
        %goal,
        beats = sim.causal_chain.len(),
        "st skim: 因果推演通过"
    );
    // 生成人类可读推演摘要（诊断日志，仅 debug；正文侧不直接输出该骨架）。
    let sim_excerpt: String = render_simulation(&sim).chars().take(300).collect();
    tracing::debug!(
        %goal,
        sim_excerpt,
        "st skim: 因果推演摘要（前 300 字）"
    );
    let mut plan = plan;
    let tail = format!(
        "。推演结束状态：{}。下一章推动力：{}",
        sim.ending_state, sim.next_impetus
    );
    if plan.goal.trim().is_empty() {
        plan.goal = format!("{}{}", sim.ending_state, tail);
    } else {
        plan.goal = format!("{}{}", plan.goal, tail);
    }
    plan
}

/// P1-1 (吞噬 denova 导演 agent): 独立导演 LLM 调用 —— 导演台手动触发时直接生成导演计划，
/// 不等下一回合。返回 Ok(Some(plan)) 成功；Ok(None) 未配置 LLM / 无需更新（null）；Err 为上游失败。
/// 输入素材：当前 node（summary + locked_beats + exits + allowed_divergence）+ 最近剧情（≤8 条）。
pub(crate) async fn generate_director_plan_llm(
    state: &AppState,
    sess: &TavernSession,
    pack: &StoryPack,
) -> Result<Option<DirectorPlan>, String> {
    let llm = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    let prov_kind = crate::llm_stream::runtime_provider_kind(&llm, &state.provider_kind);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return Ok(None);
    }
    let node = sess
        .node_id
        .as_deref()
        .and_then(|nid| pack.nodes.iter().find(|n| n.id == *nid));
    let (node_summary, beats, exits, diverge) = match node {
        Some(n) => (
            n.summary.clone(),
            n.locked_beats.join("；"),
            n.exit
                .iter()
                .map(|e| format!("{}→{}", e.when, e.next))
                .collect::<Vec<_>>()
                .join("；"),
            n.allowed_divergence.clone(),
        ),
        None => (String::new(), String::new(), String::new(), String::new()),
    };
    let recent_raw = sess
        .messages
        .iter()
        .rev()
        .take(8)
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n");
    // G4 (吞噬 denova fitTextToTokenBudget): 导演上下文预算——最近剧情按 2000 字符预算拟合，
    // 超预算头尾保留、中间省略（长会话导演上下文不溢出）。CJK 1 字符 ≈ 1 token 近似。
    const RECENT_BUDGET: usize = 2000;
    let recent_bytes = recent_raw.len();
    let recent = if recent_raw.is_empty() {
        recent_raw
    } else {
        kaleido_core::fit_text_to_token_budget(&recent_raw, RECENT_BUDGET, 0.5)
    };
    // G5 (吞噬 denova ContextLedger): 上下文账本——登记每个注入块的来源/字节/预算/是否进入最终消息，
    // 打日志供审计（长会话上下文溢出与裁剪行为可观测）。
    let ledger = vec![
        kaleido_core::DirectorLedgerEntry {
            source: "recent".into(),
            title: "最近剧情".into(),
            body_bytes: recent_bytes,
            limit: RECENT_BUDGET,
            included: recent_bytes <= RECENT_BUDGET,
            note: if recent_bytes > RECENT_BUDGET {
                format!("超预算，裁剪后 {} 字节", recent.len())
            } else {
                String::new()
            },
        },
        kaleido_core::DirectorLedgerEntry {
            source: "node_summary".into(),
            title: "当前节点".into(),
            body_bytes: node_summary.len(),
            limit: 0,
            included: true,
            note: String::new(),
        },
    ];
    tracing::debug!(ledger = ?ledger, %sess.session_id, "director ctx ledger");
    let system = "你是故事导演 agent。只输出导演计划，绝不代写剧情。导演计划是叙事意图（goal/pressure/cost/命中的 locked_beats），供主线牵引，不改变已锁定的硬事实。输出严格 JSON 对象：{\"goal\":\"…\",\"pressure\":\"…\",\"cost\":\"…\",\"hits_beats\":[\"…\"]}；无需更新时输出 null。".to_string();
    let mainline = pack.stage_director.mainline_strength.clone();
    let user = format!(
        "当前节点：{}\nlocked_beats：{}\n可用出口：{}\n允许偏离度：{}\n\n[策略档] mainline_strength={}（strong_arc=严格遵循原著主线；balanced=主线优先、允许合理分支；soft_guidance=宽松探索，旧值 soft 兼容映射）\n\n最近剧情：\n{}\n\n请生成导演计划 JSON。",
        if node_summary.is_empty() { "（起点/无节点）" } else { &node_summary },
        if beats.is_empty() { "（无）" } else { &beats },
        if exits.is_empty() { "（无）" } else { &exits },
        if diverge.is_empty() { "（默认）" } else { &diverge },
        if mainline.trim().is_empty() { "balanced" } else { &mainline },
        if recent.is_empty() { "（暂无消息）" } else { &recent },
    );
    let raw = stream_chat_completions_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        &prov_kind,
        &system,
        &user,
        0.1,
        16384,
        120,
        |_| true,
    )
    .await?;
    let trimmed = raw.trim().trim_matches('`');
    let trimmed = trimmed
        .strip_prefix("json")
        .map(|s| s.trim_matches('`').trim())
        .unwrap_or(trimmed)
        .trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("null")
        || trimmed.eq_ignore_ascii_case("none")
    {
        return Ok(None);
    }
    let v: Value =
        serde_json::from_str(trimmed).map_err(|e| format!("导演计划 JSON 解析失败: {e}"))?;
    let goal = v.get("goal").and_then(|x| x.as_str()).unwrap_or("").to_string();
    if goal.is_empty() {
        return Ok(None);
    }
    let goal_summary = goal.clone();
    let plan = DirectorPlan {
        goal,
        pressure: v
            .get("pressure")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        cost: v
            .get("cost")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        hits_beats: v
            .get("hits_beats")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
        created_turn: sess.director_plan.as_ref().map(|p| p.created_turn).unwrap_or_default(),
        updated_turn: sess.director_plan.as_ref().map(|p| p.updated_turn).unwrap_or_default(),
        // G1: 三文档（从 LLM 输出提取，缺省空串；旧逻辑无此字段 → 空）
        plan: v.get("plan").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        agent_brief: v
            .get("agent_brief")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        lore_context: v
            .get("lore_context")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        // G2: 状态机——LLM 成功 → ready
        last_run: Some(DirectorPlanRunStatus::ready(goal_summary)),
    };
    // X2c (吞噬自 xiami story_simulation.rs): 因果推演校验 —— LLM 生成导演计划后追加一次
    // 因果推演调用（复用 resolve_llm + stream_chat_completions，温度 0.1/max_tokens 2048/90s）。
    // [fix 2026-08-15] 2048→8192：模型思考挤占预算易截断。
    // 校验通过 → 把 ending_state / next_impetus 追加进导演计划 goal 描述；失败/出错静默降级
    // （保持现状行为，不阻断导演计划）。
    let plan = run_causal_simulation_llm(state, &recent, plan).await;
    Ok(Some(plan))
}

/// S4 (吞噬 denova director_plan) + G13/G14: 手动触发导演计划（后台化）。
/// 请求立即返回 `{"started":bool,"plan":…}`；LLM 生成在 `tokio::spawn` 后台执行，
/// 经 `DirectorTaskGroup`（key = session_id）串行登记——HTTP 断线不取消。
/// - `started=true`：任务已登记，完成后写回 session.director_plan（成功 → ready；
///   失败 → warn 日志 + last_run=conflict，复用 G1/G2 conflict 语义）。
/// - `started=false`：同 key 已有后台任务在跑（denova `!started` → canceled 语义：不排队、不覆盖）。
pub(crate) async fn run_director_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut s = match state.sessions_tavern.get_for_owner(&session_id, &session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if s.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }
    let _pack = match state.packs.get(&s.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    // G13/G14: 同 key 串行登记——已有后台任务在跑则不启动（denova !started → canceled 语义）
    let group = state.director_tasks.clone();
    if !group.acquire(&session_id, DIRECTOR_TASK_DIRECTOR_PLAN_UPDATE) {
        return Json(json!({
            "started": false,
            "plan": s.director_plan,
        }))
        .into_response();
    }
    // P3.2 (编排候选①): 导演计划 job 化 —— 长时 LLM 任务登记进 JobStore，
    // GET /api/v1/jobs 可观测进度/终态；重启后由 recover_hook 孤儿清理兜底。
    // payload 记录触发来源与目标会话；model 取当前解析的 LLM 配置。
    let llm_probe = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    let dp_job = match state.jobs.create(
        "director-plan",
        s.owner.as_deref().unwrap_or(&session.user_id),
        "",
        json!({
            "sessionId": session_id,
            "packId": s.pack_id,
            "trigger": "manual",
        }),
        Some(llm_probe.model.clone()),
        None,
    ) {
        Ok(j) => j,
        Err(e) => {
            group.release(&session_id);
            return map_core_err(e);
        }
    };
    let dp_run_id = dp_job.run_id.clone();
    // G13/G14: 登记当前后台任务 id（GET director-config 可见）
    s.director_task = Some(DIRECTOR_TASK_DIRECTOR_PLAN_UPDATE.into());
    let plan_now = s.director_plan.clone();
    if let Err(e) = state.sessions_tavern.save(s) {
        group.release(&session_id);
        return map_core_err(e);
    }

    let state_bg = state.clone();
    let sessions_store = state.sessions_tavern.clone();
    let packs_store = state.packs.clone();
    let session_id_bg = session_id.clone();
    let jobs_store = state.jobs.clone();
    let dp_run_id_bg = dp_run_id.clone();
    // P3.2: 起跑事件（progress 0.1）—— jobs 列表立即可见任务已启动。
    let _ = jobs_store.push_event(
        &dp_run_id,
        kaleido_core::JobEvent::progress("director: planning started", 0.1),
        Some(0.1),
        Some("stage:start".to_string()),
    );
    // 后台执行：LLM 完成写回；失败 warn 日志 + conflict；HTTP 断线不取消。
    tokio::spawn(async move {
        // P3.2: LLM 阶段事件（progress 0.3）。
        let _ = jobs_store.push_event(
            &dp_run_id_bg,
            kaleido_core::JobEvent::progress("director: llm generating plan", 0.3),
            Some(0.3),
            Some("stage:llm".to_string()),
        );
        let outcome: Result<Option<DirectorPlan>, String> = async {
            let sess = match sessions_store.get(&session_id_bg) {
                Ok(s) => s,
                Err(e) => return Err(format!("load session failed: {e}")),
            };
            if sess.pack_missing {
                return Err("session is read-only (pack missing)".to_string());
            }
            let pack = match packs_store.get(&sess.pack_id) {
                Ok(p) => p,
                Err(e) => return Err(format!("load pack failed: {e}")),
            };
            generate_director_plan_llm(&state_bg, &sess, &pack).await
        }
        .await;
        let mut fresh = match sessions_store.get(&session_id_bg) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(error = %e, %session_id_bg, "director plan bg: load session failed");
                // P3.2: 会话加载失败也要给 job 终态（防幽灵 running 占并发槽）。
                let _ = jobs_store.complete(
                    &dp_run_id_bg,
                    "failed",
                    None,
                    Some(format!("load session failed: {e}")),
                );
                group.release(&session_id_bg);
                return;
            }
        };
        fresh.director_task = None;
        match &outcome {
            Ok(Some(plan)) => {
                fresh.director_plan = Some(plan.clone());
                fresh.director_pending = false;
                tracing::info!(%session_id_bg, "director plan bg: plan ready");
            }
            Ok(None) => {
                // LLM 未配置 / 模型返回 null：不清空既有计划，也不伪造成 conflict。
                tracing::info!(%session_id_bg, "director plan bg: no update (LLM unconfigured or null)");
            }
            Err(ref e) => {
                // G1/G2 conflict 语义：失败 → last_run=conflict，前端可见。
                tracing::warn!(error = %e, %session_id_bg, "director plan bg: LLM failed, mark conflict");
                let mut existing = fresh.director_plan.clone().unwrap_or_default();
                existing.last_run = Some(DirectorPlanRunStatus::conflict(format!(
                    "导演计划生成失败（上游 LLM 错误）：{e}"
                )));
                fresh.director_plan = Some(existing);
            }
        }
        // P3.2: 先取终态分类（借用）再消费 outcome，避免 Err(String) 被部分移动。
        let dp_failed_msg: Option<String> = match &outcome {
            Err(e) => Some(e.clone()),
            _ => None,
        };
        remerge_bg_fields(&sessions_store, &mut fresh);
        if let Err(e) = sessions_store.save(fresh.clone()) {
            tracing::warn!(error = %e, %session_id_bg, "director plan bg: save failed");
        }
        // P3.2 (I1): job 终态收口 —— 成功带结果摘要；None 视为无更新成功；
        // 失败标 failed 并透出错误。取消优先语义由 complete 内部保证（晚到终态不复活）。
        if dp_failed_msg.is_none() {
            if let Ok(Some(plan)) = &outcome {
                let _ = jobs_store.push_event(
                    &dp_run_id_bg,
                    kaleido_core::JobEvent::progress("director: plan ready", 0.9),
                    Some(0.9),
                    Some("stage:apply".to_string()),
                );
                let result = json!({
                    "updated": true,
                    "sessionId": session_id_bg,
                    "goal": plan.goal,
                    "pressure": plan.pressure,
                    "hitsBeats": plan.hits_beats,
                });
                let _ = jobs_store.complete(
                    &dp_run_id_bg,
                    "succeeded",
                    Some(result),
                    None,
                );
            }
        } else if matches!(&outcome, Ok(None)) {
            // Ok(None)：LLM 未配置或模型返回 null —— 无更新，仍算成功完成。
            let _ = jobs_store.complete(
                &dp_run_id_bg,
                "succeeded",
                Some(json!({
                    "updated": false,
                    "sessionId": session_id_bg,
                    "reason": "llm unconfigured or model returned null",
                })),
                None,
            );
        } else {
            let e = dp_failed_msg.unwrap_or_default();
            let _ = jobs_store.push_event(
                &dp_run_id_bg,
                kaleido_core::JobEvent::progress("director: generation failed", 1.0),
                Some(1.0),
                Some("stage:error".to_string()),
            );
            let _ = jobs_store.complete(
                &dp_run_id_bg,
                "failed",
                None,
                Some(format!("director plan generation failed: {e}")),
            );
        }
        group.release(&session_id_bg);
    });

    // P3.2: 返回 runId —— 客户端可经 GET /api/v1/jobs/{runId} 轮询导演任务进度。
    Json(json!({
        "started": true,
        "runId": dp_run_id,
        "plan": plan_now,
    }))
    .into_response()
}

/// S4 (吞噬 denova director_plan): 应用本回合导演计划块（已剥离，不显示在正文）；
/// 随后消费导演手动触发挂起标记。
/// [P5 2026-10-05 从 start_turn 内联提取] 行为逐字节不变。
pub(crate) fn apply_director_plan_update(
    sess: &mut kaleido_core::TavernSession,
    director_plan_update: Option<DirectorPlanUpdate>,
    director_instruct_bg: bool,
) {
                // S4 (吞噬 denova director_plan): 应用本回合导演计划块（已剥离，不显示在正文）
                if let Some(update) = director_plan_update {
                    match update {
                        DirectorPlanUpdate::Skip => {
                            // 【导演计划】none → 不动现有计划
                        }
                        DirectorPlanUpdate::Set(out) => {
                            let is_first = sess.director_plan.is_none();
                            let existing = sess.director_plan.take().unwrap_or_default();
                            sess.director_plan = Some(kaleido_core::DirectorPlan {
                                goal: out.goal,
                                pressure: out.pressure,
                                cost: out.cost,
                                hits_beats: out.hits_beats,
                                created_turn: if is_first { sess.turn } else { existing.created_turn },
                                updated_turn: sess.turn,
                                plan: existing.plan,
                                agent_brief: existing.agent_brief,
                                lore_context: existing.lore_context,
                                last_run: Some(kaleido_core::DirectorPlanRunStatus::ready("主线导演计划已更新".to_string())),
                            });
                            tracing::info!(turn = sess.turn, "st director plan updated");
                        }
                    }
                }
                // 手动触发已被本回合消费：清挂起标记（无论 LLM 返回 none 还是新计划）
                if director_instruct_bg && sess.director_pending {
                    sess.director_pending = false;
                    tracing::info!("st director pending consumed");
                }
}

