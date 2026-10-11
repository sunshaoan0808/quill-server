//! [2026-10-06 P6] //! 主回合流式调用链（run_main_turn_attempt / run_main_stream_phase）。
#![allow(unused_imports)]
use super::super::*;

/// 主回合单次流式尝试（F3/G6 后置项）：把上游 TurnStreamEvent 翻译成
/// ChatStreamEvent 转发给前端（delta / thinking_delta），下游断开时取消 run。
/// SSE 解析已下沉 llm_stream::stream_chat_turn_dispatch，此处只做事件翻译。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_main_turn_attempt(
    base_url: &str,
    api_key: &str,
    model: &str,
    provider_kind: &str,
    sys_prompt: &str,
    user: &str,
    tx: &tokio::sync::broadcast::Sender<ChatStreamEvent>,
    run_id: &str,
    hub: &StreamHub,
) -> Result<crate::llm_stream::TurnStreamOutcome, crate::llm_stream::TurnStreamError> {
    use crate::llm_stream::{stream_chat_turn_dispatch, TurnStreamEvent};
    stream_chat_turn_dispatch(
        base_url, api_key, model, provider_kind, sys_prompt, user, 0.75, 32768, 300,
        |ev| {
            let (event_type, delta) = match ev {
                TurnStreamEvent::Delta(d) => ("delta", Some(d)),
                TurnStreamEvent::Thinking(t) => ("thinking_delta", Some(t)),
            };
            if tx
                .send(ChatStreamEvent {
                    run_id: run_id.to_string(),
                    event_type: event_type.into(),
                    delta,
                    message: None,
                    context_compaction: None,
                    input_tokens: None,
                    output_tokens: None,
                    code: None,
                })
                .is_err()
            {
                // 下游全部断开（客户端离线）：与旧内嵌循环同款处理。
                hub.cancel(run_id);
                return false;
            }
            true
        },
        || hub.is_cancelled(run_id),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
/// [P5 2026-10-06 从 start_turn worker 内联提取] 主回合流式调用链：首发
/// run_main_turn_attempt、Stopped/U11 超时与取消收尾、上游 4xx 模型回退重试、
/// 空响应单次重试、Scene Gate 首回合纠偏。终态路径（原 worker `return`）
/// 返回 None（收尾已在内部完成）；正常路径返回 (full_text, usage_tokens,
/// thinking_text)。行为逐字节不变。
pub(crate) async fn run_main_stream_phase(
    ctx: &mut TurnCtx,
    tx: &tokio::sync::broadcast::Sender<ChatStreamEvent>,
    hub: &StreamHub,
    sessions_store: &kaleido_core::TavernSessionStore,
    jobs: &kaleido_core::JobStore,
) -> Option<(String, u32, String)> {
        // [P7] TurnCtx 收编：散装值参数自 ctx 取出（Copy 先取值、其余借字段引用），
        // 块体与原实现逐行一致。
        let turn_started = ctx.turn_started;
        let turn_started_ms = ctx.turn_started_ms;
        let quality_bg = ctx.quality_bg;
        let resumed_bg = ctx.resumed_bg;
        let next_turn_bg = ctx.next_turn_bg;
        let (base_bg, key_bg, model_bg, fallback_model_bg, prov_bg, sys_prompt_bg, user_bg,
            run_id_bg, session_id_bg, player_msg_bg, used_fallback, extra_llm_calls) =
            (&ctx.base_bg, &ctx.key_bg, &ctx.model_bg, &ctx.fallback_model_bg, &ctx.prov_bg,
             &ctx.sys_prompt_bg, &ctx.user_bg, &ctx.run_id_bg, &ctx.session_id_bg,
             &ctx.player_msg_bg, &mut ctx.used_fallback, &mut ctx.extra_llm_calls);
        // ===== 主回合流式调用（F3/G6 后置项：SSE 解析下沉 llm_stream）=====
        // 单次尝试 = run_main_turn_attempt（内部走 stream_chat_turn_dispatch，
        // 三家协议统一）。错误分类与旧内嵌循环一一映射，UPSTREAM_* 语义码不变。
        let elapsed_ms = || turn_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let mut outcome_opt: Option<crate::llm_stream::TurnStreamOutcome> = None;
        match run_main_turn_attempt(
            &base_bg,
            &key_bg,
            &model_bg,
            &prov_bg,
            &sys_prompt_bg,
            &user_bg,
            &tx,
            &run_id_bg,
            &hub,
        )
        .await
        {
            Ok(o) => outcome_opt = Some(o),
            Err(TurnStreamError::Stopped) => {
                // 中途停止：取消优先（与旧循环顺序一致），其次 U11 超时看门狗。
                if !hub.is_cancelled(&run_id_bg)
                    && turn_over_budget(turn_started_ms, u11_hard_timeout_ms())
                {
                    let _ = tx.send(ChatStreamEvent {
                        run_id: run_id_bg.to_string(),
                        event_type: "error".into(),
                        delta: None,
                        message: Some("turn timeout (U11 budget exceeded)".into()),
                        code: Some("TURN_TIMEOUT".into()),
                        context_compaction: None,
                        input_tokens: None,
                        output_tokens: None,
                    });
                    let _ = jobs.merge_job_payload(
                        &run_id_bg,
                        u11_accounting_json(
                            &model_bg, quality_bg, &sys_prompt_bg, &user_bg, "",
                            *extra_llm_calls, *used_fallback, elapsed_ms(), resumed_bg, 0,
                            None, Some("turn timeout"),
                        ),
                    );
                    jobs.finish(&run_id_bg, "error");
                    clear_session_active_run(&sessions_store, &session_id_bg, Some(&run_id_bg));
                    hub.cleanup(&run_id_bg);
                }
                // 取消路径由下方统一 F4 判定收尾（保持旧语义）。
            }
            Err(TurnStreamError::Connect(e)) => {
                let _ = tx.send(ChatStreamEvent {
                    run_id: run_id_bg.to_string(),
                    event_type: "error".into(),
                    delta: None,
                    message: Some(format!("upstream connect: {e}")),
                    code: Some("UPSTREAM_CONNECT".into()),
                    context_compaction: None,
                    input_tokens: None,
                    output_tokens: None,
                });
                let _ = jobs.merge_job_payload(
                    &run_id_bg,
                    u11_accounting_json(
                        &model_bg, quality_bg, &sys_prompt_bg, &user_bg, "", *extra_llm_calls,
                        *used_fallback, elapsed_ms(), resumed_bg, 0, None, Some("upstream connect"),
                    ),
                );
                jobs.finish(&run_id_bg, "error");
                clear_session_active_run(&sessions_store, &session_id_bg, Some(&run_id_bg));
                hub.cleanup(&run_id_bg);
                return None;
            }
            Err(TurnStreamError::Status { status, body }) => {
                // Model fallback: 上游 4xx 模型类错误（invalid_model / model_not_found /
                // 不可用）→ 用 env 默认模型重试一次。
                if crate::llm_stream::is_model_rejection(status, &body)
                    && !fallback_model_bg.is_empty()
                    && fallback_model_bg != model_bg
                {
                    info!(
                        %run_id_bg,
                        requested = %model_bg,
                        fallback = %fallback_model_bg,
                        "upstream rejected model, retrying with default"
                    );
                    match run_main_turn_attempt(
                        &base_bg,
                        &key_bg,
                        &fallback_model_bg,
                        &prov_bg,
                        &sys_prompt_bg,
                        &user_bg,
                        &tx,
                        &run_id_bg,
                        &hub,
                    )
                    .await
                    {
                        Ok(o) => {
                            outcome_opt = Some(o);
                            *used_fallback = true;
                        }
                        Err(TurnStreamError::Stopped) => {
                            // 回退流中途取消/超时：同主路径 Stopped 处理。
                            if !hub.is_cancelled(&run_id_bg)
                                && turn_over_budget(
                                    turn_started_ms,
                                    u11_hard_timeout_ms(),
                                )
                            {
                                let _ = tx.send(ChatStreamEvent {
                                    run_id: run_id_bg.to_string(),
                                    event_type: "error".into(),
                                    delta: None,
                                    message: Some("turn timeout (U11 budget exceeded)".into()),
                                    code: Some("TURN_TIMEOUT".into()),
                                    context_compaction: None,
                                    input_tokens: None,
                                    output_tokens: None,
                                });
                                let _ = jobs.merge_job_payload(
                                    &run_id_bg,
                                    u11_accounting_json(
                                        &fallback_model_bg, quality_bg, &sys_prompt_bg,
                                        &user_bg, "", *extra_llm_calls, true, elapsed_ms(),
                                        resumed_bg, 0, None, Some("turn timeout"),
                                    ),
                                );
                                jobs.finish(&run_id_bg, "error");
                                clear_session_active_run(
                                    &sessions_store,
                                    &session_id_bg,
                                    Some(&run_id_bg),
                                );
                                hub.cleanup(&run_id_bg);
                            }
                        }
                        Err(TurnStreamError::Status { status: status2, body: text2 }) => {
                            let _ = tx.send(ChatStreamEvent {
                                run_id: run_id_bg.to_string(),
                                event_type: "error".into(),
                                delta: None,
                                message: Some(format!(
                                    "upstream {status2} (model fallback also failed): {}",
                                    text2.chars().take(300).collect::<String>()
                                )),
                                code: Some("UPSTREAM_STATUS_FALLBACK".into()),
                                context_compaction: None,
                                input_tokens: None,
                                output_tokens: None,
                            });
                            let _ = jobs.merge_job_payload(
                                &run_id_bg,
                                u11_accounting_json(
                                    &fallback_model_bg, quality_bg, &sys_prompt_bg, &user_bg,
                                    "", *extra_llm_calls, true, elapsed_ms(), resumed_bg, 0, None,
                                    Some("upstream status (fallback failed)"),
                                ),
                            );
                            jobs.finish(&run_id_bg, "error");
                            clear_session_active_run(
                                &sessions_store,
                                &session_id_bg,
                                Some(&run_id_bg),
                            );
                            hub.cleanup(&run_id_bg);
                            return None;
                        }
                        Err(TurnStreamError::Connect(e)) => {
                            let _ = tx.send(ChatStreamEvent {
                                run_id: run_id_bg.to_string(),
                                event_type: "error".into(),
                                delta: None,
                                message: Some(format!("upstream connect (model fallback): {e}")),
                                code: Some("UPSTREAM_CONNECT_FALLBACK".into()),
                                context_compaction: None,
                                input_tokens: None,
                                output_tokens: None,
                            });
                            let _ = jobs.merge_job_payload(
                                &run_id_bg,
                                u11_accounting_json(
                                    &fallback_model_bg, quality_bg, &sys_prompt_bg, &user_bg,
                                    "", *extra_llm_calls, true, elapsed_ms(), resumed_bg, 0, None,
                                    Some("upstream connect (model fallback)"),
                                ),
                            );
                            jobs.finish(&run_id_bg, "error");
                            clear_session_active_run(
                                &sessions_store,
                                &session_id_bg,
                                Some(&run_id_bg),
                            );
                            hub.cleanup(&run_id_bg);
                            return None;
                        }
                        Err(TurnStreamError::Stream(e)) => {
                            warn!(error = %e, "tavern stream error (model fallback)");
                            let _ = tx.send(ChatStreamEvent {
                                run_id: run_id_bg.to_string(),
                                event_type: "error".into(),
                                delta: None,
                                message: Some(e.clone()),
                                code: Some("UPSTREAM_STREAM".into()),
                                context_compaction: None,
                                input_tokens: None,
                                output_tokens: None,
                            });
                            let _ = jobs.merge_job_payload(
                                &run_id_bg,
                                u11_accounting_json(
                                    &fallback_model_bg, quality_bg, &sys_prompt_bg, &user_bg,
                                    "", *extra_llm_calls, true, elapsed_ms(), resumed_bg, 0, None,
                                    Some("stream error"),
                                ),
                            );
                            jobs.finish(&run_id_bg, "error");
                            clear_session_active_run(
                                &sessions_store,
                                &session_id_bg,
                                Some(&run_id_bg),
                            );
                            hub.cleanup(&run_id_bg);
                            return None;
                        }
                    }
                } else {
                    let _ = tx.send(ChatStreamEvent {
                        run_id: run_id_bg.to_string(),
                        event_type: "error".into(),
                        delta: None,
                        message: Some(format!(
                            "upstream {status}: {}",
                            body.chars().take(300).collect::<String>()
                        )),
                        code: Some("UPSTREAM_STATUS".into()),
                        context_compaction: None,
                        input_tokens: None,
                        output_tokens: None,
                    });
                    let _ = jobs.merge_job_payload(
                        &run_id_bg,
                        u11_accounting_json(
                            &model_bg, quality_bg, &sys_prompt_bg, &user_bg, "", *extra_llm_calls,
                            *used_fallback, elapsed_ms(), resumed_bg, 0, None, Some("upstream status"),
                        ),
                    );
                    jobs.finish(&run_id_bg, "error");
                    clear_session_active_run(&sessions_store, &session_id_bg, Some(&run_id_bg));
                    hub.cleanup(&run_id_bg);
                    return None;
                }
            }
            Err(TurnStreamError::Stream(e)) => {
                warn!(error=%e, "tavern stream error");
                let _ = tx.send(ChatStreamEvent {
                    run_id: run_id_bg.to_string(),
                    event_type: "error".into(),
                    delta: None,
                    message: Some(e.to_string()),
                    code: Some("UPSTREAM_STREAM".into()),
                    context_compaction: None,
                    input_tokens: None,
                    output_tokens: None,
                });
                let _ = jobs.merge_job_payload(
                    &run_id_bg,
                    u11_accounting_json(
                        &model_bg, quality_bg, &sys_prompt_bg, &user_bg, "", *extra_llm_calls,
                        *used_fallback, elapsed_ms(), resumed_bg, 0, None, Some("stream error"),
                    ),
                );
                jobs.finish(&run_id_bg, "error");
                clear_session_active_run(&sessions_store, &session_id_bg, Some(&run_id_bg));
                hub.cleanup(&run_id_bg);
                return None;
            }
        }

        // F4: If client disconnected during streaming, stop processing immediately.
        if hub.is_cancelled(&run_id_bg) {
            jobs.finish(&run_id_bg, "cancelled");
            clear_session_active_run(&sessions_store, &session_id_bg, Some(&run_id_bg));
            hub.cleanup(&run_id_bg);
            return None;
        }

        let outcome = match outcome_opt {
            Some(o) => o,
            None => {
                // Stopped 且非取消标志：可达路径 —— hub.cancel() 在发完 STOPPED 后立即
                // cleanup()（移除 cancelled 标志与 senders），worker 随后的 send 失败 /
                // 取消探测只能看到“已清理”状态。保守按取消收尾（R2 编排测试实证）。
                jobs.finish(&run_id_bg, "cancelled");
                clear_session_active_run(&sessions_store, &session_id_bg, Some(&run_id_bg));
                hub.cleanup(&run_id_bg);
                return None;
            }
        };
        let mut full_text = outcome.text;
        // [token 显示 2026-08-16] usage.total_tokens 随 asst_msg.tokens 落盘，
        // 前端 stMsgMeta 显示。现由 llm_stream 统一捕获。
        let mut usage_tokens: u32 = outcome.total_tokens.unwrap_or(0).min(u32::MAX as u64) as u32;
        // 模型推理内容独立累积，随 asst_msg.reasoning 落盘，前端折叠箭头展示。
        let mut thinking_text = outcome.thinking;

        // Append assistant message to session + run extraction
        if full_text.is_empty() {
            let cancelled = hub.is_cancelled(&run_id_bg);
            // 上游空响应重试 1 次（zen 池忙/限流时空流常见，实测 18s 后 empty response）。
            // 重试成功则继续走正常路径（质量管道 + 落盘）；仍空才走失败。
            // F3：重试复用同一 dispatch 入口 —— 相比旧内嵌简化解析，思维链/usage 也一并捕获。
            if !cancelled
                && !base_bg.trim().is_empty()
                && !key_bg.trim().is_empty()
            {
                *extra_llm_calls += 1;
                info!(%run_id_bg, "empty response, retrying once");
                match run_main_turn_attempt(
                    &base_bg,
                    &key_bg,
                    &model_bg,
                    &prov_bg,
                    &sys_prompt_bg,
                    &user_bg,
                    &tx,
                    &run_id_bg,
                    &hub,
                )
                .await
                {
                    Ok(o) if !o.text.trim().is_empty() => {
                        full_text = o.text;
                        thinking_text = o.thinking;
                        if usage_tokens == 0 {
                            usage_tokens =
                                o.total_tokens.unwrap_or(0).min(u32::MAX as u64) as u32;
                        }
                        info!(%run_id_bg, "empty-retry succeeded");
                    }
                    Ok(_) => {
                        warn!(%run_id_bg, "empty-retry still empty");
                    }
                    Err(e) => {
                        warn!(error = %e.message(), "empty-retry failed");
                    }
                }
            }
            if full_text.is_empty() {
                clear_session_active_run(&sessions_store, &session_id_bg, Some(&run_id_bg));
                let cancelled = hub.is_cancelled(&run_id_bg);
                let _ = tx.send(ChatStreamEvent {
                    run_id: run_id_bg.to_string(),
                    event_type: if cancelled { "done".into() } else { "error".into() },
                    delta: None,
                    message: Some(if cancelled {
                        "turn cancelled".into()
                    } else {
                        "empty model response".into()
                    }),
                    code: if cancelled { None } else { Some("EMPTY_RESPONSE".into()) },
                    context_compaction: None,
                    input_tokens: None,
                    output_tokens: None,
                });
                // U11: 空响应/取消路径也记账（耗时/调用次数）
                let _ = jobs.merge_job_payload(
                    &run_id_bg,
                    u11_accounting_json(
                        &model_bg, quality_bg, &sys_prompt_bg, &user_bg, "", *extra_llm_calls,
                        *used_fallback, elapsed_ms(), resumed_bg, 0, None,
                        Some(if cancelled { "cancelled" } else { "empty response" }),
                    ),
                );
                jobs.finish(
                    &run_id_bg,
                    if cancelled { "cancelled" } else { "error" },
                );
                hub.cleanup(&run_id_bg);
                return None;
            }
        }
        // §13.5 Scene Gate (2026-08-18): 首回合场景错位单次纠偏重试。
        // 实证：度蜜月首回合 opening 注入学校摘要+硬锚，flash-free 仍写成酒店/机场
        //（标题《代替父亲和妈妈度蜜月》的先验过强，prompt 文本压不住）。
        // 确定性检测（is_scene_mismatch_location：酒店/套房/机场/三亚等地点词）→
        // 用主线 LLM 配置带纠偏指令重试一次；二次仍错则接受原文+warn（不无限循环）。
        if next_turn_bg <= 1 && is_scene_mismatch_location(&full_text) {
            let corrected_user = format!(
                "【系统纠偏】你上一版开场把场景写错了：现在仍在当前章节注入的场景内\n（如学校/家中客厅，见系统开场摘录），你却写成了酒店/机场/三亚等后续地点。\n请重写本回合正文，从正确的当前场景开始，保留玩家指令的语义：{}。",
                player_msg_bg
            );
            let retry_msgs = vec![
                json!({"role": "system", "content": sys_prompt_bg}),
                json!({"role": "user", "content": corrected_user}),
            ];
            match crate::llm_stream::stream_chat_completions_msgs(
                &base_bg, &key_bg, &model_bg, retry_msgs, 0.1, 8192, 60, |_| true,
            )
            .await
            {
                Ok(rep) if !rep.trim().is_empty() => {
                    let rep = rep.trim().to_string();
                    if !is_scene_mismatch_location(&rep) {
                        full_text = rep;
                        thinking_text.clear();
                        info!(%run_id_bg, "scene-gate corrected first-turn scene");
                    } else {
                        warn!(%run_id_bg, "scene-gate retry still mismatched, keeping original");
                    }
                }
                Ok(_) => warn!(%run_id_bg, "scene-gate retry empty, keeping original"),
                Err(e) => warn!(%run_id_bg, err = %e, "scene-gate retry failed"),
            }
        }
        Some((full_text, usage_tokens, thinking_text))
}

