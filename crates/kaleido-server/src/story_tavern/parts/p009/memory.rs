//! p009/memory —— 记忆分析/优化/压缩 + L4 情感记忆补丁合并。（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;

/// Build the conversation text for memory analysis: serialize recent messages
/// plus memory layer snapshots.
pub(crate) fn build_memory_analysis_context(sess: &TavernSession) -> String {
    let mut parts = Vec::new();
    // Recent messages (last 30 messages max to stay within context)
    let start = sess.messages.len().saturating_sub(30);
    for msg in &sess.messages[start..] {
        let label = match msg.role.as_str() {
            "user" => "玩家",
            "assistant" => "NPC",
            _ => continue,
        };
        let text = msg.content.trim();
        if text.is_empty() {
            continue;
        }
        parts.push(format!("{}：{}", label, text));
    }
    // Memory L2 events
    if !sess.memory_l2.events.is_empty() {
        parts.push("\n=== 已有事件记录 (L2) ===".into());
        for ev in &sess.memory_l2.events {
            parts.push(format!("[回合{}] ({}) {}", ev.turn, ev.kind, ev.summary));
        }
    }
    // Memory L3 facts
    if !sess.memory_l3.facts.is_empty() {
        parts.push("\n=== 已知事实 (L3) ===".into());
        for fact in &sess.memory_l3.facts {
            parts.push(format!("- {}", fact));
        }
    }
    // Memory L4 secrets/promises
    if !sess.memory_l4.secrets_known.is_empty() {
        parts.push("\n=== 秘密 ===".into());
        for s in &sess.memory_l4.secrets_known {
            parts.push(format!("- {}", s));
        }
    }
    if !sess.memory_l4.promises.is_empty() {
        parts.push("\n=== 承诺 ===".into());
        for p in &sess.memory_l4.promises {
            parts.push(format!("- {}", p));
        }
    }
    parts.join("\n")
}

