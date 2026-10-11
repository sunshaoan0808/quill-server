//! [2026-10-06 P8] start_turn 本体迁入 turn/entry.rs；p006.rs 至此退役删除。
//! 回合编排壳：预检 → 流式主调用 → 质量管道 → 解析 → 持久化 → 记忆链。
#![allow(unused_imports)]
use super::super::*;

pub(crate) async fn start_turn(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<TurnStartRequest>,
) -> Response {
    let _session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    // 1. Validate session (F1: ownership check)
    let mut tavern = match state.sessions_tavern.get_for_owner(&session_id, &_session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };
    if tavern.pack_missing {
        return map_core_err(kaleido_core::CoreError::BadRequest(
            "pack missing: session is read-only".into(),
        ));
    }

    // G10 (吞噬 denova D1): 回合提交幂等守卫 —— 同一回合同内容已提交时直接返回幂等回执
    // （accepted=false + duplicate_submit + 最近 assistant 消息 id），不产生第二次副作用
    // （LLM 双跑 / 双写入）。未完成回合（末条 user 消息无 assistant 回应）交由下方
    // P2 空响应复用重试路径处理，不判重复。
    let user_msg_hash = kaleido_core::text_hash(&body.message);
    if kaleido_core::turn_submit_guard(&tavern, &user_msg_hash) {
        let message_id = tavern
            .messages
            .iter()
            .rev()
            .find(|m| m.role == "assistant")
            .map(|m| m.id.clone());
        tracing::info!(
            %session_id,
            hash = %user_msg_hash,
            message_id = ?message_id,
            "st turn duplicate submit rejected (idempotent receipt)"
        );
        return Json(json!({
            "accepted": false,
            "reason": "duplicate_submit",
            "messageId": message_id,
            "sessionId": session_id,
        }))
        .into_response();
    }

    // Stale active_run locks the session after aborted streams / empty completions.
    // Auto-clear when the job is no longer running; otherwise ask client to stop.
    // U11: 中断回合恢复（resume）—— job 仍标 running 但无活 worker（服务重启 / 任务进程死亡 /
    // 超时遗留）时，不再 409 死锁：先 rearm_running 留恢复证据，再以 cancelled 终态关闭旧记录
    // 释放并发槽，随后走正常新回合（用户消息与上下文完整保留，可验证 active_run_id 状态机）。
    let mut resumed_bg = false;
    if let Some(prev) = tavern.active_run_id.clone() {
        let still_running = state
            .jobs
            .get(&prev)
            .map(|j| kaleido_core::is_active_job_status(&j.status))
            .unwrap_or(false);
        if still_running {
            // 仅当 hub 中仍有该 run 的流式 worker 时维持 409（并发 turn 护栏）。
            if state.hub.has_live_worker(&prev) {
                return conflict("ST_TURN_BUSY", "turn in progress; tap 停止 then retry");
            }
            tracing::info!(%session_id, run_id=%prev, "st turn resume: orphaned running job detected");
            resumed_bg = true;
            match state.jobs.rearm_running(&prev) {
                Ok(_) => {
                    // rearm 后立刻以 cancelled 终态关闭旧记录（释放 running 并发槽）；
                    // complete 的 cancel-wins 语义保证不会复活已停止的旧 run。
                    let _ = state.jobs.complete(
                        &prev,
                        "cancelled",
                        None,
                        Some("U11 resume: interrupted turn rearmed then superseded by new run".into()),
                    );
                }
                Err(e) => {
                    tracing::warn!(%session_id, run_id=%prev, error=%e, "st turn resume: rearm failed, continuing anyway");
                }
            }
        }
        // F2: Use atomic release to avoid overwriting a concurrent acquire_turn.
        state.sessions_tavern.release_turn(&session_id, Some(&prev));
        tavern.active_run_id = None;
    }

    // F2: Atomic turn acquisition — set active_run_id to a placeholder BEFORE
    // any heavy work. This prevents two concurrent requests from both seeing
    // active_run_id==None and proceeding to spawn duplicate LLM workers.
    let pending_run_id = format!("pending-{}", tavern.turn + 1);
    match state.sessions_tavern.acquire_turn(&session_id, &pending_run_id) {
        Ok(updated) => {
            tavern = updated;
        }
        Err(kaleido_core::CoreError::Conflict(_msg)) => {
            return conflict("ST_TURN_BUSY", "turn in progress; tap 停止 then retry");
        }
        Err(e) => return map_core_err(e),
    }

    // 2. Persist user message first (failover — Q41)
    // 首回合也保留“回合前”快照，使 /reroll 不会因 no checkpoint 失败。
    // 仅在无历史检查点时写入，避免重复请求不断堆叠相同的 turn-0 快照。
    if tavern.checkpoints.is_empty() {
        tavern.push_checkpoint();
    }
    // P2 空响应重试去重：仅当上一回合未产出 assistant（末条消息仍为同内容的本用户消息）时
    // 复用原 msg，避免重试产生连续重复的用户消息（规格：同一输入可 idempotent 重试）。
    let reuse_last_user = matches!(
        tavern.messages.last(),
        Some(m) if m.role == "user" && m.content == body.message
    );
    if reuse_last_user {
        tracing::info!(%session_id, "st retry: reusing last user message");
    } else {
        // [fix chaos 交付/清场] 新用户回合开始：清掉上一回合已交付（delivered）的命运事件
        // pending（含 pressure 重置，语义见 chaos.rs clear_delivered_if_any）。
        // 未交付（delivered=false）或无 pending 时为 no-op，不影响压力累积；
        // 空响应重试路径（reuse_last_user）不清，保留原注入等待重试成功。
        tavern.chaos.clear_delivered_if_any();
        let user_msg = TavernMessage {
            id: format!("msg-{}", Uuid::new_v4()),
            role: "user".into(),
            content: body.message.clone(),
            created_at: Utc::now().to_rfc3339(),
            options: vec![],
            engine_tag: None,
            program: None,
            reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
        };
        tavern.messages.push(user_msg);
    }
    // F2: active_run_id already set atomically by acquire_turn above.

    // 3. Load pack context
    let (pack, director_instruct_bg, chapter_body, next_turn, query_embedding) =
        match load_pack_context(&state, &mut tavern, &session_id).await {
            Ok(v) => v,
            Err(r) => return r,
        };

    // 4. Build system prompt
    let mut system_prompt = build_turn_system_prompt(
        &state, &pack, &mut tavern, &session_id, &_session.workspace_id, &chapter_body,
        director_instruct_bg, query_embedding,
    ).await;
    // 5. Build user message for LLM
    // §13.4① (2026-08-18): 玩家动作指令强制包装（短动作型消息加强调，防选项动作被吞）
    let user_content = wrap_player_action(&body.message).unwrap_or_else(|| body.message.clone());
    // [时间天气 v2 2026-08-17] 捕获玩家原始消息到 spawn 作用域（供回合结束时间/天气
    // 推进信号解析；spawn 内 `let body=json!()` 会遮蔽外层 body，故单独捕获此变量）。
    let player_msg_original = body.message.clone();

    // 6. Resolve LLM config
    let llm = state.app_state.resolve_llm(
        state.llm_base.as_deref(),
        state.llm_key.as_deref(),
        &state.llm_model,
    );
    let base = llm.base_url.clone();
    let key = llm.api_key.clone();
    if base.trim().is_empty() || key.trim().is_empty() {
        return service_unavailable("ST_LLM_NOT_CONFIGURED", "LLM not configured");
    }
    // G6: effective provider kind (managed provider protocol > env default).
    let prov_kind_bg = crate::llm_stream::runtime_provider_kind(&llm, &state.provider_kind);

    let model = body
        .model
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let m = llm.model.clone();
            if m.is_empty() {
                state.llm_model.clone()
            } else {
                m
            }
        });

    // P0: 回合生成质量档位（吞噬 denova novel-lite/standard/heavy）。
    // 生效顺序：turn 请求参数 > 会话持久化档位 > lite（现状零回归）。
    // 显式传入且与会话档位不同时回写会话，实现 per-session 选择（API 优先；前端 UI 可后置）。
    let quality = body.quality.unwrap_or_else(|| tavern.quality.into());
    if quality != tavern.quality.into() {
        tavern.quality = quality.into();
    }
    info!(?quality, "st turn quality");

    // P4 (吞噬 denova appendWritingSkillLoadHint): 本轮系统提示注入写作 Skill 档位行
    // + 原则级写作规则（standard/heavy 有规则文件才注入；lite 仅提示，控制 token）。
    {
        let tier = crate::skill_layer::resolve_tier_for_quality(quality);
        let ws_id = _session.workspace_id.clone();
        let skill_hint = load_writing_skill(state.auth.data_root().root(), Some(&ws_id), tier);
        system_prompt = append_writing_skill_hint(&system_prompt, tier, skill_hint.as_ref());
    }

    // 7. Create Job
    let job = match state.jobs.create(
        "tavern-turn",
        &_session.user_id,
        &_session.workspace_id,
        json!({
            "sessionId": session_id,
            "packId": tavern.pack_id,
        }),
        Some(model.clone()),
        None,
    ) {
        Ok(j) => j,
        Err(e) => return map_core_err(e),
    };
    let run_id = job.run_id.clone();

    // F3: If the job was queued (at concurrency limit), do NOT spawn the LLM
    // worker. Release the turn lock and return 429 so the client can retry later.
    // This ensures max_concurrent_jobs is a real limit on upstream model requests.
    if kaleido_core::normalize_job_status(&job.status) == "queued" {
        tracing::info!(
            %session_id, %run_id,
            running = state.jobs.running_count(),
            "st turn rejected: job queued (concurrency limit)"
        );
        // Cancel the queued job and release the session turn lock.
        let _ = state.jobs.cancel(&run_id);
        state.sessions_tavern.release_turn(&session_id, Some(&run_id));
        return err_with_code(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            "ST_SERVER_BUSY",
            "server busy: concurrency limit reached, please retry shortly",
            serde_json::json!({ "runId": run_id, "retryable": true }),
        );
    }

    tavern.active_run_id = Some(run_id.clone());

    // 8. Save session (user msg persisted, run_id set)
    // M-2 (CAS): use save_with_revision with the base revision captured at
    // acquire_turn (the most recent locked write). If a concurrent save
    // happened in between (e.g. director_plan CAS write above updated the
    // revision), we re-read the live revision from tavern.updated_at so the
    // CAS reflects the latest on-disk state. On conflict, surface 409 so the
    // client can retry — we must NOT silently overwrite a concurrent write.
    // acquire_turn already serialized this session under the store lock. The
    // opening-plan background task may advance updated_at immediately after
    // acquire; using a second stale CAS here turns a valid first turn into a
    // ghost job. Persist the acquired snapshot atomically; later worker writes
    // still use CAS/merge guards.
    match state.sessions_tavern.save(tavern) {
        Ok(_) => {}
        Err(e) => {
            // F4 (2026-08-16 幽灵 job 根治): job 已在上面 create 标为 running，
            // 若在此早退（CAS 冲突等）不 cancel，会遗留「running 但 worker 未 spawn」
            // 的幽灵 job，永久占满 max_concurrent_jobs，导致后续所有回合 429
            // （st turn rejected: job queued）。必须先 cancel 再返回。
            let _ = state.jobs.cancel(&run_id);
            state.sessions_tavern.release_turn(&session_id, Some(&run_id));
            return map_core_err(e);
        }
    }

    // [morphling ROMA P0 2026-08-19] 回合级检查点：进入 LLM 流前落「Streaming」阶段，
    // 崩溃/中断后可从会话文件判读上一回合死在哪一步（U11 resume 用）。跟随会话落盘。
    let _ = state.sessions_tavern.update_session(&session_id, |sess| {
        sess.set_turn_progress(next_turn, TurnPhase::Streaming, &run_id, "mainstream llm");
        Ok(())
    });

    // P3 编排（I3）：阶段事件写入 job 记录 —— GET /api/v1/jobs 可观测回合进度
    // （progress/cursor 与 background pipeline 同口径，双通道统一，审D 三-3）。
    let _ = state.jobs.push_event(
        &run_id,
        kaleido_core::JobEvent::progress("streaming: main llm", 0.1),
        Some(0.1),
        Some("phase:streaming".to_string()),
    );

    // 9. Register with StreamHub
    let tx = state.hub.register(&run_id);
    info!(%run_id, %session_id, %model, "tavern turn started");

    // 10. Spawn LLM streaming task
    let hub = state.hub.clone();
    let jobs = state.jobs.clone();
    let sessions_store = state.sessions_tavern.clone();
    let packs_store = state.packs.clone();
    let works_store = state.works.clone();
    let workspace_id_bg = _session.workspace_id.clone();
    let cross_dir = state.auth.data_root().cross_session_dir();
    let run_id_bg = run_id.clone();
    let session_id_bg = session_id.clone();
    let _pack_id_bg = pack.id.clone();
    let base_bg = base;
    let key_bg = key;
    let model_bg = model.clone();
    let prov_bg = prov_kind_bg;
    // Fallback model for upstream 4xx model errors (invalid_model / model_not_found):
    // env default is the most trustworthy (frontend/settings may carry stale ids).
    let fallback_model_bg = state.llm_model.clone();
    let sys_prompt_bg = system_prompt;
    let user_bg = user_content;
    // [时间天气 v2 2026-08-17] spawn 块内捕获玩家原始消息（外层 body.message 会被
    // 内层 `let body=json!()` 遮蔽），供回合结束的时间/天气推进信号解析用。
    let player_msg_bg = player_msg_original;
    // §13.5 Scene Gate: 首回合场景错位纠偏需要回合号（turn==1 才启用）
    let next_turn_bg = next_turn;
    let embedding_base_bg = state.embedding_base.clone();
    let data_root_bg = state.auth.data_root().root().to_path_buf();
    let quality_bg = quality;
    // U11: 本回合是否经中断恢复（孤儿 running → rearm + 新回合）进入。
    let resumed_bg = resumed_bg;
    // P4: 装载当前档位写作 Skill（workspace→user→builtin 三层），随 spawn 传入质量管道。
    let skill_bg = load_writing_skill(
        &data_root_bg,
        Some(&workspace_id_bg),
        crate::skill_layer::resolve_tier_for_quality(quality),
    );

    // P3 编排硬化（I1 前置捕获）：看门狗克隆集必须在 worker spawn 之前创建 ——
    // 下方 `async move` 会把 hub/jobs/sessions_store/run_id_bg/... 移入 worker 闭包，
    // 之后原绑定失效无法再 clone。
    let hub_panic = hub.clone();
    let jobs_panic = jobs.clone();
    let sessions_store_panic = sessions_store.clone();
    let run_id_panic = run_id_bg.clone();
    let session_id_panic = session_id_bg.clone();
    let model_panic = model_bg.clone();
    let quality_panic = quality_bg;
    let sys_prompt_panic = sys_prompt_bg.clone();
    let user_panic = user_bg.clone();
    let resumed_panic = resumed_bg;
    let worker = tokio::spawn(async move {
        // U11: 回合预算/成本记账状态（主流 + 质量管道 + 非阻塞后处理共享）。
        let turn_started = std::time::Instant::now();
        let turn_started_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // [P7 2026-10-06] 回合上下文参数包：LLM 路由/身份/环境/计时/计数 收编于 TurnCtx
        //（设计文档 §3.1 P7），取代 stream/memory 提取函数原散装值参数。
        // [P7] retried_empty 标志已移除：空响应重试天然单发——入口条件含 full_text.is_empty()，
        // 重试成功则文本非空不再进入，仍空/失败则提前 return（无循环重入路径）。
        let mut ctx = TurnCtx {
            base_bg, key_bg, model_bg, fallback_model_bg, prov_bg, sys_prompt_bg, user_bg,
            run_id_bg, session_id_bg, workspace_id_bg, player_msg_bg,
            cross_dir, embedding_base_bg, quality_bg, resumed_bg, next_turn_bg,
            turn_started, turn_started_ms,
            used_fallback: false,
            extra_llm_calls: 0,
        };
        // [R2 编排测试 2026-10-06] 可注入 panic（仅测试构建）：message 为 __panic_probe__
        // 时 worker 立即 panic，用于验证 panic 看门狗兜底路径（job failed/清锁/hub 清理）。
        #[cfg(test)]
        if ctx.user_bg == "__panic_probe__" {
            panic!("simulated worker panic (turn_orch_tests)");
        }
        // ===== 主回合流式调用链提取（P5）：首发 attempt、U11 收尾、模型回退、
        // 空响应重试、Scene Gate 纠偏统一提取为 run_main_stream_phase。
        // 终态路径（原 worker `return`，已收尾）→ None；正常路径 → 三元组。
        let (mut full_text, usage_tokens, thinking_text) = match run_main_stream_phase(
            &mut ctx, &tx, &hub, &sessions_store, &jobs,
        ).await {
            Some(t) => t,
            None => return,
        };
        if !full_text.is_empty() {
            // P0 (吞噬 denova 写作三档): standard/heavy 追加多轮 LLM 协作。
            // lite 直出不走此分支（零回归）；失败保留初稿，绝不阻塞正文。
            // 协议块预剥离 stash：heavy/standard 多轮管道会重写正文吞掉主调用的
            // 协议块，进入 run_quality_refine 前按序预剥离并暂存；lite 不走预剥离
            // （stash 全空 → 管道后剥离段照常工作，零回归）。
            // [P5 2026-10-05] stash 预剥离 + 质量管道提取为同文件私有 async fn
            // run_pre_refine_phase（mem::take 保块体字节不变；9 个 stash 经元组返回，下游引用名不变）。
            let mut thinking_pipe = String::new();
            let (stash_options, stash_panels, stash_program, stash_mcp, stash_state, stash_check, stash_plan, stash_advance, stash_receipt) = run_pre_refine_phase(
                &mut full_text, &mut thinking_pipe, &sessions_store, &jobs, ctx.quality_bg,
                &ctx.base_bg, &ctx.key_bg, &ctx.model_bg, &ctx.prov_bg, skill_bg.as_ref(),
                &data_root_bg, &ctx.workspace_id_bg, &ctx.user_bg, &ctx.sys_prompt_bg,
                &ctx.session_id_bg, ctx.next_turn_bg, &ctx.run_id_bg, ctx.turn_started_ms,
            ).await;

            // [P5 2026-10-05] 管道后解析提取为同文件私有 async fn run_post_parse_phase
            //（mem::take 保块体字节不变；11 个值经元组返回，下游引用名不变）。
            let (merged_receipt, options, panels, program, mcp_results, state_updates, roster_names, check_requests, director_plan_update, mut llm_advance_to) = run_post_parse_phase(
                &mut full_text, &stash_receipt, stash_options, stash_panels, stash_program,
                stash_mcp, stash_state, stash_check, stash_plan, stash_advance,
                &sessions_store, ctx.quality_bg, &data_root_bg, &ctx.workspace_id_bg, &ctx.session_id_bg, &pack,
            ).await;
            // [P5 2026-10-05] 速读质检回写 + 持久化 + done 收口提取为 assemble_and_persist_turn
            //（sess 快照在函数内 get/save；engine_tag 上提供 memory 阶段共用）。
            if let Ok(mut sess) = sessions_store.get(&ctx.session_id_bg) {
            let engine_tag = kaleido_core::classify_engine_tag(&ctx.user_bg);
            assemble_and_persist_turn(
                &mut sess, &sessions_store, &packs_store, &jobs, &tx, &state, &pack, &full_text,
                &ctx.player_msg_bg, &ctx.user_bg, engine_tag, &thinking_text, &thinking_pipe,
                &merged_receipt, &check_requests, &options, &panels, program, mcp_results,
                state_updates, director_plan_update, director_instruct_bg, usage_tokens,
                &ctx.session_id_bg, ctx.next_turn_bg, &ctx.run_id_bg,
            ).await;

                // [P5 2026-10-05] 记忆/守卫/账本/章节摘要/节点推进 提取为 post_turn_memory_phase
                //（canon 守卫打回 llm_advance_to 与 LLM 计数 extra_llm_calls 经 &mut 写回）。
                post_turn_memory_phase(
                    &mut sess, &mut ctx, &packs_store, &sessions_store, &state, &jobs, &full_text,
                    &pack, &chapter_body, roster_names, &works_store, engine_tag,
                    &mut llm_advance_to,
                ).await;
                // 以既有保留策略限制容量：core `push_checkpoint` 自带 MAX_CHECKPOINTS(30) 最旧裁剪，
                // 与 restore_checkpoint 的截断语义对齐，回合级快照不会无界增长。
                sess.push_checkpoint();
                remerge_bg_fields(&sessions_store, &mut sess);
                let _ = sessions_store.save(sess);
            }
        }

        // P3 编排（I3）：回合全链路完成 —— job 进度事件收口（progress=1.0，cursor=done）。
        let _ = jobs.push_event(
            &ctx.run_id_bg,
            kaleido_core::JobEvent::progress("turn complete", 1.0),
            Some(1.0),
            Some("phase:done".to_string()),
        );

        hub.cleanup(&ctx.run_id_bg);
    });

    // P3 编排硬化（吞噬审D 二-4 / 审查C 4-5）：worker panic 终态兜底看门狗。
    // 回合 worker 若 panic（pack 数据异常 / 索引越界等），tokio 默认吞掉 panic：
    // SSE 静默断流、job 永远 running 占满并发槽、会话锁死。此看门狗 await JoinHandle，
    // 仅在 Err（panic/abort）时兜底收尾——发 error 终态事件（hub.send 带重放持久化）+
    // job failed + U11 记账 + 清锁 + hub 清理；正常路径 worker 自行收尾，看门狗零干预。
    tokio::spawn(async move {
        if worker.await.is_err() {
            // 终态保护：若 panic 发生在 jobs.finish("done") 之后的后处理阶段，
            // job 已是终态，不得降级为 failed（回合本身已成功交付）。
            let job_still_active = jobs_panic
                .get(&run_id_panic)
                .map(|j| kaleido_core::is_active_job_status(&j.status))
                .unwrap_or(false);
            if !job_still_active {
                warn!(run_id = %run_id_panic, "P3 panic watchdog: worker panicked after terminal state; no recovery needed");
                return;
            }
            warn!(run_id = %run_id_panic, "P3 panic watchdog: turn worker panicked; recovering terminal state");
            let _ = hub_panic.send(
                &run_id_panic,
                ChatStreamEvent {
                    run_id: run_id_panic.clone(),
                    event_type: "error".into(),
                    delta: None,
                    message: Some("turn worker panicked (internal error)".into()),
                    code: Some("TURN_PANIC".into()),
                    context_compaction: None,
                    input_tokens: None,
                    output_tokens: None,
                },
            );
            let elapsed_ms_v = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
                .saturating_sub(
                    jobs_panic
                        .get(&run_id_panic)
                        .map(|j| j.created_at.timestamp_millis().max(0) as u64)
                        .unwrap_or(0),
                );
            let _ = jobs_panic.merge_job_payload(
                &run_id_panic,
                u11_accounting_json(
                    &model_panic, quality_panic, &sys_prompt_panic, &user_panic, "",
                    0, false, elapsed_ms_v, resumed_panic, 0, None, Some("worker panic"),
                ),
            );
            jobs_panic.finish(&run_id_panic, "error");
            clear_session_active_run(&sessions_store_panic, &session_id_panic, Some(&run_id_panic));
            hub_panic.cleanup(&run_id_panic);
        }
    });

    Json(json!({
        "accepted": true,
        "runId": run_id,
        "sessionId": session_id,
    }))
    .into_response()
}

