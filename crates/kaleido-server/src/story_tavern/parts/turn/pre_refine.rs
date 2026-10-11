//! [2026-10-06 P6] //! 质量管道前置（协议块预剥离 + run_quality_refine 多轮协作）。
#![allow(unused_imports)]
use super::super::*;

#[allow(clippy::too_many_arguments)]
/// [P5 2026-10-05 从 start_turn 内联提取] 预剥离 stash + 质量管道（heavy/standard）。
/// full_text/thinking_pipe 经 std::mem::take 换入换出，块体逐字节不变；
/// 返回 9 元组 stash（调用点解构为原名，下游零改动）。
pub(crate) async fn run_pre_refine_phase(
    full_text: &mut String,
    thinking_pipe: &mut String,
    sessions_store: &kaleido_core::TavernSessionStore,
    jobs: &kaleido_core::JobStore,
    quality_bg: TurnQuality,
    base_bg: &str,
    key_bg: &str,
    model_bg: &str,
    prov_bg: &str,
    skill_bg: Option<&SkillDoc>,
    data_root_bg: &std::path::Path,
    workspace_id_bg: &str,
    user_bg: &str,
    sys_prompt_bg: &str,
    session_id_bg: &str,
    next_turn_bg: u32,
    run_id_bg: &str,
    turn_started_ms: u64,
) -> (Vec<String>, Vec<kaleido_core::TavernPanel>, Option<String>, Vec<crate::tavern_mcp::McpCall>, Vec<kaleido_core::ActorStateUpdate>, Vec<kaleido_core::TurnCheckRequest>, Option<DirectorPlanUpdate>, Option<String>, TurnReceipt) {
    let mut full_text_local = std::mem::take(full_text);
    let thinking_pipe_handle = thinking_pipe;
    let mut thinking_pipe = std::mem::take(thinking_pipe_handle);
            let mut stash_options: Vec<String> = Vec::new();
            let mut stash_panels: Vec<kaleido_core::TavernPanel> = Vec::new();
            let mut stash_program: Option<String> = None;
            let mut stash_mcp: Vec<crate::tavern_mcp::McpCall> = Vec::new();
            let mut stash_state: Vec<kaleido_core::ActorStateUpdate> = Vec::new();
            let mut stash_check: Vec<kaleido_core::TurnCheckRequest> = Vec::new();
            let mut stash_plan: Option<DirectorPlanUpdate> = None;
            let mut stash_advance: Option<String> = None;
            // 【回执】单一 JSON 尾块（文本协议根治 2026-10）：pre-refine 预剥离的回执，
            // 字段优先级低于管道后 post 解析、高于散装标记 stash（见 p010 模块注释）。
            let mut stash_receipt = TurnReceipt::default();
            // U11: 预算守卫 —— 接近硬超时则跳过质量管道（保留初稿，best-effort 不阻塞正文）。
            let refine_budget_ok = !turn_over_budget(turn_started_ms, u11_hard_timeout_ms());
            // 管道内剥离的思维段（fix 阶段 <thinking> 块）汇入，最终并入 monologue 折叠展示。
            // 定义在 if 块外：lite/预算不足跳过管道时保持空串，合并区仍可安全引用。
            if quality_bg != TurnQuality::Lite
                && !base_bg.trim().is_empty()
                && !key_bg.trim().is_empty()
                && refine_budget_ok
            {
                // P3 编排（I2）：质量管道入口阶段检查点 —— 中断诊断可判「死在管道内哪一步」。
                let _ = sessions_store.update_session(session_id_bg, |live| {
                    live.set_turn_progress(next_turn_bg, TurnPhase::Quality, run_id_bg, "quality refine");
                    Ok(())
                });
                let _ = jobs.push_event(
                    run_id_bg,
                    kaleido_core::JobEvent::progress("quality: refine pipeline", 0.5),
                    Some(0.5),
                    Some("phase:quality".to_string()),
                );
                let refine_llm = RemoteQualityLlm {
                    base_url: base_bg,
                    api_key: key_bg,
                    model: model_bg,
                    provider_kind: prov_bg,
                };
                // 预剥离【技能加载】块：heavy/standard 多轮管道会吞掉该块导致 skill_load
                // 永不装载。先按当前档位装载完整 SKILL.md 存入会话（此分支必非 lite），
                // 剥离后的文本才进入 run_quality_refine。
                let (pre_skill_text, skill_calls) =
                    crate::skill_layer::split_skill_load_calls_from_narrative(&full_text_local);
                if !skill_calls.is_empty() {
                    let tier = crate::skill_layer::resolve_tier_for_quality(quality_bg);
                    if let Some(doc) = crate::skill_layer::load_writing_skill(
                        data_root_bg,
                        Some(workspace_id_bg),
                        tier,
                    ) {
                        if let Ok(mut sess_skill) = sessions_store.get(session_id_bg) {
                            sess_skill.skill_load = Some(kaleido_core::SkillLoadInfo {
                                tier: tier.to_string(),
                                markdown: crate::skill_layer::skill_full_markdown(&doc),
                            });
                            if let Err(e) = sessions_store.save(sess_skill.clone()) {
                                warn!(error = %e, "st skill load persist failed");
                            }
                            tracing::info!(n = skill_calls.len(), tier, "st skill load injected pre-refine");
                        }
                    }
                    full_text_local = pre_skill_text;
                }
                // 协议块预剥离（bugC 修复）：heavy/standard 多轮管道 write 阶段会重写
                // 正文、丢掉主调用的协议块，因此在进入 run_quality_refine 前按序剥离
                // 全部协议块并 stash；管道只收纯正文。skill 预剥离已在上方完成（不动）。
                // [fix §10.4 2026-08-16] 开头推理剥离下沉公共链：strip_lite_reasoning_leak
                // 原只在 run_quality_refine（质量管道）调用，Lite 直出路径完全缺失——
                // 实踩 msg「让我理清当前情况。玩家选择了…」整段推理出现在正文首段。
                full_text_local = strip_lite_reasoning_leak(full_text_local);
                // 【回执】单一 JSON 尾块预剥离（文本协议根治 2026-10）：先剥回执再剥散装
                // 标记，质量管道只收纯正文；回执字段 stash 后在管道后与 post 解析合并。
                let (receipt_clean, pre_receipt) = split_turn_receipt_from_narrative(&full_text_local);
                full_text_local = receipt_clean;
                stash_receipt = pre_receipt.unwrap_or_default();
                let (pre_clean, pre_options) = split_options_from_narrative(&full_text_local);
                full_text_local = pre_clean;
                stash_options = pre_options;
                if !stash_options.is_empty() {
                    tracing::info!(n = stash_options.len(), "st options pre-refine");
                }

                let (pre_clean_p, pre_panels) = split_panels_from_narrative(&full_text_local);
                full_text_local = pre_clean_p;
                stash_panels = pre_panels;
                if !stash_panels.is_empty() {
                    tracing::info!(n = stash_panels.len(), "st panels pre-refine");
                }

                let (pre_clean_pr, pre_program) = split_program_from_narrative(&full_text_local);
                full_text_local = pre_clean_pr;
                stash_program = pre_program;
                if stash_program.is_some() {
                    tracing::info!("st program pre-refine");
                }

                let (pre_clean_m, pre_mcp) =
                    crate::tavern_mcp::split_mcp_calls_from_narrative(&full_text_local);
                full_text_local = pre_clean_m;
                stash_mcp = pre_mcp;
                if !stash_mcp.is_empty() {
                    tracing::info!(n = stash_mcp.len(), "st mcp pre-refine");
                }

                let (pre_clean_s, pre_state) = split_state_updates_from_narrative(&full_text_local);
                full_text_local = pre_clean_s;
                stash_state = pre_state;
                if !stash_state.is_empty() {
                    tracing::info!(n = stash_state.len(), "st state updates pre-refine");
                }

                let (pre_clean_c, pre_check) = split_check_from_narrative(&full_text_local);
                full_text_local = pre_clean_c;
                stash_check = pre_check;
                if !stash_check.is_empty() {
                    tracing::info!(n = stash_check.len(), "st rule checks pre-refine");
                }

                let (pre_clean_pl, pre_plan) = split_director_plan_from_narrative(&full_text_local);
                full_text_local = pre_clean_pl;
                stash_plan = pre_plan;
                if stash_plan.is_some() {
                    tracing::info!("st director plan pre-refine");
                }

                // ST-15: 【节点推进】marker 预剥离（同 3967-3978 逻辑）
                // [ST-15 fix 2026-08-16] 用 extract_advance_marker 兼容【节点推进:n2】格式
                if let Some((adv_pos, node_id)) = extract_advance_marker(&full_text_local) {
                    tracing::info!(%node_id, "st llm advance pre-refine");
                    stash_advance = Some(node_id);
                    full_text_local = full_text_local[..adv_pos].to_string() + " ";
                }
                // X2a (吞噬自 xiami skimming.rs): 读者速读质检进质量管道 —— 审稿环节前做
                // 确定性检查，速读风险问题文案并入审稿/修复 prompt（QUALITY_FIX 流程生效）；
                // 仅附加信息，失败/发现问题均不阻断回合。
                let skim_config = reader_skimming_config();
                let pre_skim_issues = analyze_skimming(&full_text_local, &skim_config);
                let mut refine_sys_bg = sys_prompt_bg.to_string();
                if !pre_skim_issues.is_empty() {
                    let skim_block = render_skim_issues_for_prompt(&pre_skim_issues);
                    refine_sys_bg = format!(
                        "{}\n\n## 读者速读质检问题（审稿时纳入考量）\n{}",
                        refine_sys_bg, skim_block
                    );
                    tracing::info!(
                        n = pre_skim_issues.len(),
                        "st skim: 速读质检前置问题并入审稿 prompt（{} 个）",
                        pre_skim_issues.len()
                    );
                }
                // 管道内剥离的思维段在 if 块外定义（thinking_pipe），此处直接引用。
                match run_quality_refine(
                    quality_bg,
                    &refine_llm,
                    &refine_sys_bg,
                    user_bg,
                    &full_text_local,
                    skill_bg,
                    &mut thinking_pipe,
                )
                .await
                {
                    Ok((revised, memory_patch)) if !revised.trim().is_empty() => {
                        tracing::info!(quality = ?quality_bg, "st quality refine applied");
                        full_text_local = revised;
                        // P4: heavy MemoryPatch 回写 actor_states（best-effort，失败仅日志）
                        if let Some(patch) = memory_patch {
                            if let Ok(mut sess_mem) = sessions_store.get(session_id_bg) {
                                let n = crate::skill_layer::apply_memory_patch_to_states(
                                    &mut sess_mem.actor_states,
                                    &patch,
                                );
                                if n > 0 {
                                    tracing::info!(n, "st heavy memory patch applied to actor states");
                                }
                                let _ = sessions_store.save(sess_mem);
                            }
                        }
                    }
                    Ok(_) => {
                        tracing::warn!(quality = ?quality_bg, "st quality refine empty; keeping draft");
                    }
                    Err(e) => {
                        tracing::warn!(quality = ?quality_bg, error=%e, "st quality refine failed; keeping draft");
                    }
                }
            }
    *full_text = full_text_local;
    *thinking_pipe_handle = thinking_pipe;
    (stash_options, stash_panels, stash_program, stash_mcp, stash_state, stash_check, stash_plan, stash_advance, stash_receipt)
}

