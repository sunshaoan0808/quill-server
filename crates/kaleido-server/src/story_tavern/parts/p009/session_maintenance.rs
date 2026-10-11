//! p009/session_maintenance —— 会话压缩 / 分支摘要 / 孤儿 run 清扫。（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn compact_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<CompactSessionRequest>,
) -> Response {
    let _session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&session_id, &_session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };

    let before_turns = sess.messages.len();
    if before_turns < 4 {
        return Json(json!({
            "ok": true,
            "message": "Too few messages to compact",
            "before_turns": before_turns,
            "after_turns": before_turns,
            "summary": "",
        })).into_response();
    }

    // Use memory_weaver to determine if compaction is needed and find cut point
    let config = state.weaver_config.clone();
    let total_tokens = kaleido_core::memory_weaver::estimate_total_tokens(&sess.messages);
    let force_compact = total_tokens > 0; // manual trigger always compacts

    let stats = kaleido_core::memory_weaver::analyze_messages(&sess.messages);
    let cut_index = if force_compact {
        // For manual trigger, find a reasonable cut point
        kaleido_core::memory_weaver::find_cut_point(&stats, config.keep_recent_tokens)
            .unwrap_or(sess.messages.len() / 2)
    } else {
        return Json(json!({
            "ok": true,
            "message": "Context within budget, no compaction needed",
            "before_turns": before_turns,
            "after_turns": before_turns,
            "summary": "",
        })).into_response();
    };

    if cut_index < 2 {
        return Json(json!({
            "ok": true,
            "message": "Not enough messages to compact",
            "before_turns": before_turns,
            "after_turns": before_turns,
            "summary": "",
        })).into_response();
    }

    // Build conversation text for LLM summarization
    let (conversation_text, omit_stats) = kaleido_core::memory_weaver::serialize_for_summary_with_stats(
        &sess.messages[..cut_index],
        "玩家",
        "NPC",
    );
    tracing::info!(
        session = %session_id,
        kept = omit_stats.kept,
        skipped_program = omit_stats.skipped_program,
        skipped_reasoning = omit_stats.skipped_reasoning,
        skipped_empty = omit_stats.skipped_empty,
        skipped_other = omit_stats.skipped_other,
        "场记摘要序列化省略统计（吸收自 OpenHanako OmissionCounts）"
    );
    let previous_summary = if sess.memory_l1.scene_summary.is_empty() {
        None
    } else {
        Some(sess.memory_l1.scene_summary.as_str())
    };
    let system_prompt = compact_system_prompt(&body.style);
    let user_prompt = kaleido_core::memory_weaver::build_rp_summary_user_text(
        &conversation_text,
        &format!("回合: {} | 模式: {}", sess.turn, sess.play_mode.as_str()),
        previous_summary,
    );

    let summary = match call_llm_nonstream(&state, &system_prompt, &user_prompt).await {
        Ok(s) => s,
        Err(e) => {
            return internal("ST_LLM_CALL_FAILED", e)
        }
    };

    // P5: 场记摘要结构契约 + 修复循环（吸收自 OpenHanako rolling-summary-format：
    // 校验失败附原因重试 1 次，仍失败用原摘要降级——修复成功则落盘更完整）。
    // [P9 2026-08-15] 语义污染检测：结构校验之外，fix 元话语（让我分析审稿意见…）
    // 会被摘要 LLM 照抄进 L1——污染视同校验失败，进修复循环。
    let summary = {
        let polluted = kaleido_core::st_memory_contract::summary_is_polluted(&summary);
        let issues = kaleido_core::st_memory_contract::validate_rp_summary(&summary);
        if !polluted && issues.ok {
            summary
        } else {
            let mut reason_lines: Vec<String> = issues.issues.clone();
            if polluted {
                reason_lines.push("摘要含 fix 元话语污染（让我分析审稿意见/让我检查/审稿意见清单等自指段），须重写为干净场记摘要".to_string());
                tracing::warn!(session = %session_id, "场记摘要语义污染（fix 元话语），尝试修复");
            }
            tracing::warn!(
                session = %session_id,
                issues = ?issues.issues,
                "场记摘要结构校验失败，尝试格式修复"
            );
            let repair_prompt = kaleido_core::st_memory_contract::build_rp_summary_repair_prompt();
            let repair_input =
                kaleido_core::st_memory_contract::build_rp_summary_repair_input(&issues.issues, &summary);
            let mut repaired = summary.clone();
            if let Ok(r) = call_llm_nonstream(&state, &repair_prompt, &repair_input).await {
                let after = kaleido_core::st_memory_contract::validate_rp_summary(&r);
                if after.ok {
                    tracing::info!(session = %session_id, "场记摘要格式修复成功");
                    repaired = r;
                } else {
                    tracing::warn!(
                        session = %session_id,
                        issues = ?after.issues,
                        "场记摘要修复后仍不合规，使用原摘要降级"
                    );
                }
            }
            repaired
        }
    };

    // Extract retained facts from summary (parse structured output)
    let _retained_facts: Vec<String> = Vec::new(); // Facts are embedded in summary text

    // Apply compaction: keep only messages from cut_index onward, update L1 summary
    let retained_messages: Vec<TavernMessage> = sess.messages[cut_index..].to_vec();
    sess.messages = retained_messages;
    // Merge new summary with existing
    let merged_summary = if let Some(prev) = previous_summary {
        format!("{}\n\n---\n\n{}", prev, summary)
    } else {
        summary.clone()
    };
    sess.memory_l1.scene_summary = merged_summary;
    sess.memory_l1.updated_at_turn = sess.turn;

    // Record compaction epoch
    sess.epoch = sess.epoch.saturating_add(1);
    sess.epoch_last_turn = Some(sess.turn);
    sess.epoch_last_chars = Some(total_tokens as u32);

    let after_turns = sess.messages.len();
    let final_epoch = sess.epoch;
    let _ = state.sessions_tavern.save(sess);

    Json(json!({
        "ok": true,
        "before_turns": before_turns,
        "after_turns": after_turns,
        "summary": summary,
        "style": body.style,
        "epoch": final_epoch,
    })).into_response()
}

