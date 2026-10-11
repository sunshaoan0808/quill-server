//! [2026-10-06 P6] //! 回合组装与持久化收口（assemble_and_persist_turn）。
#![allow(unused_imports)]
use super::super::*;

pub(crate) async fn assemble_and_persist_turn(
    sess: &mut kaleido_core::TavernSession,
    sessions_store: &kaleido_core::TavernSessionStore,
    packs_store: &kaleido_core::PackStore,
    jobs: &kaleido_core::JobStore,
    tx: &tokio::sync::broadcast::Sender<ChatStreamEvent>,
    state: &AppState,
    pack: &StoryPack,
    full_text: &str,
    player_msg_bg: &str,
    user_bg: &str,
    engine_tag: kaleido_core::EngineTag,
    thinking_text: &str,
    thinking_pipe: &str,
    merged_receipt: &TurnReceipt,
    check_requests: &[kaleido_core::TurnCheckRequest],
    options: &[String],
    panels: &[kaleido_core::TavernPanel],
    program: Option<String>,
    mcp_results: Vec<kaleido_core::ToolResultBrief>,
    state_updates: Vec<kaleido_core::ActorStateUpdate>,
    director_plan_update: Option<DirectorPlanUpdate>,
    director_instruct_bg: bool,
    usage_tokens: u32,
    session_id_bg: &str,
    next_turn_bg: u32,
    run_id_bg: &str,
) {
            // X2a (吞噬自 xiami skimming.rs): 读者速读质检 —— 正文定稿后做确定性检查。
            // SkimIssue（severity 1/2）转 tracing::warn 日志；仅附加信息，不阻断回合、不写回 session。
            let skim_issues = analyze_skimming(full_text, &reader_skimming_config());
            for issue in &skim_issues {
                tracing::warn!(
                    severity = issue.severity,
                    category = %issue.category,
                    "st skim 速读质检: {}（{}）", issue.message, issue.evidence
                );
            }
            if !skim_issues.is_empty() {
                tracing::info!(
                    n = skim_issues.len(),
                    "st skim: 速读质检发现 {} 个问题（诊断记录，不阻断回合）",
                    skim_issues.len()
                );
            }

                // X3: 把 X2a 速读质检结果写回 session（诊断展示用，不阻断回合）。
                // 空结果也写（清空旧记录），由 sessions_store.save 在回合终态落盘。
                sess.skim_issues = skim_issues;
                sess.skim_sample = full_text.chars().take(200).collect();
                // P3 编排（I2）：Persisting 阶段检查点 —— 正文已定稿、即将组装 assistant
                // 消息并推进 turn；中断在此后可判「正文已产出，死在落盘/后处理」。
                sess.set_turn_progress(next_turn_bg, TurnPhase::Persisting, run_id_bg, "persist assistant msg");

                // ST-27: 规则检定执行（rule_system 从当前 pack 宽容解析；无则剥离块但不执行）。
                // [P5 2026-10-05] 主体提取为同文件私有 fn execute_rule_checks（行为逐字节不变）。
                let check_result_text = execute_rule_checks(sess, check_requests, packs_store);
                // 正文开头「导演自白」剥离：模型偶发把「好的，我是XX…我要怎么演」
                // 之类的内部推理写进正文开头（TEN_ROUND_PLOT_VERIFY R4/R7/R8 泄漏）。
                // 剥离段归入 reasoning 折叠展示，正文保持干净。仅匹配明确的自白前缀，
                // 不误伤正常叙事（正常正文不以「好的，我是」开头）。
                let mut story_body = format!("{}{}", full_text, check_result_text);
                let mut reasoning_merged = thinking_text.trim().to_string();
                // [fix 2026-08-15 结构根治] 统一入口结构剥离：对最终正文再做一次
                // <thinking>/<story> 标签解析（覆盖 lite 直出 / 管道失败保留初稿路径——
                // 主回合正文若被模型写入 thinking 块同样折叠，不落正文）。
                append_thinking(&mut reasoning_merged, thinking_pipe);
                append_thinking(
                    &mut reasoning_merged,
                    &strip_fix_thinking_blocks(&mut story_body),
                );
                // [修复 2026-08-15] Heavy 管道 fix 思维前缀剥离（关键词兜底，旧模型
                // 无标签输出；新结构输出已由 strip_fix_thinking_blocks 处理）。
                let stripped_fix = strip_heavy_fix_preamble(&mut story_body);
                if !stripped_fix.is_empty() {
                    append_thinking(&mut reasoning_merged, &stripped_fix);
                }
                // [fix 2026-08-15] 尾部自检剥离（模型末尾追加「让我再检查/再检查是否需要」段）。
                let stripped_tail = strip_heavy_fix_tail(&mut story_body);
                if !stripped_tail.is_empty() {
                    append_thinking(&mut reasoning_merged, &stripped_tail);
                }
                let stripped = strip_director_preamble(&mut story_body);
                if !stripped.is_empty() {
                    append_thinking(&mut reasoning_merged, &stripped);
                }
                let reasoning_final = if reasoning_merged.is_empty() {
                    None
                } else {
                    Some(reasoning_merged)
                };

                // [Swipe] 继承 pending_swipes（reroll 旧正文），去重后挂载
                let mut inherited_swipes = std::mem::take(&mut sess.pending_swipes);
                inherited_swipes.retain(|s| s != &story_body);
                let asst_msg = TavernMessage {
                    id: format!("msg-{}", Uuid::new_v4()),
                    role: "assistant".into(),
                    content: story_body,
                    created_at: Utc::now().to_rfc3339(),
                    options: options.to_vec(),
                    engine_tag: Some(engine_tag),
                    program,
                    reasoning: reasoning_final,
                    // [token 显示 2026-08-16] 上游 usage.total_tokens（无则 0，前端不显示）
                    swipes: inherited_swipes,
                    swipe_index: 0,
                    tokens: usage_tokens,
                };
                sess.messages.push(asst_msg);
                sess.turn += 1;
                // [世界书定时] 每回合 tick：sticky 到期→转 cooldown；新激活条目 record。
                {
                    let clen = sess.messages.len() as i32;
                    // collect live lore keys for cooldown lookup
                    let lore_now_all = filter_lore_entries(&pack.lore_entries, sess.chapter_cursor.as_deref().unwrap_or(""), sess.node_id.as_deref().unwrap_or(""));
                    let lore_now: Vec<&serde_json::Value> = lore_now_all.into_iter().filter(|e| lore_trigger_ok(e, &sess)).collect();
                    let by_key: std::collections::HashMap<String, &serde_json::Value> = lore_now.iter().map(|e| {
                        let title = e.get("title").and_then(|v| v.as_str()).unwrap_or("");
                        let key = if title.is_empty() { format!("pack.{}", e.get("id").and_then(|v| v.as_str()).unwrap_or("?")) } else { format!("pack.{}", title) };
                        (key, *e)
                    }).collect();
                    // tick: expire + sticky→cooldown (protected)
                    let mut ended: Vec<String> = vec![];
                    for (k, eff) in sess.timed_world_info.sticky.iter() {
                        if clen >= eff.end || clen < eff.start { ended.push(k.clone()); }
                    }
                    for k in ended {
                        if let Some(eff) = sess.timed_world_info.sticky.remove(&k) {
                            let rewound = clen < eff.start;
                            if !rewound {
                                if let Some(entry) = by_key.get(&k) {
                                    let cd = entry.get("cooldown").or_else(|| entry.get("extensions").and_then(|x| x.get("cooldown"))).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                                    if cd > 0 {
                                        sess.timed_world_info.cooldown.insert(k.clone(), kaleido_core::WiTimedEffect { key: k.clone(), start: clen, end: clen + cd, protected: true });
                                    }
                                }
                            }
                        }
                    }
                    sess.timed_world_info.cooldown.retain(|_, eff| clen < eff.end && (clen >= eff.start || eff.protected));
                    // record newly activated (chapter-matched, not suppressed, has sticky/cooldown)
                    for (key, entry) in by_key {
                        if sess.timed_world_info.sticky.contains_key(&key) || sess.timed_world_info.cooldown.contains_key(&key) { continue; }
                        let st = entry.get("sticky").or_else(|| entry.get("extensions").and_then(|x| x.get("sticky"))).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                        let cd = entry.get("cooldown").or_else(|| entry.get("extensions").and_then(|x| x.get("cooldown"))).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                        if st > 0 {
                            sess.timed_world_info.sticky.insert(key.clone(), kaleido_core::WiTimedEffect { key, start: clen, end: clen + st, protected: false });
                        } else if cd > 0 {
                            sess.timed_world_info.cooldown.insert(key.clone(), kaleido_core::WiTimedEffect { key, start: clen, end: clen + cd, protected: false });
                        }
                    }
                }
                // [吞噬系统 auto-tick] 每回合末自动推进（与世界书定时同位置）。
                // Needs 衰减（天气联动）+ 灾变由 tick_decay 内部触发。
                {
                    let rough = sess.world_climate.atmosphere != kaleido_core::world_climate::WorldAtmosphere::Breathable;
                    let clear = sess.game_clock.weather == "晴";
                    // Needs 自动建档（present 新角色补默认）
                    for cid in sess.present_character_ids.clone() {
                        sess.needs.entry(cid).or_insert_with(kaleido_core::needs::Needs::default);
                    }
                    for needs in sess.needs.values_mut() { needs.tick_decay(rough, clear); }
                    // [fix chaos 交付/清场] 本回合 system prompt 已注入的命运事件（上回合末或
                    // manual tick arm、build_tavern_system_prompt prompt_injection 注入）：
                    // 回合正文已成功落盘即视为已交付 —— consume_pending 置 delivered，解除
                    // tick/should_trigger 的 pending 阻塞（chaos.rs:34/40）并停止后续 prompt
                    // 重复注入；下一用户回合由 start_turn 的 clear_delivered_if_any 清场。
                    // consume_pending 内部按 has_pending 门控：无未交付 pending 时不动
                    // delivered，避免空 delivered 触发 clear 的 pressure 重置。
                    // 必须在下方 tick/roll/arm 之前执行，防止把本回合末刚 arm 的新事件误标记。
                    if sess.chaos.consume_pending().is_some() {
                        tracing::debug!(%session_id_bg, "chaos event delivered this turn");
                    }
                    // Chaos 压力（启用时；pending 未交付时 tick 内部跳过）
                    if sess.chaos.enabled {
                        sess.chaos.tick();
                        // roll → arm（用 turn 做确定性 roll，避免 rand 依赖；阈值内触发）
                        let roll = (sess.turn.wrapping_mul(2654435761) >> 8) % 100;
                        if sess.chaos.should_trigger(roll as u32) && !sess.chaos.has_pending() {
                            let char_name = sess.present_character_ids.first()
                                .and_then(|cid| pack.characters.iter().find(|c| c.id == *cid).map(|c| c.name.clone()))
                                .unwrap_or_else(|| "角色".into());
                            let pool_len = kaleido_core::chaos::chance_pool().len();
                            sess.chaos.arm_event(&char_name, (sess.turn as usize) % pool_len.max(1));
                        }
                    }
                    // 羁绊衰减（10 回合一步）
                    for b in sess.relationships.values_mut() { b.maybe_decay(); }
                    // Journal 冷却（在场角色）
                    let sid_c = sess.session_id.clone();
                    for cid in sess.present_character_ids.clone() { sess.journal.cool(&sid_c, &cid); }
                    // Growth 褪色（30 回合）
                    sess.growth.fade_old(sess.turn, 30);
                    // Dreams 跨夜检测（day 推进即 rollover）
                    sess.dream.check_rollover(Some(&sess.session_id.clone()), sess.game_clock.day as i32, true);
                }
                // [时间天气系统 v2] 回合结束推进权威时钟。
                // [P5 2026-10-05] 主体提取为同文件私有 async fn advance_game_clock（行为逐字节不变）。
                let _time_jump_desc = advance_game_clock(
                    state,
                    sess,
                    player_msg_bg,
                    full_text,
                    merged_receipt.time_advance.as_deref(),
                ).await;
                sess.active_run_id = None;
                // S4: 应用本回合导演计划块 + 消费导演挂起标记
                // [P5 2026-10-05] 提取为同文件私有 fn apply_director_plan_update（行为逐字节不变）。
                apply_director_plan_update(sess, director_plan_update, director_instruct_bg);
                // 面板回写（吸收自梨园 panels.ts）：同名更新，软上限6已在提取时截断
                if !panels.is_empty() {
                    for p in panels {
                        if let Some(existing) = sess.panels.iter_mut().find(|x| x.name == p.name) {
                            *existing = p.clone();
                        } else {
                            sess.panels.push(p.clone());
                        }
                    }
                }
                // MCP 外设结果回填（下轮 build 注入，供叙事取材）
                if !mcp_results.is_empty() {
                    sess.mcp_tool_results = mcp_results;
                }
                // ST-26: 应用并落盘 actor 状态
                let updated = sess.actor_states.apply_updates(&state_updates);
                if updated > 0 {
                    tracing::info!(updated, "st actor state applied");
                }
                // 账本写入链路（P0 补全）: 回合正文启发式提取 → Liyuan 账本（ledger.json）。
                // 零 LLM、fail-open，不阻塞回合；下回合 preload_asset_blocks 注入。
                // F3 修复：旧代码依赖 spawn 内 `let body=json!()` 的遮蔽取值，
                // body.get("message") 恒为空串；改用捕获的玩家原始消息（该变量本为此目的）。
                let turn_body = sess.messages.last().map(|m| m.content.as_str()).unwrap_or("");
                let user_text = player_msg_bg;
                ledger_upsert_from_turn(state, &pack, &sess, turn_body, user_text);
                // Save before done under the store lock; never write a stale worker snapshot
                // over progress updates or the acquire_turn revision.
                if let Err(e) = sessions_store.update_session(session_id_bg, |live| {
                    live.messages = sess.messages.clone();
                    live.checkpoints = sess.checkpoints.clone();
                    live.turn = sess.turn;
                    live.memory_l1 = sess.memory_l1.clone();
                    live.memory_l2 = sess.memory_l2.clone();
                    live.memory_l3 = sess.memory_l3.clone();
                    live.actor_states = sess.actor_states.clone();
                    live.panels = sess.panels.clone();
                    live.mcp_tool_results = sess.mcp_tool_results.clone();
                    live.director_pending = sess.director_pending;
                    live.updated_at = sess.updated_at.clone();
                    live.active_run_id = None;
                    live.set_turn_progress(sess.turn, TurnPhase::Done, run_id_bg, "turn complete");
                    Ok(())
                }) {
                    tracing::warn!(error = %e, "failed to merge session before done");
                }
                // [全自动事件提取] 回合末后台 LLM：物品/承诺/成长/羁绊直写（默认开，小模型低 token）。
                // 后台执行不阻断 done；fresh 读 + CAS 写回；失败静默（下回合 prompt 仍有全量状态）。
                if sess.event_extract {
                    let store_ex = sessions_store.clone();
                    let appst_ex = state.app_state.clone();
                    let base_ex = state.llm_base.clone();
                    let key_ex = state.llm_key.clone();
                    let model_ex = state.llm_model.clone();
                    let sid_ex = session_id_bg.to_string();
                    let turn_ex = sess.turn;
                    let user_ex = user_bg.to_string();
                    let full_ex = full_text.to_string();
                    let present_ex = sess.present_character_ids.clone();
                    let focus_ex = sess.focus_character_id.clone().unwrap_or_default();
                    let pack_chars_ex: Vec<(String,String)> = pack.characters.iter().map(|c| (c.id.clone(), c.name.clone())).collect();
                    tokio::spawn(async move {
                        tracing::info!(%sid_ex, turn = turn_ex, "st event_extract bg: start");
                        match run_event_extract(&store_ex, &appst_ex, base_ex.as_deref(), key_ex.as_deref(), &model_ex, &sid_ex, turn_ex, &user_ex, &full_ex, &present_ex, &focus_ex, &pack_chars_ex).await {
                            Ok(_) => tracing::info!(%sid_ex, "st event_extract bg: done"),
                            Err(e) => tracing::warn!(error = %e, %sid_ex, "st event_extract bg: failed, non-blocking"),
                        }
                    });
                }
                
                // Finish: send done event
                let _ = tx.send(ChatStreamEvent {
                    run_id: run_id_bg.to_string(),
                    event_type: "done".into(),
                    delta: None,
                    message: None,
                    context_compaction: None,
                    input_tokens: None,
                    output_tokens: None,
                    code: None,
                });
                jobs.finish(run_id_bg, "done");
                
                // P3 编排（I2/I3）：Done 阶段检查点 + job 进度事件收口。
                sess.set_turn_progress(sess.turn, TurnPhase::Done, run_id_bg, "turn complete");
}