/// POST /api/v1/story-tavern/sessions/{id}/analyze-memory
/// Analyze character memory for a specific character (or focus character).
/// Returns structured {character, memories:[{content,type,certainty}], proposed_updates}.
pub(crate) async fn analyze_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<AnalyzeMemoryRequest>,
) -> Response {
    let _session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let sess = match state.sessions_tavern.get_for_owner(&session_id, &_session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };

    // Resolve target character
    let pack = match state.packs.get(&sess.pack_id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let target_char_id = body.character_id.clone()
        .or_else(|| sess.focus_character_id.clone())
        .or_else(|| sess.entry.vessel_character_id.clone());
    let target_char_name = target_char_id.as_ref()
        .and_then(|cid| pack.characters.iter().find(|c| c.id == *cid))
        .map(|c| c.name.as_str())
        .unwrap_or("(未知角色)");
    let target_char_card = target_char_id.as_ref()
        .and_then(|cid| pack.characters.iter().find(|c| c.id == *cid))
        .map(|c| c.personality.as_str())
        .unwrap_or("(未提供)");

    let context = build_memory_analysis_context(&sess);
    let system_prompt = "你是一个专门负责角色记忆管理的AI助手。你需要基于对话记录和已有记忆，分析角色与玩家之间的关系、关键事件、秘密和承诺。\n\n请以纯JSON格式输出，不要包含markdown标记。JSON结构如下：\n{\n  \"character\": \"角色名\",\n  \"memories\": [\n    {\n      \"content\": \"记忆内容描述\",\n      \"type\": \"secret|promise|relationship|event\",\n      \"certainty\": \"high|medium|low\"\n    }\n  ],\n  \"proposed_updates\": {\n    \"secrets\": [\"新增秘密...\"],\n    \"promises\": [\"新增承诺...\"],\n    \"facts\": [\"新增事实...\"],\n    \"events\": [\"新增事件摘要...\"]\n  }\n}";
    let user_prompt = format!(
        "请分析以下角色的记忆。\n\n### 目标角色\n- 角色名：{}\n- 角色卡描述：{}\n\n### 对话与记忆上下文\n{}\n\n请基于上述内容，分析该角色与玩家之间的关系演变、关键事件、秘密和承诺，输出结构化JSON。",
        target_char_name, target_char_card, context
    );

    let raw = match call_llm_nonstream(&state, &system_prompt, &user_prompt).await {
        Ok(r) => r,
        Err(e) => return internal("ST_LLM_CALL_FAILED", e),
    };

    // Try to parse as JSON; if not, wrap in a simple structure
    let result = if let Some(v) = crate::llm_stream::extract_json_value(&raw) {
        v
    } else {
        json!({
            "character": target_char_name,
            "memories": [{"content": raw, "type": "event", "certainty": "medium"}],
            "proposed_updates": {}
        })
    };

    // If apply=true, push proposed_updates into MemoryPatch pipeline
    if body.apply {
        if let Some(updates) = result.get("proposed_updates") {
            let mut sess = sess;
            if let Some(secrets) = updates.get("secrets").and_then(|v| v.as_array()) {
                for s in secrets {
                    if let Some(text) = s.as_str() {
                        if !text.is_empty() && !sess.memory_l4.secrets_known.iter().any(|x| x == text) {
                            sess.memory_l4.secrets_known.push(text.to_string());
                        }
                    }
                }
            }
            if let Some(promises) = updates.get("promises").and_then(|v| v.as_array()) {
                for p in promises {
                    if let Some(text) = p.as_str() {
                        if !text.is_empty() && !sess.memory_l4.promises.iter().any(|x| x == text) {
                            sess.memory_l4.promises.push(text.to_string());
                        }
                    }
                }
            }
            if let Some(facts) = updates.get("facts").and_then(|v| v.as_array()) {
                for f in facts {
                    if let Some(text) = f.as_str() {
                        if !text.is_empty() && !sess.memory_l3.facts.iter().any(|x| x == text) {
                            // [fix §10 2026-08-16] 关键物品/承诺永久层：含所有权/收藏/承诺
                            // 语义的事实（如「素描原稿被向明初收藏」）入 pinned，注入不裁剪——
                            // 窝边草素描链 t38-42 曾被 take(6) 挤出导致「画好了又自己画」失忆。
                            if is_key_fact(text) {
                                if !sess.memory_l3.pinned.iter().any(|x| x == text) {
                                    sess.memory_l3.pinned.push(text.to_string());
                                }
                            } else {
                                sess.memory_l3.facts.push(text.to_string());
                            }
                        }
                    }
                }
            }
            if let Some(events) = updates.get("events").and_then(|v| v.as_array()) {
                for e in events {
                    if let Some(text) = e.as_str() {
                        if !text.is_empty() {
                            let ev = kaleido_core::MemoryL2Event {
                                id: format!("ev-{}", Uuid::new_v4()),
                                turn: sess.turn,
                                kind: "analyze".into(),
                                summary: text.to_string(),
                                actors: target_char_id.clone().into_iter().collect(),
                                node_id: sess.node_id.clone(),
                                embedding: vec![],
                            };
                            sess.memory_l2.events.push(ev);
                        }
                    }
                }
            }
            let _ = state.sessions_tavern.save(sess);
        }
    }

    Json(result).into_response()
}

/// POST /api/v1/story-tavern/sessions/{id}/optimize-memory
/// Scan L2 events + L3 facts → LLM deduplication/merge → MemoryPatch.
pub(crate) async fn optimize_memory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<OptimizeMemoryRequest>,
) -> Response {
    let _session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut sess = match state.sessions_tavern.get_for_owner(&session_id, &_session.user_id) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };

    // Build combined memory text for optimization
    let mut memory_lines = Vec::new();
    for ev in &sess.memory_l2.events {
        memory_lines.push(format!("[事件] 回合{} ({}): {}", ev.turn, ev.kind, ev.summary));
    }
    for fact in &sess.memory_l3.facts {
        memory_lines.push(format!("[事实] {}", fact));
    }
    for s in &sess.memory_l4.secrets_known {
        memory_lines.push(format!("[秘密] {}", s));
    }
    for p in &sess.memory_l4.promises {
        memory_lines.push(format!("[承诺] {}", p));
    }

    if memory_lines.is_empty() {
        return Json(json!({"ok": true, "message": "No memories to optimize", "before": 0, "after": 0})).into_response();
    }

    let before_count = sess.memory_l2.events.len() + sess.memory_l3.facts.len()
        + sess.memory_l4.secrets_known.len() + sess.memory_l4.promises.len();
    let memory_text = memory_lines.join("\n");

    let system_prompt = "你是一个专门负责人物记忆分析与优化的专家。你的任务是读取现有的事件记录和事实，进行去冗余、归并、消除逻辑矛盾。\n\n请返回优化后的JSON结果，格式如下：\n{\n  \"events\": [\"精简后的事件摘要...\"],\n  \"facts\": [\"精简后的事实...\"],\n  \"secrets\": [\"精简后的秘密...\"],\n  \"promises\": [\"精简后的承诺...\"],\n  \"removed_count\": 0,\n  \"notes\": \"优化说明...\"\n}\n\n规则：\n1. 合并重复或高度相似的条目\n2. 消除逻辑矛盾，以更合理的版本为准\n3. 保留所有关键信息，宁多勿漏\n4. events/facts/secrets/promises 分类不变";
    let user_prompt = format!(
        "请优化以下角色记忆记录，去冗余、归并、消除矛盾：\n\n{}",
        memory_text
    );

    let raw = match call_llm_nonstream(&state, &system_prompt, &user_prompt).await {
        Ok(r) => r,
        Err(e) => return internal("ST_LLM_CALL_FAILED", e),
    };

    let result = if let Some(v) = crate::llm_stream::extract_json_value(&raw) {
        v
    } else {
        json!({"error": "Failed to parse optimization result", "raw": raw})
    };

    // If apply=true, replace memory layers with optimized version
    if body.apply {
        if let Some(events_arr) = result.get("events").and_then(|v| v.as_array()) {
            let new_events: Vec<kaleido_core::MemoryL2Event> = events_arr
                .iter()
                .filter_map(|v| v.as_str())
                .enumerate()
                .map(|(i, text)| kaleido_core::MemoryL2Event {
                    id: format!("opt-{}-{}", Uuid::new_v4(), i),
                    turn: sess.turn,
                    kind: "optimized".into(),
                    summary: text.to_string(),
                    actors: vec![],
                    node_id: None,
                    embedding: vec![],
                })
                .collect();
            sess.memory_l2.events = new_events;
        }
        if let Some(facts_arr) = result.get("facts").and_then(|v| v.as_array()) {
            sess.memory_l3.facts = facts_arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(secrets_arr) = result.get("secrets").and_then(|v| v.as_array()) {
            sess.memory_l4.secrets_known = secrets_arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(promises_arr) = result.get("promises").and_then(|v| v.as_array()) {
            sess.memory_l4.promises = promises_arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        let _ = state.sessions_tavern.save(sess.clone());
    }

    let after_count = result.get("events").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
        + result.get("facts").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
        + result.get("secrets").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
        + result.get("promises").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);

    Json(json!({
        "ok": true,
        "result": result,
        "before": before_count,
        "after": after_count,
    })).into_response()
}

// ─── U13 M2: Explicit Session Compaction ─────────────────────────────────────

/// Build a compact system prompt based on style.
pub(crate) fn compact_system_prompt(style: &str) -> String {
    match style {
        "bulletin" => {
            "你是一个要点式会话压缩器。将以下对话记录压缩为要点摘要。\n\n输出格式：\n## 关键要点\n- [要点1]\n- [要点2]\n...\n\n## 保留事实\n- [事实1]\n...\n\n## 已丢弃范围说明\n[简要说明丢弃了哪些内容]\n\n规则：\n1. 每个要点控制在一句话内\n2. 保留所有承诺、秘密、关系变化\n3. 保留当前场景状态".into()
        }
        "battle_report" => {
            "你是一个战场式会话压缩器（适配双Agent战场叙事）。将以下对话压缩为战场报告。\n\n输出格式：\n## 战场态势\n[当前局势概括]\n\n## 行动日志\n- [关键行动1]\n- [关键行动2]\n...\n\n## 保留事实\n- [事实1]\n...\n\n## 已丢弃范围说明\n[简要说明丢弃了哪些内容]\n\n规则：\n1. 保留所有角色状态变化和阵营关系\n2. 保留关键决策和转折点\n3. 保留悬念和伏笔".into()
        }
        _ => {
            // "conversation" — 对话流
            "你是一个叙事上下文压缩器。将以下对话记录压缩为结构化的前情摘要，供后续对话续接。\n\n输出格式：\n## 前情提要\n[按时间顺序概述关键事件]\n\n## 人物状态\n[每位出场人物的当前状态和关系变化]\n\n## 保留事实\n- [事实1]\n- [事实2]\n...\n\n## 已丢弃范围说明\n[简要说明丢弃了哪些内容]\n\n规则：\n1. 保留所有承诺、秘密、关系变化\n2. 保留当前场景状态（地点、时间、在场角色）\n3. [P1B 2026-08-16] 保留着装状态：每位角色当前穿着 + 已脱/脱下的衣物去向（谁拿着/扔在哪/是否穿回），脱衣/穿衣/换装事件不得省略\n4. 不虚构、不续写剧情\n5. 人名地名保持剧中写法\n6. [P13 2026-08-15] 环境意象/氛围描写去重：同一意象（如「楼板吱呀响」「鼾声」「雨声」「烛火/蜡油」）在对话中反复出现时，只在摘要中记录一次（说明其为持续氛围即可），禁止逐回合复述或作为新事件强调；事件推进以剧情转折/对白/行动为准，重复的静态描写不占摘要篇幅。".into()
        }
    }
}

/// POST /api/v1/story-tavern/sessions/{id}/compact
/// Manually trigger session context compaction.
/// L4 情感层提炼模板（2026-08-14 补写端）：从最近剧情事件提炼情感增量。
/// 输出 JSON 契约 {affinity:{角色ID:0-100}, secretsKnown:[], promises:[]}，
/// 由 st_memory_contract::parse_l4_patch 校验（不合规降级，不阻塞回合）。
pub(crate) const L4_REFINE_SYSTEM: &str = "你是叙事情感记忆提炼器。从最近剧情事件中提炼情感状态增量，只输出 JSON：\n\
{\"affinity\": {\"<角色ID>\": 0-100 整数}, \"secretsKnown\": [\"新秘密...\"], \"promises\": [\"新承诺...\"]}\n\
规则：1) 只输出 JSON，不要任何其他文字或解释 2) affinity 为 0-100 整数（角色ID 用 pack 的角色 id）\
3) 没有新内容输出空对象/空数组 4) secretsKnown/promises 只记明确达成或透露的，不猜测\
5) [P14 2026-08-15] 好感度必须克制分级：50-60=正常友善，61-70=明显好感，71-80=强烈好感/亲密倾向，\
81-90=深度信任/依恋，91-100=近乎无条件（仅当长期剧情确实发展至此才给）。\
单次事件最多 +10，禁止因一次亲密举动直接给 90+；分歧/抗拒事件应下降或持平。\
宁缺勿滥——不确定时给区间下限，不要给剧情定死结局。";

/// 合并 L4 提炼补丁（增量 merge，不清空旧值）：affinity 覆盖更新、secrets/promises 追加去重。
pub(crate) fn merge_l4_patch(l4: &mut kaleido_core::MemoryL4, patch: &kaleido_core::st_memory_contract::L4Patch) {
    if let Some(aff_map) = l4.affinity.as_object_mut() {
        for (k, v) in &patch.affinity {
            aff_map.insert(k.clone(), serde_json::json!(v));
        }
    } else {
        let mut m = serde_json::Map::new();
        for (k, v) in &patch.affinity {
            m.insert(k.clone(), serde_json::json!(v));
        }
        l4.affinity = serde_json::Value::Object(m);
    }
    for s in &patch.secrets_known {
        if !l4.secrets_known.iter().any(|x| x == s) {
            l4.secrets_known.push(s.clone());
        }
    }
    for p in &patch.promises {
        if !l4.promises.iter().any(|x| x == p) {
            l4.promises.push(p.clone());
        }
    }
}
