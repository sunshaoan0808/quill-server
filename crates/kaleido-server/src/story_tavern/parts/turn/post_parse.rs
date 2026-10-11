//! [2026-10-06 P6] //! 导演计划拆分/应用 + 管道后解析（run_post_parse_phase）。
#![allow(unused_imports)]
use super::super::*;

pub(crate) fn split_director_plan_from_narrative(full: &str) -> (String, Option<DirectorPlanUpdate>) {
    const PLAN_MARKER: &str = "【导演计划】";
    let Some(mark) = full.find(PLAN_MARKER) else {
        return (full.trim().to_string(), None);
    };
    let before = full[..mark].trim_end();
    let after = &full[mark + PLAN_MARKER.len()..];
    let trimmed = after.trim_start();
    let lead = after.len() - trimmed.len();

    // 【导演计划】none —— 无需更新
    if trimmed.starts_with("none") {
        let rest = trimmed["none".len()..].trim_start();
        let text = if before.is_empty() {
            rest.to_string()
        } else {
            format!("{before}\n{rest}")
        };
        return (text.trim().to_string(), Some(DirectorPlanUpdate::Skip));
    }

    // 括号深度匹配 JSON（同【状态更新】）
    let Some(open) = trimmed.find('{') else {
        return (full.trim().to_string(), None);
    };
    let json_start = mark + PLAN_MARKER.len() + lead + open;
    let mut depth = 0i32;
    let mut close: Option<usize> = None;
    for (i, ch) in full[json_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(json_start + i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(end) = close else {
        return (full.trim().to_string(), None);
    };
    match serde_json::from_str::<DirectorPlanOutput>(&full[json_start..end]) {
        Ok(out) if !out.goal.trim().is_empty() => {
            let after_block = full[end..].trim_start();
            let text = if before.is_empty() {
                after_block.to_string()
            } else {
                format!("{before}\n{after_block}")
            };
            (text.trim().to_string(), Some(DirectorPlanUpdate::Set(out)))
        }
        _ => (full.trim().to_string(), None),
    }
}

#[allow(clippy::too_many_arguments)]
/// 【回执】管道后解析（文本协议根治 2026-10）：最终正文上先剥回执再走散装标记，
/// 与 pre-refine stash 合并后按 9 个消费点提取全部结构化字段 + 协议观测日志。
/// [P5 2026-10-05 从 start_turn 内联提取] full_text 经 mem::take 换入换出，块体字节不变。
/// 返回（merged_receipt, options, panels, program, mcp_results, state_updates,
///        roster_names, check_requests, director_plan_update, llm_advance_to）。
#[allow(clippy::type_complexity)]
pub(crate) async fn run_post_parse_phase(
    full_text: &mut String,
    stash_receipt: &TurnReceipt,
    stash_options: Vec<String>,
    stash_panels: Vec<kaleido_core::TavernPanel>,
    stash_program: Option<String>,
    stash_mcp: Vec<crate::tavern_mcp::McpCall>,
    stash_state: Vec<kaleido_core::ActorStateUpdate>,
    stash_check: Vec<kaleido_core::TurnCheckRequest>,
    stash_plan: Option<DirectorPlanUpdate>,
    stash_advance: Option<String>,
    sessions_store: &kaleido_core::TavernSessionStore,
    quality_bg: TurnQuality,
    data_root_bg: &std::path::Path,
    workspace_id_bg: &str,
    session_id_bg: &str,
    pack: &StoryPack,
) -> (TurnReceipt, Vec<String>, Vec<kaleido_core::TavernPanel>, Option<String>, Vec<kaleido_core::ToolResultBrief>, Vec<kaleido_core::ActorStateUpdate>, Option<Vec<String>>, Vec<kaleido_core::TurnCheckRequest>, Option<DirectorPlanUpdate>, Option<String>) {
    let mut full_text_local = std::mem::take(full_text);
            // 【回执】管道后解析（文本协议根治 2026-10）：最终正文上先剥回执再走散装
            // 标记。与 pre-refine stash 合并——经历质量管道的回合回执已在管道前剥离
            //（post 为空 → 用 stash）；lite 直出回合 stash 为空 → 直接用 post。
            let (receipt_clean, post_receipt) = split_turn_receipt_from_narrative(&full_text_local);
            full_text_local = receipt_clean;
            let merged_receipt =
                post_receipt.unwrap_or_default().merge(stash_receipt.clone());
            if !merged_receipt.is_empty() {
                tracing::info!("st turn receipt parsed+merged");
            }
            // 协议观测（2026-10-05）：本回合各结构化字段来自回执还是旧标记回退。
            // 用于评估旧散装标记解析器的退役时机（回执命中率持续接近 100% 即可裁撤）。
            let mut receipt_fields: Vec<&'static str> = Vec::new();
            let mut legacy_fields: Vec<&'static str> = Vec::new();

            // Extract options + strip from narrative (chips are the only option UI)
            let (clean_text, options) = if merged_receipt.options.is_some() {
                (full_text_local.clone(), merged_receipt.options.clone().unwrap_or_default())
            } else if !stash_options.is_empty() {
                (full_text_local.clone(), stash_options)
            } else {
                split_options_from_narrative(&full_text_local)
            };
            full_text_local = clean_text;
            if !options.is_empty() {
                tracing::info!(n = options.len(), "st options extracted+stripped");
            }
            if merged_receipt.options.is_some() {
                receipt_fields.push("options");
            } else if !options.is_empty() {
                legacy_fields.push("options");
            }

            // [fix 2026-08-16] 开头推理剥离全档位生效：strip_lite_reasoning_leak 原只在
            // run_quality_refine（standard/heavy 管道）调用，Lite 直出路径完全缺失——
            // 实踩（实例2 代替父亲会话 20:23/20:28）：deepseek-v4-flash-free 在 Lite 直出
            // 时输出「让我理清当前状态…」「好的，梳理一下当前状态…」等整段思考链混入正文。
            // 名称虽含 lite，逻辑是通用「最长连续叙事段」提取，接入公共链后全档位生效。
            full_text_local = strip_lite_reasoning_leak(full_text_local);

            // [fix 2026-08-16 P2+ 档位全覆盖] 尾部元话语剥离：此前 strip_trailing_metadiscourse
            // 只在 Heavy 的 fix 阶段（strip_fix_thinking_blocks）调用，Lite/Standard 直出路径
            // 缺失——实踩 deepseek-v4-flash-free 在正文后输出「嗯，这个版本不错。让我检查一下：
            // 1. 时间…✓」自检段 + 状态更新思考（「让我写状态更新块：…我应该更新哪个？…」），
            // 推理混入正文且占用输出预算导致正文后段像「没写完」。接入公共链后全档位生效。
            full_text_local = strip_trailing_metadiscourse(full_text_local);

            // H5 (吞噬 humanizer-zh): 破折号确定性硬修（全档位，零 LLM）。
            // 中文叙事“——”90% 可换逗号/句号；替换数进日志，humanize 诊断跑修后文本。
            {
                let (fixed, n) = kaleido_core::humanize::hard_fix_dashes(&full_text_local);
                if n > 0 {
                    tracing::info!(n, "st humanize: 破折号硬修");
                    full_text_local = fixed;
                }
            }

            // ST-30 (2026-08-15 根治): 剥离 <角色清单>…</角色清单> 结构化块（生成端角色自报）。
            // LLM 在回合正文末尾列出实际出场人名，守卫做精确集合比对——根治切词启发式的
            // 漏报（叙述形态点名）与误报（切词切碎短语）。清单缺失时守卫自动降级启发式。
            let (mut full_text_local, roster_names) = split_roster_from_narrative(&full_text_local);
            if roster_names.is_some() {
                tracing::info!("st roster block extracted");
            }

            // 面板（吸收自梨园 panels.ts）：提取【面板】块并剥离
            let (clean_panels, panels) = if merged_receipt.panels.is_some() {
                (full_text_local.clone(), merged_receipt.panels.clone().unwrap_or_default())
            } else if !stash_panels.is_empty() {
                (full_text_local.clone(), stash_panels)
            } else {
                split_panels_from_narrative(&full_text_local)
            };
            full_text_local = clean_panels;
            if !panels.is_empty() {
                tracing::info!(n = panels.len(), "st panels extracted+stripped");
            }
            if merged_receipt.panels.is_some() {
                receipt_fields.push("panels");
            } else if !panels.is_empty() {
                legacy_fields.push("panels");
            }

            // 程序卡(吸收自梨园 show_html)：提取【程序】块并剥离, 存到消息 program 字段
            let (clean_prog, program) = if merged_receipt.program.is_some() {
                (full_text_local.clone(), merged_receipt.program.clone())
            } else if stash_program.is_none() {
                split_program_from_narrative(&full_text_local)
            } else {
                (full_text_local.clone(), stash_program)
            };
            full_text_local = clean_prog;
            if program.is_some() {
                tracing::info!("st program card extracted");
            }
            if merged_receipt.program.is_some() {
                receipt_fields.push("program");
            } else if program.is_some() {
                legacy_fields.push("program");
            }

            // skill 工具按需加载（吸收自 denova skill.NewMiddleware）：提取【技能加载】独立块
            // 并剥离，按当前档位装载完整 SKILL.md 存入会话，下轮 system prompt 回填全文；lite 忽略。
            let (pre_skill_text, skill_calls) =
                crate::skill_layer::split_skill_load_calls_from_narrative(&full_text_local);
            if !skill_calls.is_empty() {
                let tier = crate::skill_layer::resolve_tier_for_quality(quality_bg);
                if tier != "lite" {
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
                            tracing::info!(n = skill_calls.len(), tier, "st skill load injected for next round");
                        }
                    }
                } else {
                    tracing::info!("st skill load ignored (lite)");
                }
                full_text_local = pre_skill_text;
            }

            // MCP 外设（吸收自梨园 mcp.ts）：提取【工具】块并剥离、执行、结果存 session 待下轮回填
            let (clean_mcp, mcp_calls) = if stash_mcp.is_empty() {
                crate::tavern_mcp::split_mcp_calls_from_narrative(&full_text_local)
            } else {
                (full_text_local.clone(), stash_mcp)
            };
            full_text_local = clean_mcp;
            let mut mcp_results: Vec<kaleido_core::ToolResultBrief> = Vec::new();
            if !mcp_calls.is_empty() {
                tracing::info!(n = mcp_calls.len(), "st mcp calls extracted+stripped");
                for call in &mcp_calls {
                    let tool_tag = format!("{}:{}", call.server, call.tool);
                    match crate::tavern_mcp::call_tool(
                        data_root_bg,
                        &call.server,
                        &call.tool,
                        call.arguments.clone(),
                    )
                    .await
                    {
                        Ok(r) => mcp_results.push(kaleido_core::ToolResultBrief {
                            tool: tool_tag,
                            ok: true,
                            summary: r,
                        }),
                        Err(e) => mcp_results.push(kaleido_core::ToolResultBrief {
                            tool: tool_tag,
                            ok: false,
                            summary: e,
                        }),
                    }
                }
            }

            // ST-26: Actor 状态更新块解析（【状态更新】JSON，成功块剥离；回执 stateUpdates 优先）
            let (clean_state, state_updates) = if merged_receipt.state_updates.is_some() {
                (full_text_local.clone(), merged_receipt.state_updates.clone().unwrap_or_default())
            } else if stash_state.is_empty() {
                split_state_updates_from_narrative(&full_text_local)
            } else {
                (full_text_local.clone(), stash_state)
            };
            full_text_local = clean_state;
            if !state_updates.is_empty() {
                tracing::info!(n = state_updates.len(), "st actor state updates extracted");
            }
            if merged_receipt.state_updates.is_some() {
                receipt_fields.push("stateUpdates");
            } else if !state_updates.is_empty() {
                legacy_fields.push("stateUpdates");
            }

            // [morphling C2 2026-08-16] 章节剧情摘要（顺带总结模式）：剥离【章节摘要】块
            // 并落账本（manual_edited 保护；LLM 未输出块时静默跳过，由 fallback 提炼兜底）。
            // 回执 chapterDiary 优先（文本协议根治 2026-10）。
            let (clean_diary, diary_summary) = match merged_receipt.chapter_diary.clone() {
                Some(d) => (full_text_local.clone(), Some(d)),
                None => kaleido_core::chapter_diary::extract_chapter_diary_block(&full_text_local),
            };
            if merged_receipt.chapter_diary.is_some() {
                receipt_fields.push("chapterDiary");
            } else if diary_summary.is_some() {
                legacy_fields.push("chapterDiary");
            }
            if diary_summary.is_some() {
                full_text_local = clean_diary;
                if let Ok(mut s) = sessions_store.get(session_id_bg) {
                    if let Some(ch_id) = s.chapter_cursor.clone() {
                        let ch_title = pack
                            .chapters
                            .iter()
                            .find(|c| c.id == ch_id)
                            .map(|c| c.title.clone())
                            .unwrap_or_else(|| ch_id.clone());
                        let changed = kaleido_core::chapter_diary::upsert_chapter_diary(
                            &mut s.chapter_diaries,
                            &ch_id,
                            &ch_title,
                            diary_summary.as_deref().unwrap_or(""),
                            s.turn,
                        );
                        if changed {
                            if let Err(e) = sessions_store.save(s.clone()) {
                                tracing::warn!(error = %e, "章节摘要落库失败");
                            }
                            tracing::info!(session = %session_id_bg, chapter = %ch_id, "章节摘要已随回合顺带更新");
                        }
                    }
                }
            }

            // ST-27: 规则检定块解析（【检定】JSON，action 非空块剥离；回执 check 优先）
            let (clean_check, check_requests) = if merged_receipt.check.is_some() {
                (full_text_local.clone(), merged_receipt.check.clone().unwrap_or_default())
            } else if stash_check.is_empty() {
                split_check_from_narrative(&full_text_local)
            } else {
                (full_text_local.clone(), stash_check)
            };
            full_text_local = clean_check;
            if !check_requests.is_empty() {
                tracing::info!(n = check_requests.len(), "st rule checks extracted");
            }
            if merged_receipt.check.is_some() {
                receipt_fields.push("check");
            } else if !check_requests.is_empty() {
                legacy_fields.push("check");
            }

            // S4 (吞噬 denova director_plan): 【导演计划】块解析（none = 无需更新；JSON = 新计划）
            // 回执 directorPlan 优先（文本协议根治 2026-10；skip/none 语义等价旧块）。
            let (clean_plan, director_plan_update) = if merged_receipt.director_plan.is_some() {
                (full_text_local.clone(), merged_receipt.director_plan.clone())
            } else if stash_plan.is_none() {
                split_director_plan_from_narrative(&full_text_local)
            } else {
                (full_text_local.clone(), stash_plan)
            };
            full_text_local = clean_plan;
            if director_plan_update.is_some() {
                tracing::info!("st director plan block parsed");
            }
            if merged_receipt.director_plan.is_some() {
                receipt_fields.push("directorPlan");
            } else if director_plan_update.is_some() {
                legacy_fields.push("directorPlan");
            }

            // ST-15: extract 【节点推进:nodeId】 marker for LLM-directed auto-advance
            // [ST-15 fix 2026-08-16] 用 extract_advance_marker 兼容【节点推进:n2】格式
            // 回执 advance 优先（文本协议根治 2026-10；正文已剥回执，无需再剥离标记）。
            let mut llm_advance_to = merged_receipt.advance.clone();
            let adv_from_receipt = llm_advance_to.is_some();
            let mut adv_from_legacy = false;
            if llm_advance_to.is_none() {
                if let Some((adv_pos, node_id)) = extract_advance_marker(&full_text_local) {
                    info!(%node_id, "llm directed auto-advance");
                    llm_advance_to = Some(node_id);
                    adv_from_legacy = true;
                    // Strip the marker from visible narrative
                    full_text_local = full_text_local[..adv_pos].to_string() + " ";
                }
            }
            // heavy/standard 预剥离回填：本轮正文已无 marker 时用 stash 补上
            if llm_advance_to.is_none() && stash_advance.is_some() {
                llm_advance_to = stash_advance;
                adv_from_legacy = true;
                if let Some(adv) = llm_advance_to.as_ref() {
                    info!(node_id = %adv, "st llm advance pre-refine fallback");
                }
            }
            if adv_from_receipt {
                receipt_fields.push("advance");
            } else if adv_from_legacy {
                legacy_fields.push("advance");
            }
            // 协议观测汇总：一行/回合（tracing JSON 后可按 session 聚合出回执命中率）
            info!(receipt = ?receipt_fields, legacy = ?legacy_fields, "st protocol adoption");

    *full_text = full_text_local;
    (merged_receipt, options, panels, program, mcp_results, state_updates, roster_names, check_requests, director_plan_update, llm_advance_to)
}

