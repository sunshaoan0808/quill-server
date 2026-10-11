//! [2026-10-06 P6] //! ST-27 规则检定（execute_rule_checks）。
#![allow(unused_imports)]
use super::super::*;

/// ST-27: 规则检定执行（rule_system 从当前 pack 宽容解析；无则剥离块但不执行）。
/// [P5 2026-10-05 从 start_turn 内联提取] 返回拼好的【检定结果】文本（由调用方 append 到正文尾部）。
pub(crate) fn execute_rule_checks(
    sess: &mut kaleido_core::TavernSession,
    check_requests: &[kaleido_core::TurnCheckRequest],
    packs_store: &kaleido_core::PackStore,
) -> String {
    let mut check_result_text = String::new();
                if !check_requests.is_empty() {
                    let rule_sys = packs_store
                        .get(&sess.pack_id)
                        .ok()
                        .and_then(|pack| pack.stage_director.resolved_snapshot.and_then(|snap| snap.rule_system))
                        .as_ref()
                        .and_then(kaleido_core::RuleSystem::from_value);
                    if let Some(rs) = rule_sys {
                        for req in check_requests {
                            let tpl = req
                                .template_id
                                .as_ref()
                                .and_then(|id| rs.checks.iter().find(|c| c.id == *id));
                            let seed = kaleido_core::dice::seed_from(&[&sess.session_id, &sess.turn.to_string(), &req.action]);
                            let mut srng = kaleido_core::dice::SeededRng::new(&seed);
                            let res = kaleido_core::roll_check(req, tpl, || srng.roll_one(20));
                            // [fix 2026-08-15] 检定结果不再带（骰面 X + 加成 Y = Z / DC → outcome）括号：
                            // 保留行动名 + 结果描述，隐藏机械骰面细节，保持叙事沉浸感。
                            // [2026-08-16] 剧情卡片：正文检定带中文结果词（隐藏机械骰面数字，保持沉浸），
                            // 前端按结果词着色彩色徽章（大成功/成功/失败/大失败）。
                            let outcome_zh = match res.outcome.as_str() {
                                "critical_success" => "大成功",
                                "success" => "成功",
                                "failure" => "失败",
                                "critical_failure" => "大失败",
                                _ => "检定",
                            };
                            check_result_text.push_str(&format!(
                                "\n【检定结果】{}（{}）\n{}\n",
                                req.action,
                                outcome_zh,
                                res.result_text
                            ));
                            // [P12 2026-08-15] 检定结果存入会话，下回合注入 system prompt 作为
                            // 「剧情约束」——失败（含 critical_failure）必须在正文真实体现，
                            // 不再只是事后 append 展示（修复「提线木偶」：检定失败零影响）。
                            // 先清空上一回合结果（只保留最近一回合的约束），再写入本次。
                            if sess.last_check_results.len() > 4 {
                                sess.last_check_results.clear();
                            }
                            sess.last_check_results.push(format!(
                                "{}（{}）",
                                req.action,
                                res.result_text
                            ));
                            // [2026-08-16] 全量检定历史：跨回合累积（区别于 last_check_results 只留最近一回合），
                            // 供导演台「检定结果」区块展示全部检定。同时记录骰面细节供前端卡片渲染。
                            sess.check_history.push(kaleido_core::CheckHistoryEntry {
                                action: req.action.clone(),
                                outcome: res.outcome.clone(),
                                result_text: res.result_text.clone(),
                                natural: res.natural,
                                total: res.total,
                                dc: res.dc,
                                seed: seed.clone(),
                                state_changes: res.state_changes.clone(),
                                turn: sess.turn as u64,
                            });
                            let updated = sess.actor_states.apply_state_changes(&res.state_changes);
                            if updated > 0 {
                                tracing::info!(updated, "st check state applied");
                            }
                        }
                    } else {
                        warn!(n = check_requests.len(), "st rule checks stripped but no rule_system configured; not executed");
                    }
                }

    check_result_text
}

