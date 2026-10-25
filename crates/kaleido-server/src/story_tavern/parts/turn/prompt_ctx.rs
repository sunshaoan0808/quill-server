//! parts/turn/prompt_ctx — [2026-10-05 P2-P4] 按设计文档 §3.1 从 p006/p009 逐字节迁出。
//! read_outline_clip（大纲切片注入）。
//! 以 `use super::super::*;` 直连 parts 作用域；本文件不自带 use 头，
//! cargo fix 无桥可拆（工程纪律见 docs/REFACTOR_20260927.md）。
#![allow(unused_imports)]

use super::super::*;

/// P1-3 (吞噬 denova outline 牵引): best-effort 读取原著剖析（outline/ 产物）。
/// 尝试 pack.title 同名 / outline.md / start.md；失败静默跳过，返回截断(≤2000字)内容。
pub(crate) fn read_outline_clip(state: &AppState, pack: &StoryPack, workspace_id: &str) -> String {
    let mut outline_bg = String::new();
    let mut candidates = vec![
        format!("outline/{}.md", pack.title),
        "outline/outline.md".to_string(),
        "outline/start.md".to_string(),
    ];
    candidates.dedup();
    for rel in candidates {
        if let Ok(body) = state.works.read_text(workspace_id, &rel) {
            if !body.content.trim().is_empty() {
                outline_bg = body.content;
                break;
            }
        }
    }
    if outline_bg.is_empty() {
        return String::new();
    }
    outline_bg.chars().take(2000).collect()
}