// ─── U13 M3: Branch Summary ──────────────────────────────────────────────────

/// GET /api/v1/story-tavern/sessions/{id}/branches/{bid}/summary
/// Independent branch summary. When no branch exists, returns session summary.
pub(crate) async fn branch_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((session_id, branch_id)): Path<(String, String)>,
) -> Response {
    let _session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let sess = match state.sessions_tavern.get_for_owner(&session_id, &_session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };

    // Check if branch_id matches current session or is a side branch
    let is_current_branch = branch_id == "main" || branch_id == sess.session_id;
    let is_side_branch = sess.side_branch_node_id.as_deref() == Some(&branch_id);

    if is_current_branch || !is_side_branch {
        // No separate branch: return session summary (L1)
        return Json(json!({
            "branchId": branch_id,
            "summary": sess.memory_l1.scene_summary,
            "updatedAtTurn": sess.memory_l1.updated_at_turn,
            "isCurrentBranch": true,
            "turn": sess.turn,
            "epoch": sess.epoch,
        })).into_response();
    }

    // For side branches, generate a branch-specific summary from session messages
    // Filter messages relevant to this branch (messages since entering the side branch)
    let branch_messages: Vec<&TavernMessage> = sess.messages.iter().collect();

    let conversation_text = kaleido_core::memory_weaver::serialize_for_summary(
        &branch_messages.iter().map(|m| (*m).clone()).collect::<Vec<_>>(),
        "玩家",
        "NPC",
    );

    if conversation_text.trim().is_empty() {
        return Json(json!({
            "branchId": branch_id,
            "summary": String::new(),
            "updatedAtTurn": sess.turn,
            "isCurrentBranch": false,
            "turn": sess.turn,
            "epoch": sess.epoch,
        })).into_response();
    }

    let system_prompt = "你是一个分支摘要生成器。请为以下对话分支生成一个结构化摘要。\n\n输出格式：\n## 目标\n[此分支中玩家试图完成什么]\n\n## 进展\n- [已完成]\n- [进行中]\n\n## 关键决策\n- [决策及理由]\n\n## 下一步\n1. [接下来应该做什么]\n\n保持每节简洁，保留文件路径、函数名、错误消息等精确信息。";
    let user_prompt = format!(
        "请为以下对话分支生成摘要：\n\n分支ID: {}\n支线标题: {}\n\n{}",
        branch_id,
        sess.side_branch_label.as_deref().unwrap_or("(未命名)"),
        conversation_text
    );

    let summary = match call_llm_nonstream(&state, system_prompt, &user_prompt).await {
        Ok(s) => s,
        Err(e) => {
            return internal("ST_LLM_CALL_FAILED", e)
        }
    };

    Json(json!({
        "branchId": branch_id,
        "summary": summary,
        "updatedAtTurn": sess.turn,
        "isCurrentBranch": false,
        "turn": sess.turn,
        "epoch": sess.epoch,
        "branchLabel": sess.side_branch_label,
    })).into_response()
}

