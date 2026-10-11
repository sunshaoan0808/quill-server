//! [2026-10-06 P6] //! 游戏时钟推进（advance_game_clock）。
#![allow(unused_imports)]
use super::super::*;

/// [时间天气系统 v2] 回合结束推进权威时钟（剧情信号驱动，信号优先级见块内注释）。
/// [P5 2026-10-05 从 start_turn 内联提取] 返回 time_jump_desc（仅日志消费）。
pub(crate) async fn advance_game_clock(
    state: &AppState,
    sess: &mut kaleido_core::TavernSession,
    player_msg: &str,
    full_text: &str,
    receipt_time_advance: Option<&str>,
) -> Option<String> {
                // [时间天气系统] 回合结束推进权威时钟：
                // 1) 若正文含 [时间推进: <时段|次日|N天后>] 标注 → 显式跳转；
                //    2) 否则默认顺移 1 个时段（跨日 day+1）。
                // 3) 天气：若正文场景标签带天气建议，走邻接渐进（不跳变）。
                // [时间天气系统 v2 2026-08-17] 时间推进改为「剧情信号驱动」，移除强制轮转。
                // 信号优先级（从高到低）：
                //   1) 用户消息/正文自然语言时间推进（次日/睡一觉/N天后）[extract_advance_signal]
                //   2) 正文末尾 [时间推进: X] 标注（模型/剧情标记）
                //   3) LLM 剧情评估（低频，llm_eval_due 间隔控制，省成本）
                //   4) 无任何信号 → 时间保持不变（不再每回合 +1 时段）
                // 天气：正文天气词建议（suggest_weather，邻接渐进）→ 自动轮转兜底；
                //   用户 /weather 指令走 force_weather（第一原则，见 handle_weather_cmd）。
                let mut time_jump_desc = None;
                // 1) 用户输入与正文的自然语言信号（玩家说了「睡一觉」「次日」等）
                let user_story_text = format!("{} {}", player_msg, full_text);
                if let Some(sig) = kaleido_core::time_clock::GameClock::extract_advance_signal(&user_story_text) {
                    match sess.game_clock.jump(&sig, sess.turn) {
                        Ok((_, d)) => time_jump_desc = Some(d),
                        Err(_) => {} // 解析失败忽略
                    }
                }
                // 1b) 回执 timeAdvance 字段（文本协议根治 2026-10；模型侧信号，优先于旧标注）
                if time_jump_desc.is_none() {
                    if let Some(t) = receipt_time_advance {
                        match sess.game_clock.jump(t, sess.turn) {
                            Ok((_, d)) => time_jump_desc = Some(d),
                            Err(_) => {} // 无法解析的时段忽略
                        }
                    }
                }
                // 2) 正文显式 [时间推进: X] 标注（模型侧信号；玩家信号优先）
                if time_jump_desc.is_none() {
                    let advance_req: String = full_text
                        .split("时间推进:")
                        .nth(1)
                        .map(|s| s.split([']', '」', '\n']).next().unwrap_or("").trim().to_string())
                        .unwrap_or_default();
                    if !advance_req.is_empty() {
                        match sess.game_clock.jump(&advance_req, sess.turn) {
                            Ok((_, d)) => time_jump_desc = Some(d),
                            Err(_) => {} // 无法解析的标注忽略
                        }
                    }
                }
                // 3) LLM 剧情时间评估（低频：每 llm_eval_interval 回合一次）。
                //    复用 call_llm_nonstream 轻量评估剧情是否推进时间；失败/超时静默跳过（时间保持）。
                //    仅在 1/2 均无信号时触发，避免与显式信号叠加。
                if time_jump_desc.is_none()
                    && sess.game_clock.llm_eval_due(sess.turn, LLM_CLOCK_EVAL_INTERVAL)
                {
                    match call_llm_nonstream(
                        state,
                        "你是剧情时间评估器。只输出一个 JSON：{\"advance\": true/false, \"target\": \"时段或天数或null\"}。\n\
                        依据当前时间状态、玩家输入与正文判断剧情时间是否应当推进（如过夜、赶路、等待、过了几天），\
                         不要因普通对话推进时间。target 取《清晨/上午/正午/午后/傍晚/夜晚/深夜/凌晨》或《N天后》或 null。",
                        &format!(
                            "当前时间：{}\n玩家输入：{}\n正文末尾：{}",
                            sess.game_clock.state_line(),
                            player_msg,
                            full_text.chars().rev().take(200).collect::<String>().chars().rev().collect::<String>()
                        ),
                    )
                    .await
                    {
                        Ok(out) => {
                            // 解析 {"advance":true,"target":"次日清晨"} → jump
                            let v: serde_json::Value = serde_json::from_str(&out).unwrap_or(serde_json::Value::Null);
                            let advance = v.get("advance").and_then(|x| x.as_bool()).unwrap_or(false);
                            let target = v.get("target").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
                            if advance && !target.is_empty() {
                                match sess.game_clock.jump(&target, sess.turn) {
                                    Ok((_, d)) => time_jump_desc = Some(d),
                                    Err(_) => {}
                                }
                            }
                        }
                        Err(_) => {} // LLM 失败静默：时间保持
                    }
                    sess.game_clock.last_llm_eval_turn = sess.turn;
                }
                // 4) 无任何信号 → 时间保持不变（v2 移除强制顺移）。
                // 天气建议（保守：仅当解析出权威天气且邻接才采纳）；
                // 若无建议或未采纳，则按季节加权自动渐进（维持/渐进 1 级）。
                let mut suggested = false;
                if let Some(w) = kaleido_core::time_clock::WEATHERS
                    .iter()
                    .find(|w| full_text.contains(&format!("｜{}", *w)) || full_text.contains(*w))
                {
                    suggested = sess.game_clock.suggest_weather(w);
                }
                if !suggested {
                    sess.game_clock.auto_advance_weather();
                }
                tracing::debug!(
                    time = %sess.game_clock.state_line(),
                    jump = ?time_jump_desc,
                    "st game clock advanced"
                );
    time_jump_desc
}