/// [P9+ 2026-10-06 计划外补课] start_turn 步骤 3（pack 上下文装载）：导演调度判定、
/// G3 开局规划后台登记、事件卡抽取、章节体读取、ST-21 query embedding。
/// 原 start_turn 内联代码逐行迁出（pack 缺失早退 map_core_err → Err(Response)）。
pub(crate) async fn load_pack_context(
    state: &AppState,
    tavern: &mut kaleido_core::TavernSession,
    session_id: &str,
) -> Result<(StoryPack, bool, String, u32, Option<Vec<f32>>), Response> {
    // 3. Load pack context
    // 3. Load pack context
    let pack = match state.packs.get(&tavern.pack_id) {
        Ok(p) => p,
        Err(e) => return Err(map_core_err(e)),
    };

    // S4 (吞噬 denova director_run_policy.go): 调度判定 —— 本回合是否附加导演计划生成指令。
    // - interval && director_due → 触发（turn 取本回合完成后的回合数）
    // - on_demand && 上一回合检定命中（assistant 消息含【检定结果】）→ 触发
    // - manual / 模式无效 → 不自动触发；仅 API 置 director_pending 后由本回合消费
    // 兼容旧包：默认 on_demand+interval=0 无规则检定 → 永不自动触发。
    let director_pending_bg = tavern.director_pending;
    let next_turn = tavern.turn + 1;
    let last_plan_turn = tavern
        .director_plan
        .as_ref()
        .map(|p| p.updated_turn);
    let run_mode = pack.stage_director.run_policy.mode.trim().to_string();
    let interval_turns = pack.stage_director.run_policy.interval_turns;
    let last_turn_had_check = tavern
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "assistant")
        .map(|m| m.content.contains("【检定结果】"))
        .unwrap_or(false);
    let director_instruct_bg = director_pending_bg
        || match run_mode.as_str() {
            "interval" => kaleido_core::director_due(&run_mode, interval_turns, next_turn, last_plan_turn),
            "on_demand" => last_turn_had_check,
            _ => false,
        };

    // G3 opening plan is intentionally scheduled after the first turn is persisted.
    // Scheduling it here mutates updated_at during the initial turn CAS window and
    // can turn a valid request into a ghost 409. Later turns remain eligible.
    if tavern.turn > 0
        && kaleido_core::opening_plan_due(tavern.turn, tavern.director_plan.is_some(), tavern.opening_seeded)
    {
        let group_g3 = state.director_tasks.clone();
        if group_g3.acquire(session_id, "opening_plan") {
            tavern.director_task = Some("opening_plan".into());
            let state_g3 = state.clone();
            let session_id_g3 = session_id.to_string();
            let pack_id_g3 = tavern.pack_id.clone();
            tokio::spawn(async move {
                let outcome: Result<Option<DirectorPlan>, String> = async {
                    let sess = match state_g3.sessions_tavern.get(&session_id_g3) {
                        Ok(s) => s,
                        Err(e) => return Err(format!("load session failed: {e}")),
                    };
                    if sess.pack_missing {
                        return Err("session is read-only (pack missing)".into());
                    }
                    let pack = match state_g3.packs.get(&pack_id_g3) {
                        Ok(p) => p,
                        Err(e) => return Err(format!("load pack failed: {e}")),
                    };
                    generate_director_plan_llm(&state_g3, &sess, &pack).await
                }
                .await;
                let mut fresh = match state_g3.sessions_tavern.get(&session_id_g3) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!(error = %e, %session_id_g3, "st opening_plan bg: load session failed");
                        group_g3.release(&session_id_g3);
                        return;
                    }
                };
                fresh.director_task = None;
                match outcome {
                    Ok(Some(plan)) => {
                        tracing::info!(%session_id_g3, "st opening_plan bg: 开局导演规划生成成功");
                        fresh.director_plan = Some(plan);
                        fresh.director_pending = false;
                    }
                    Ok(None) => {
                        tracing::debug!(%session_id_g3, "st opening_plan bg: 导演 LLM 未配置，跳过开局规划");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, %session_id_g3, "st opening_plan bg: 开局导演规划生成失败，不阻断");
                    }
                }
                // CAS 写回：base 取本次 fresh 读的 revision，避免覆盖并发落盘（回合 worker 等）
                let base_rev = fresh.updated_at.clone();
                if let Err(e) = state_g3.sessions_tavern.save_with_revision(fresh, &base_rev) {
                    tracing::warn!(error = %e, %session_id_g3, "st opening_plan bg: CAS save 冲突，计划丢弃（下回合可重新生成）");
                }
                group_g3.release(&session_id_g3);
            });
        } else {
            tracing::debug!(%session_id, "st opening_plan: 后台任务已在跑，跳过重复登记");
        }
    }

    // S5 (吞噬 denova event_package): 每回合开始时抽取事件卡（若 pack 配置了事件包）。
    // seed 取 session_id + next_turn 的确定性哈希：同会话同回合可复现，跨会话/回合不重复。
    if pack.event_packages.is_empty() {
        tavern.last_event = None;
    } else {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&tavern.session_id, &mut h);
        std::hash::Hash::hash(&next_turn, &mut h);
        let seed = h.finish();
        // B1 首回合保护：turn=1 不抽事件卡（穿越茫然开幕不被后期事件污染）。
        // A2 按章过滤：传当前 chapter_cursor，卡标 chapter_range 时只抽覆盖当前章的卡；
        // 无标注卡不受影响（A3 兼容旧 pack）。
        let current_chapter = tavern.chapter_cursor.as_deref();
        let event_pick = if next_turn <= 1 {
            None
        } else {
            kaleido_core::pick_event_card(&pack, seed, next_turn, tavern.last_event.as_ref(), current_chapter)
        };
        // G7 冷却：以最近一次抽取记录做冷却参考（turn 差 < cooldown_turns 排除同卡）；None = 无冷却限制
        match event_pick {
            Some((pkg, card)) => {
                tavern.last_event = Some(kaleido_core::EventLogEntry {
                    turn: next_turn,
                    package_id: pkg.id.clone(),
                    card_id: card.id.clone(),
                    title: card.title.clone(),
                    prompt: card.prompt.clone(),
                    created_at: Utc::now().to_rfc3339(),
                    type_name: card.type_name.clone(),
                    category: card.category.clone(),
                    intensity: card.intensity.clone(),
                });
            }
            None => tavern.last_event = None,
        }
    }

    // Chapter body
    let chapter_body = if let Some(ch_id) = &tavern.chapter_cursor {
        // Find the chapter's body_path
        if let Some(ch) = pack.chapters.iter().find(|c| c.id == *ch_id) {
            if !ch.body_path.is_empty() {
                state
                    .packs
                    .read_chapter_body(&tavern.pack_id, &ch.body_path)
                    .unwrap_or_default()
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // 3b. Compute query embedding for memory RAG (ST-21)
    let query_embedding: Option<Vec<f32>> = {
        let last_user = tavern.messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.as_str())
            .unwrap_or("");
        let node_desc = tavern.node_id.as_deref().unwrap_or("");
        if !last_user.is_empty() || !node_desc.is_empty() {
            let text = format!("{} {}", last_user, node_desc);
            if let Some(eb) = &state.embedding_base {
                let eb = eb.clone();
                let client = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .pool_idle_timeout(std::time::Duration::from_secs(4))
        .pool_max_idle_per_host(2)
                    .timeout(StdDuration::from_secs(10))
                    .build()
                    .unwrap_or_default();
                match crate::llm_stream::get_embedding(&eb, &text, &client).await {
                    Ok(emb) => Some(emb),
                    Err(e) => {
                        tracing::warn!(error=%e, "embedding failed, falling back to token-level");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        }
    };
    Ok((pack, director_instruct_bg, chapter_body, next_turn, query_embedding))
}

/// [M1 2026-10-07] 注入 router（自 museai-server 移植）：一次小模型 JSON 调用决定
/// 本回合注什么。失败/超时/关闭 → fallback 全量（零回归）。默认关闭，
/// `KALEIDO_INJECT_ROUTER=1` 启用；`KALEIDO_INJECT_SHADOW=1` 影子模式（跑但不生效）。
async fn run_inject_router(
    state: &AppState,
    pack: &StoryPack,
    tavern: &kaleido_core::TavernSession,
    user_input: &str,
) -> Option<crate::inject_router::InjectPlan> {
    use crate::inject_router as ir;
    if !ir::InjectPlan::enabled() {
        return None;
    }
    let t0 = std::time::Instant::now();
    let ceiling = match tavern.content_tier {
        kaleido_core::ContentTier::Safe => ir::ContentTierLite::Safe,
        kaleido_core::ContentTier::Standard => ir::ContentTierLite::Standard,
        kaleido_core::ContentTier::Open => ir::ContentTierLite::Open,
    };
    let allowed_ids: Vec<String> =
        pack.characters.iter().map(|c| c.id.clone()).collect();
    let present_names: Vec<String> = pack
        .characters
        .iter()
        .filter(|c| tavern.present_character_ids.iter().any(|p| p == &c.id))
        .map(|c| c.name.clone())
        .collect();
    let recent: Vec<String> = tavern
        .messages
        .iter()
        .rev()
        .take(3)
        .rev()
        .map(|m| m.content.clone())
        .collect();
    let chapter = tavern.chapter_cursor.clone().unwrap_or_default();
    let (sys, usr) = if ir::mini_mode() {
        (
            ir::build_router_mini_system_prompt(ceiling.as_str()),
            ir::build_router_mini_user_prompt(user_input),
        )
    } else {
        (
            ir::build_router_system_prompt(ir::ROUTER_BLOCKS, ceiling.as_str()),
            ir::build_router_user_prompt(user_input, &chapter, &present_names, &recent),
        )
    };
    let router_model = std::env::var("KALEIDO_INJECT_ROUTER_MODEL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let router_timeout_secs: u64 = std::env::var("KALEIDO_INJECT_ROUTER_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|&n| (1..=120).contains(&n))
        .unwrap_or(15);

    let base = state.llm_base.clone().unwrap_or_default();
    let key = state.llm_key.clone().unwrap_or_default();
    let model = router_model.unwrap_or_else(|| state.llm_model.clone());
    let http = reqwest::Client::builder().no_proxy().build().unwrap_or_default();
    let llm_fut = crate::llm_stream::chat_completion(
        &base, &key, &model, &sys, &usr, 0.2, 512, router_timeout_secs, &http,
    );
    let llm_result = tokio::time::timeout(
        std::time::Duration::from_secs(router_timeout_secs),
        llm_fut,
    )
    .await;
    let (raw, err): (Option<String>, Option<String>) = match llm_result {
        Err(_) => (None, Some(format!("router timeout ({router_timeout_secs}s)"))),
        Ok(Err(e)) => (None, Some(format!("router llm error: {e}"))),
        Ok(Ok(s)) => (Some(s), None),
    };
    let plan = match raw {
        Some(s) => match ir::parse_plan(&s) {
            Ok(mut p) => {
                p = ir::clamp_plan(p, &allowed_ids, ceiling, ir::ROUTER_BLOCKS, ir::CORE_BLOCKS);
                p.error = err;
                p
            }
            Err(e) => {
                let mut f = ir::InjectPlan::default();
                f.source = "fallback_full".into();
                f.error = Some(format!("plan parse: {e}"));
                f
            }
        },
        None => {
            let mut f = ir::InjectPlan::default();
            f.source = "fallback_full".into();
            f.error = err;
            f
        }
    };
    tracing::info!(
        plan_source = %plan.source,
        intent = %plan.intent,
        latency_ms = t0.elapsed().as_millis() as u64,
        error = ?plan.error,
        "inject router plan"
    );
    Some(plan)
}

/// [P9+ 2026-10-06 计划外补课] start_turn 步骤 4（system prompt 构建）：基础 prompt +
/// U9 风格指南 + P0 资产块 + P1-3 原著剖析 + S4 导演指令 + S7 混合检索召回。
/// 原 start_turn 内联代码逐行迁出；scribe_whispers 用完即清副作用保留原位。
pub(crate) async fn build_turn_system_prompt(
    state: &AppState,
    pack: &StoryPack,
    tavern: &mut kaleido_core::TavernSession,
    session_id: &str,
    workspace_id: &str,
    chapter_body: &str,
    director_instruct_bg: bool,
    query_embedding: Option<Vec<f32>>,
) -> String {
    // 4. Build system prompt
    kaleido_core::ensure_focus_character(tavern);
    // MCP 外设工具清单（吸收自 Liyuan mcp.ts）
    let mcp_tools = crate::tavern_mcp::list_tools_cached(state.auth.data_root().root()).await;
    // P0 闭环: 调用点预载伏笔（weight 前 15 + 字符预算），保持 build_tavern_system_prompt 纯函数。
    let foreshadow_block = preload_foreshadow_block(&state, &pack, &tavern);
    tracing::info!(
        foreshadow_chars = foreshadow_block.as_ref().map(|s| s.chars().count()).unwrap_or(0),
        pack = %pack.id,
        "foreshadow preload",
    );
    let mut system_prompt = build_tavern_system_prompt(&pack, &tavern, &chapter_body, &state.auth.data_root().cross_session_dir(), query_embedding, &mcp_tools, foreshadow_block.as_deref());
    // Scribe 低语用完即清（本回合已注入 prompt；下回合由后台提取重写）
    if !tavern.scribe_whispers.is_empty() {
        tracing::info!(n = tavern.scribe_whispers.len(), "st scribe: whispers consumed");
        tavern.scribe_whispers.clear();
    }
    // U9 (吞噬 Openwrite O5): 风格指南注入 —— 参考库启用的 style_guide 追加到 system prompt。
    // 零 LLM 依赖（规则版 evidence 合成）；未启用/无指南时零开销。
    {
        let style_block = crate::reference_library::ReferenceLibraryStore::new(state.auth.data_root().root()).injection_block();
        if !style_block.is_empty() {
            system_prompt = format!("{}\n\n{}", system_prompt, style_block);
        }
    }
    // 吞噬资产接线（P0）: 账本/情感曲线/角色弧/关系演化注入 —— 全启发式零 LLM，
    // 调用点预载（同 foreshadow 模式），无资产时零开销。参考库风格指南之后注入。
    // [M1 2026-10-07] 注入 router：默认关闭（零回归）；开启时按 plan 做块开关/召回预算。
        // [M1] user_input = 本回合用户消息（build 前已持久化为最后一条 user 消息）
    let user_input = tavern
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let inject_plan = run_inject_router(state, pack, tavern, &user_input).await;
    {
        let asset_block = preload_asset_blocks(&state, &pack, &tavern, &chapter_body, inject_plan.as_ref());
        if let Some(ab) = asset_block {
            system_prompt = format!("{}\n\n{}", system_prompt, ab);
        }
    }
    // P1-3 (吞噬 denova outline 牵引): 原著剖析注入剧情 prompt —— 每回合给剧情模型原著素材，
    // 与导演指令注入独立（导演调度命中才注入；此处 best-effort 恒注入，无 outline 产物时零开销）。
    {
        let outline_clip = read_outline_clip(state, pack, workspace_id);
        if !outline_clip.is_empty() {
            system_prompt = format!(
                "{}\n\n## 原著剖析（大纲产物，用于主线牵引）\n{}",
                system_prompt, outline_clip
            );
        }
    }
    // S4 (吞噬 denova director_plan): 导演计划生成指令（ST-26 指令）—— 调度命中时附加
    if director_instruct_bg {
        let node = tavern
            .node_id
            .as_deref()
            .and_then(|nid| pack.nodes.iter().find(|n| n.id == *nid));
        let beats: Vec<&str> = node
            .map(|n| n.locked_beats.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        let mut inst = String::from(
            "【导演计划】指令：若剧情进入新阶段且需要导演计划，请输出块：【导演计划】{\"goal\":\"…\",\"pressure\":\"…\",\"cost\":\"…\",\"hits_beats\":[\"<仅可引用当前节点 locked_beats 原文>\"]}；无需更新则输出：【导演计划】none",
        );
        if !beats.is_empty() {
            inst.push_str(&format!("。当前节点 locked_beats 原文：{}", beats.join("；")));
        }
        inst.push_str("。导演计划仅为叙事意图，绝不改写 locked_beats 已锁定的硬事实。");
        // P1-3 (吞噬 denova outline 牵引): 注入原著剖析（outline/ 产物）到导演指令。
        // best-effort：尝试 pack.title 同名 / outline.md / start.md；失败静默跳过。
        let outline_clip = read_outline_clip(state, pack, workspace_id);
        if !outline_clip.is_empty() {
            inst.push_str(&format!("\n[原著剖析]\n{}", outline_clip));
        }
        // G6 (吞噬 denova interactiveDirectorEventCatalog): 事件目录注入 —— 让导演知道有哪些牌可打，
        // 计划可引用目录中的事件作后续张力/铺垫来源，但不得强制本回合触发（保持叙事自由）。
        let event_catalog = build_event_catalog_block(&pack);
        if !event_catalog.is_empty() {
            inst.push_str(&format!(
                "\n{}\n注：导演计划可引用事件目录中的事件作为后续张力/铺垫来源，但不得强制本回合触发（保留玩家叙事自由）。",
                event_catalog
            ));
        }
        system_prompt = format!("{}\n\n{}", system_prompt, inst);
    }

    // S7 (P1-1): history vector recall — query session vector index with recent context,
    // inject top hits into system prompt so compacted/archived details are recoverable.
    {
        let sess_key = format!("sess-{session_id}");
        let s7_idx = state.vector_index.load(&sess_key);
        if !s7_idx.entries.is_empty() {
            // recent context = last few messages (exclude the incoming user message)
            let qtext: Vec<String> = tavern
                .messages
                .iter()
                .rev()
                .take(6)
                .filter_map(|m| {
                    let c = m.content.trim();
                    if c.is_empty() || c.starts_with('[') || c.starts_with("（第") {
                        None
                    } else {
                        Some(c.to_string())
                    }
                })
                .collect();
            let qtext = qtext.join("\n");
            if !qtext.trim().is_empty() {
                // S7 改进① (2026-08-18): query 叠加当前节点剧情要点（locked_beats + 节点标题）。
                // 使久远关键剧情（承诺/约定/关系确立）在剧情推进到相关节点时能语义召回，
                // 而非只依赖"最近消息"——此前长间距关键事件因 query 无剧情导向而漏召回。
                // clone 而非 move：下方 6699 的 `queries = vec![qtext.clone()]` 仍需原 qtext
                let mut qtarget = qtext.clone();
                if let Some(nid) = tavern.node_id.as_deref() {
                    if let Some(node) = pack.nodes.iter().find(|n| n.id == nid) {
                        qtarget = s7_attach_plot_scope(qtarget, Some((&node.title, &node.locked_beats)));
                    }
                }
                let qtext2 = qtarget;
                match tokio::task::spawn_blocking(move || crate::embed_local::embed_one(&qtext2)).await {
                    Ok(Ok(qv)) => {
                        // [morphling Wave B1 2026-08-16] 混合检索升级（吸收自 SillyTavern-BakemonoMemory
                        // hybrid-retrieval）：语义(余弦) + 词法(IDF/中文ngram) + 关键词三路候选并集。
                        // 修复纯向量阈值漏召回：低相似度但词法命中的旧剧情（如「素描」「画室」）也能召回。
                        let records: Vec<kaleido_core::bakemono_retrieval::HybridRecord> = s7_idx
                            .entries
                            .iter()
                            .filter(|e| !e.vector.is_empty())
                            .map(|e| kaleido_core::bakemono_retrieval::HybridRecord {
                                id: e.uid.clone(),
                                message_id: None,
                                title: None,
                                summary: None,
                                text: e.text.clone(),
                                embedding_score: Some(kaleido_core::vector_cosine_similarity(&qv, &e.vector)),
                            })
                            .collect();
                        if !records.is_empty() {
                            let queries = vec![qtext.clone()];
                            let candidates = kaleido_core::bakemono_retrieval::select_hybrid_candidates(
                                &records,
                                &queries,
                                &[],
                                &kaleido_core::bakemono_retrieval::CandidateOptions {
                                    embedding_threshold: 0.42,
                                    candidate_count: 8,
                                    ..Default::default()
                                },
                            );
                            let lines: Vec<String> = candidates
                                .iter()
                                .take(4)
                                .map(|c| c.record.text.clone())
                                .collect();
                            if !lines.is_empty() {
                                // S7 改进② (2026-08-18): 注入硬度分级——命中的旧对话含剧情关键信号
                                //（承诺/约定/答应/约好）时升级为硬约束（同"剧情连续性守卫"语气），
                                // 否则维持"按需沿用"软参考。关键事实不丢、不削弱模型自由度。
                                let joined = lines.join("\n\n");
                                let head = s7_recall_title(&joined);
                                let recall = format!("{}\n{}", head, joined);
                                system_prompt = format!("{}\n\n{}", system_prompt, recall);
                                info!(session = %session_id, hits = candidates.len(), "S7 hybrid recall injected");
                            }
                        }
                    }
                    Ok(Err(e)) => warn!(error = %e, "S7 recall embed failed"),
                    Err(e) => warn!(error = %e, "S7 recall join failed"),
                }
            }
        }
    }
    system_prompt
}