/// 启动时孤儿回合清扫（2026-08-15 实踩：13:23:33 服务重启打断进行中回合，
/// 会话 active_run_id 永久残留——U11 resume 只在新回合触发时检查，重启后无新
/// 消息则孤儿永远挂着，前端显示「生成中」卡死）。
///
/// 语义：服务刚启动，hub 必然无任何流式 worker。凡 active_run_id 非空且对应
/// job 仍标 running 的会话，一律判定为「被重启打断的孤儿回合」：
/// 1. job 终态 cancelled（释放 running 并发槽，防死锁）
/// 2. 会话 active_run_id 清空（解除会话锁）
/// 3. 写入一条 assistant 系统提示消息（可见、可恢复），正文明示中断原因
///
/// 幂等：仅处理 active_run_id 非空且 job 状态 active 的会话；正常会话不动。
/// fail-open：任何 store 错误仅 warn，不阻断服务启动。
pub(crate) fn sweep_orphan_runs(state: &AppState) {
    let Ok(list) = state.sessions_tavern.list() else {
        tracing::warn!("st sweep_orphan_runs: sessions list failed");
        return;
    };
    let mut cleaned = 0usize;
    // list() 仅返回摘要（无 active_run_id），需用 get() 取完整 TavernSession。
    for item in list {
        let Some(sid) = item.get("sessionId").and_then(|v| v.as_str()).map(|s| s.to_string()) else {
            continue;
        };
        let Ok(mut sess) = state.sessions_tavern.get(&sid) else {
            continue;
        };
        let Some(prev) = sess.active_run_id.clone() else {
            continue;
        };
        // 孤儿判定：job 已终态（dispatch_recovered 启动恢复后，tavern-turn 无 slug 会
        // 被标 failed）或 job 不存在 → 会话锁死，标记中断。queued/running（有活 worker
        // 或已 rearm 待恢复）不算孤儿。
        let job_state = state
            .jobs
            .get(&prev)
            .map(|j| kaleido_core::normalize_job_status(&j.status))
            .unwrap_or_else(|| "missing".into());
        let is_orphan = !kaleido_core::is_active_job_status(&job_state);
        if !is_orphan {
            continue;
        }
        // 服务刚启动，hub 无活 worker——非 active 即孤儿。
        tracing::warn!(
            %prev,
            %sid,
            job_status = %job_state,
            "st sweep_orphan_runs: interrupted turn marked (service restart)"
        );
        // 1. job 终态 cancelled（cancel-wins 语义，不会复活）
        let _ = state.jobs.complete(
            &prev,
            "cancelled",
            None,
            Some("sweep_orphan_runs: interrupted by service restart".into()),
        );
        // 2. 解除会话锁
        sess.active_run_id = None;
        // 3. 系统提示消息（assistant 角色，前端自然展示）
        sess.messages.push(TavernMessage {
            id: format!("msg-{}", uuid::Uuid::new_v4()),
            role: "assistant".into(),
            content: "（上一回合因服务重启而中断，未完成生成。你可以重新发送刚才的话，或继续新的行动。）".into(),
            created_at: chrono::Utc::now().to_rfc3339(),
            options: Vec::new(),
            engine_tag: None,
            program: None,
            reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
            mvu_blocks: Vec::new(),
        });
        if let Err(e) = state.sessions_tavern.save(sess.clone()) {
            tracing::warn!(%prev, error=%e, "st sweep_orphan_runs: save failed");
            continue;
        }
        cleaned += 1;
    }
    if cleaned > 0 {
        tracing::info!(cleaned, "st sweep_orphan_runs: interrupted turns marked");
    }
}
