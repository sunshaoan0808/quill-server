//! [2026-10-06 P6] //! 回合后记忆链（post_turn_memory_phase）。
#![allow(unused_imports)]
use super::super::*;

#[allow(clippy::too_many_arguments)]
/// [P5 2026-10-05 从 start_turn 内联提取] 回合后记忆链：ST-12 启发式 L2/L3、ST-30
/// 角色识别兜底（后台 LLM）、ST-14 LLM 抽取、ST-19 跨会话、ST-22 向量、P2 叙界守卫
/// （high 打回 llm_advance_to）、AZ 实时作者区、U3 场记卡、U11 计账、M-2/M-3 记忆
/// 补强、章节摘要提炼、节点推进（try_advance_node）。行为逐字节不变。
pub(crate) async fn post_turn_memory_phase(
    mut sess: &mut kaleido_core::TavernSession,
    ctx: &mut TurnCtx,
    packs_store: &kaleido_core::PackStore,
    sessions_store: &kaleido_core::TavernSessionStore,
    state: &AppState,
    jobs: &kaleido_core::JobStore,
    full_text: &str,
    pack: &StoryPack,
    chapter_body: &str,
    roster_names: Option<Vec<String>>,
    works_store: &kaleido_core::WorksFs,
    engine_tag: kaleido_core::EngineTag,
    llm_advance_to: &mut Option<String>,
) {
        // [P7] TurnCtx 收编：散装值参数自 ctx 取出（Copy 先取值、其余借字段引用），
        // 块体与原实现逐行一致。
        let turn_started = ctx.turn_started;
        let quality_bg = ctx.quality_bg;
        let resumed_bg = ctx.resumed_bg;
        let (user_bg, base_bg, key_bg, model_bg, prov_bg, session_id_bg, run_id_bg,
            workspace_id_bg, cross_dir, embedding_base_bg, used_fallback, extra_llm_calls,
            sys_prompt_bg) =
            (&ctx.user_bg, &ctx.base_bg, &ctx.key_bg, &ctx.model_bg, &ctx.prov_bg,
             &ctx.session_id_bg, &ctx.run_id_bg, &ctx.workspace_id_bg, &ctx.cross_dir,
             &ctx.embedding_base_bg, &mut ctx.used_fallback, &mut ctx.extra_llm_calls,
             &ctx.sys_prompt_bg);
    let elapsed_ms = || turn_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
                // Post-turn extraction (memory + node advance)
                if let Ok(pack) = packs_store.get(&sess.pack_id) {
                    // ST-12: heuristic L2/L3 (+ light L1) from this turn text
                    let mut ext = kaleido_core::heuristic_l2_l3_from_turn(
                        &sess,
                        user_bg,
                        full_text,
                    );

                    // ST-30 (2026-08-15 根治兜底): 异步 LLM 识别正文外角色——语义理解
                    // 替代切词启发式，根治叙述/动作形态点名漏检（「李铁柱的声音比雨声重」）
                    // 与切碎短语误报。实测（2026-08-15）生成端 <角色清单> 内容不可靠
                    // （deepseek 形式遵守但漏列叙述点名角色）→ 本兜底**每回合都跑**，
                    // 与清单互补：清单精确比对 + 兜底补漏。best-effort 非阻塞。
                    if !full_text.trim().is_empty()
                        && !base_bg.trim().is_empty()
                        && !key_bg.trim().is_empty()
                    {
                        *extra_llm_calls += 1;
                        let mut known_v: Vec<String> = pack
                            .characters
                            .iter()
                            .map(|c| c.name.clone())
                            .collect();
                        for l in &pack.lore_entries {
                            if let Some(kw) = l.get("keywords").and_then(|k| k.as_array()) {
                                for v in kw {
                                    if let Some(str) = v.as_str() {
                                        known_v.push(str.to_string());
                                    }
                                }
                            }
                        }
                        let known_joined = known_v.join("、");
                        let body_clip: String = full_text.chars().take(1500).collect();
                        let sys_roster = "你是小说角色识别器。识别叙事文本中实际出场的人物专名（姓名/称呼/外号）。\
                            严禁输出句子片段、短语、普通名词、形容词或口语碎片（如「明日柜上」「却带着点」都不是人名）。\
                            只输出 JSON 数组，如 [\"王麻子\",\"李铁柱\"]；没有则输出 []。";
                        let user_roster = format!(
                            "已知角色名单（出现不算越界，不要列出）：{}\n叙事文本：\n{}",
                            known_joined, body_clip
                        );
                        let base_llm_r = base_bg.to_string();
                        let key_llm_r = key_bg.to_string();
                        let model_llm_r = model_bg.to_string();
                        let prov_kind_r = prov_bg.to_string();
                        let sess_load_r = sessions_store.clone();
                        let sid_r = session_id_bg.to_string();
                        let e_tag_r = engine_tag;
                        tokio::spawn(async move {
                            // P3.1：首发失败立即重试一次（bg_llm_with_retry）。
                            match bg_llm_with_retry(
                                &base_llm_r,
                                &key_llm_r,
                                &model_llm_r,
                                &prov_kind_r,
                                sys_roster,
                                &user_roster,
                                0.1,
                                16384,
                                45,
                            )
                            .await
                            {
                                Ok(llm_text) => {
                                    info!(%sid_r, raw = llm_text.chars().take(150).collect::<String>(), "st roster llm extract raw");
                                    let names: Vec<String> = serde_json::from_str(llm_text.trim())
                                        .unwrap_or_else(|_| {
                                            // 兼容纯文本/顿号列表：按非 JSON 兜底解析
                                            llm_text
                                                .split(['"', '，', '、', ',', '\n'])
                                                .map(|s| s.trim().trim_matches(['[', ']', '"', '，', '、', ',', '。', ' ']).to_string())
                                                .filter(|s| s.chars().count() >= 2)
                                                .collect()
                                        });
                                    // 2026-08-16 统一来源根治: 兜底名单也过滤非人名碎片——
                                    // 散文解析会 split 出「就是/我们/按理/带着点」等虚词（实测误报）。
                                    // 判定条件(全满足): >=2字 && 不含功能字 && 非代词/虚词 && 非称呼后缀。
                                    let names: Vec<String> = names
                                        .into_iter()
                                        .filter(|n| {
                                            if n.chars().count() < 2 {
                                                return false;
                                            }
                                            if n.chars().any(is_functional_char) {
                                                return false;
                                            }
                                            if matches!(
                                                n.as_str(),
"就是" |
                                                "我们" |
                                                "你们" |
                                                "他们" |
                                                "她们" |
                                                "大家" |
                                                "现在" |
                                                "然后" |
                                                "不过" |
                                                "按理" |
                                                "转而" |
                                                "改口" |
                                                "带着点" |
                                                "玩家" |
                                                "这个" |
                                                "那个" |
                                                "什么" |
                                                "怎么" |
                                                "于是" |
                                                "接着" |
                                                "随后" |
                                                "只见" |
                                                "原来" |
                                                "果然" |
                                                "忽然" |
                                                "突然" |
                                                "终于" |
                                                "毕竟" |
                                                "虽然" |
                                                "但是" |
                                                "可是" |
                                                "因为" |
                                                "所以" |
                                                "如果" |
                                                "的话" |
                                                "一下" |
                                                "起来" |
                                                "过去" |
                                                "出来" |
                                                "进来" |
                                                "回去" |
                                                "下去" |
                                                "时候" |
                                                "地方" |
                                                "东西" |
                                                "事情" |
                                                "感觉" |
                                                "声音" |
                                                "样子" |
                                                "表情" |
                                                "语气" |
                                                "意思" |
                                                "神色" |
                                                "目光" |
                                                "视线" |
                                                "脚步" |
                                                "动作"
                                            ) {
                                                return false;
                                            }
                                            if TITLE_SUFFIXES.iter().any(|t| n.ends_with(t)) {
                                                return false;
                                            }
                                            true
                                        })
                                        .collect();
                                    if let Ok(mut s) = sess_load_r.get(&sid_r) {
                                        let known_set: std::collections::HashSet<String> =
                                            known_v.iter().cloned().collect();
                                        for n in names {
                                            let n = n.trim().to_string();
                                            // 正文（本回合最后一条 assistant）确实出现才判越界
                                            // （LLM 幻觉名单防误报；历史出现过 = 已 known/已报过）
                                            if n.chars().count() >= 2
                                                && !known_set.contains(&n)
                                                && s.messages.last().map(|m| m.content.contains(&n)).unwrap_or(false)
                                            {
                                                s.guard_events.push(format!(
                                                    "[{}][{}] {}{}",
                                                    "high",
                                                    "人物",
                                                    format!("原著外对象「{}」（LLM 兜底识别）", n),
                                                    if e_tag_r == kaleido_core::EngineTag::Canon { "" } else { "（记录）" }
                                                ));
                                                info!(%sid_r, name=%n, "st roster llm fallback: 检出外角色");
                                            }
                                        }
                                        let _ = sess_load_r.save(s);
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error=%e, %sid_r, "st roster llm fallback failed");
                                }
                            }
                        });
                    }

                    // ST-14: LLM extraction every 3 turns (best-effort, non-blocking)
                    if sess.turn > 0 && sess.turn % 3 == 0 && !base_bg.trim().is_empty() && !key_bg.trim().is_empty() {
                        info!(turn=sess.turn, "llm extraction trigger condition met");
                        // U11: 非阻塞抽取计入本回合 LLM 调用数。
                        *extra_llm_calls += 1;
                        // Build recent conversation context for the LLM
                        let mut recent_lines = Vec::new();
                        for msg in sess.messages.iter().rev().take(6).rev() {
                            let role = if msg.role == "user" { "玩家" } else { "叙事" };
                            let content = msg.content.chars().take(300).collect::<String>();
                            recent_lines.push(format!("[{}] {}", role, content));
                        }
                        let recent_text = recent_lines.join("\n");
                        let sys_extract = "你是一个故事记忆提取器。只输出JSON。\n\
                                                    events: [{id,kind,summary,actors[],nodeId}] kind=meet|promise|secret|conflict|romance|other summary=30字\n\
                                                    edges: [{from,to,rel,note,turn}]\n\
                                                    from/to=角色名（当前在场角色或与玩家互动的角色）；\n\
                                                    rel=当前关系类型（开放枚举，从近期对话判定关系演变）：\n\
                                                    interact|tension|trust|secrets|romance 兼容旧值，外加细粒度类型：\n\
                                                    暧昧|情人|心动|依赖|疏远|敌对|母子|父女|夫妻|姐弟|兄妹|师徒|好友|同事|上下级 等。\n\
                                                    关系**演变**时（如母子→暧昧、友好→敌对）用最新关系类型，note 一句话说明演变/证据（10字）；\n\
                                                    无演变则沿用既有类型；纯普通对话无关系变化时不输出该边。\n\
                                                    facts: [短事实字符串]\n\
                                                    {\"events\":[{\"id\":\"e-1\",\"kind\":\"meet\",...}],\"edges\":[...],\"facts\":[...]}";
                        let user_extract = format!(
                            "节点：{}\n当前角色：{}\n近期对话：\n{}\n",
                            sess.node_id.as_deref().unwrap_or("?"),
                            sess.present_character_ids.join(","),
                            recent_text,
                        );

                        // Fire extraction as a separate spawned task so it doesn't block post-turn
                        let base_llm = base_bg.to_string();
                        let key_llm = key_bg.to_string();
                        let model_llm = model_bg.to_string();
                        let sess_load = sessions_store.clone();
                        let sid = session_id_bg.to_string();
                        let user_clone = user_bg.to_string();
                        let cross_dir_ext = cross_dir.clone();
                        let eb_llm = embedding_base_bg.clone();
                        let prov_kind_e = prov_bg.to_string();
                        tokio::spawn(async move {
                            // P3.1：首发失败立即重试一次（bg_llm_with_retry）。
                            match bg_llm_with_retry(
                                &base_llm, &key_llm, &model_llm, &prov_kind_e,
                                &sys_extract, &user_extract, 0.1, 8192, 60,
                            )
                            .await
                            {
                                Ok(llm_text) => {
                                    info!(%sid, raw = llm_text.chars().take(200).collect::<String>(), "llm extraction raw response");
                                    if let Ok(mut s) = sess_load.get(&sid) {
                                        // [审计修复 核心叙事引擎-6] 解析失败不再静默丢记忆：
                                        // parse 返回 None（内部已 warn）时走 heuristic L2/L3 兜底，
                                        // 保证本回合仍有事件/事实入库。
                                        let llm_ext = match kaleido_core::parse_extraction_response(&llm_text) {
                                            Some(ext) => ext,
                                            None => {
                                                let last_asst = s
                                                    .messages
                                                    .iter()
                                                    .rev()
                                                    .find(|m| m.role == "assistant")
                                                    .map(|m| m.content.as_str())
                                                    .unwrap_or("");
                                                tracing::warn!(%sid, "llm extraction parse failed, using heuristic L2/L3 fallback");
                                                kaleido_core::heuristic_l2_l3_from_turn(&s, &user_clone, last_asst)
                                            }
                                        };
                                        let e_tag = kaleido_core::classify_engine_tag(&user_clone);
                                        kaleido_core::apply_extraction(&mut s, &llm_ext, e_tag);
                                        // ST-19: persist LLM extraction to cross-session memory
                                        // [审计修复 核心叙事引擎-7] persist 失败不再被静默吞掉
                                        if let Err(e) = kaleido_core::persist_cross_session(
                                            &cross_dir_ext,
                                            &s.pack_id,
                                            &sid,
                                            s.turn,
                                            s.node_id.as_deref(),
                                            &s.present_character_ids,
                                            &llm_ext.new_events,
                                            &llm_ext.new_facts,
                                            &llm_ext.new_edges,
                                        ) {
                                            tracing::warn!(error = %e, %sid, "persist cross-session memory (llm extraction) failed");
                                        }
                                        // ST-22: compute embeddings for LLM-extracted events
                                        if let Some(eb) = &eb_llm {
                                            if !eb.trim().is_empty() && !llm_ext.new_events.is_empty() {
                                                for ev in &llm_ext.new_events {
                                                    if let Some(stored) = s.memory_l2.events.iter_mut().find(|e| e.id == ev.id) {
                                                        if !stored.embedding.is_empty() {
                                                            continue;
                                                        }
                                                        let text = format!("{} {}", stored.kind, stored.summary);
                                                        let eb_c = eb.clone();
                                                        let client_ev = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .pool_idle_timeout(std::time::Duration::from_secs(4))
        .pool_max_idle_per_host(2)
                                                            .timeout(StdDuration::from_secs(10))
                                                            .build().unwrap_or_default();
                                                        match crate::llm_stream::get_embedding(&eb_c, &text, &client_ev).await {
                                                            Ok(emb) => stored.embedding = emb,
                                                            Err(e) => tracing::warn!(error=%e, id=%ev.id, "llm event embedding failed"),
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        let _ = sess_load.save(s);
                                        info!(%sid, "llm extraction applied");
                                    } else {
                                        warn!("llm extraction no session for {sid}");
                                    }
                                }
                                Err(e) => {
                                    warn!("llm extraction fail for {sid}: {e}");
                                }
                            }
                        });
                    }

                    // P2 (叙界守卫): 生成后多维守卫 — 扩展现 ST-26 人名黑名单为
                    // 人物/节拍/出场/大纲四维检查；high → 打回（阻止推进），medium → 仅提示。
                    // 2026-08-14 修复：检测全模式跑（原绑死 engine_tag==Canon 只覆盖剧情推进模式，
                    // 说话/行为 90% 回合无保护）；打回保留 Canon 语义（跳章推进才阻止）。
                    let guard_violations = {
                        let known: std::collections::HashSet<String> = {
                            let mut s = std::collections::HashSet::new();
                            for c in &pack.characters {
                                s.insert(c.name.clone());
                                s.insert(c.id.clone());
                            }
                            // [守卫降噪] 已登场人物不再是“原著外对象”（自由剧情无原著）
                            for k in sess.pockets.keys().chain(sess.relationships.keys()) {
                                s.insert(k.clone());
                            }
                            for c in sess.journal.cards.iter().map(|c| c.character_id.clone()) {
                                s.insert(c);
                            }
                            for r in sess.growth.rings.iter().map(|r| r.character.clone()) {
                                s.insert(r);
                            }
                            for l in &pack.lore_entries {
                                if let Some(kw) = l.get("keywords").and_then(|k| k.as_array()) {
                                    for v in kw {
                                        if let Some(str) = v.as_str() {
                                            s.insert(str.to_string());
                                        }
                                    }
                                }
                            }
                            s
                        };
                        let node = sess
                            .node_id
                            .as_deref()
                            .and_then(|nid| pack.nodes.iter().find(|n| n.id == *nid));
                        let present_names: Vec<String> = node
                            .map(|n| {
                                n.present_characters
                                    .iter()
                                    .map(|pc| {
                                        pack.characters
                                            .iter()
                                            .find(|c| c.id == *pc || c.name == *pc)
                                            .map(|c| c.name.clone())
                                            .unwrap_or_else(|| pc.clone())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let locked_beats: Vec<String> = node
                            .map(|n| n.locked_beats.clone())
                            .unwrap_or_default();
                        let chapter_goals: Vec<String> = node
                            .map(|_| {
                                pack.chapters
                                    .iter()
                                    .find(|ch| {
                                        sess.node_id
                                            .as_ref()
                                            .map(|nid| ch.node_ids.iter().any(|x| x == nid))
                                            .unwrap_or(false)
                                    })
                                    .map(|ch| ch.goals.clone())
                                    .unwrap_or_default()
                            })
                            .unwrap_or_default();
                        // [守卫降噪] 状态体现：口袋物品/Journal 卡内容并入检测文本
                        // （信在口袋里躺着也算“体现”，自由剧情常用物传情）
                        let guard_text = {
                            let mut g = full_text.to_string();
                            for (_cid, pk) in &sess.pockets {
                                for it in pk.carrying.iter().chain(pk.worn.iter()) {
                                    // [守卫降噪] 纯中文持有行（拉丁 id 会污染 n-gram，如 narrator→or信）
                                    g.push_str(&format!("\n持有{}。", it.name));
                                }
                            }
                            for c in sess.journal.cards.iter().take(20) {
                                let snip: String = c.content.chars().take(50).collect();
                                g.push_str(&format!("\n[记:{}]", snip));
                            }
                            g
                        };
                        // [主线完结] pack 走完终点 → 自由生长：主线判据静默，只保留人物+双持
                        let exhausted = kaleido_core::is_mainline_exhausted(&pack, &sess);
                        if exhausted {
                            tracing::info!("st guard: 主线已完结·自由生长中（节拍/目标/出场静默）");
                        }
                        let mut vs = guard_narrative(
                            &guard_text,
                            &chapter_body,
                            &known,
                            if exhausted { &[] } else { &present_names },
                            if exhausted { &[] } else { &locked_beats },
                            if exhausted { &[] } else { &chapter_goals },
                            roster_names.as_deref(),
                        );
                        // [声线漂移] 回合诊断 vd_score ≥ 0.6 → med 记录（只记录不阻断）
                        if let Some(dg) = sess.last_turn_diagnostic.as_ref() {
                            if dg.voice_drift_score >= 0.25 && !dg.voice_drift_char.is_empty() {
                                vs.push(GuardViolation {
                                    severity: GuardSeverity::Medium,
                                    dim: "声线",
                                    msg: format!("声线漂移「{}」（{:.0}%）：{}",
                                        dg.voice_drift_char,
                                        dg.voice_drift_score * 100.0,
                                        dg.voice_drift_reasons.join("；")),
                                });
                            }
                        }
                        // [幻觉/冲突] hallu_names/conflicts → 告警（吞噬 AI-Reader-V2 口径；只记录不阻断）
                        if let Some(dg) = sess.last_turn_diagnostic.as_ref() {
                            for h in &dg.hallu_names {
                                vs.push(GuardViolation {
                                    severity: GuardSeverity::Medium,
                                    dim: "幻觉",
                                    msg: format!("提取名「{h}」在正文无 grounding（疑似幻觉，已跳过写入）"),
                                });
                            }
                            for c in &dg.conflicts {
                                let sev = if c.contains("死亡") { GuardSeverity::High } else { GuardSeverity::Medium };
                                vs.push(GuardViolation {
                                    severity: sev,
                                    dim: "冲突",
                                    msg: c.clone(),
                                });
                            }
                        }
                        // [骰审] 伪造/矛盾 → med 记录（只记录不阻断，吞噬 loreweaver Stop 形态降级版）
                        if let Some(dg) = sess.last_turn_diagnostic.as_ref() {
                            if dg.dice_forgery || dg.dice_contradiction {
                                vs.push(GuardViolation {
                                    severity: GuardSeverity::Medium,
                                    dim: "骰审",
                                    msg: format!("骰审命中：{}", dg.dice_reasons.join("；")),
                                });
                            }
                        }
                        // [双持检测] 同一物品在两个角色口袋 = 吃书（turn 17 银锁双持实踩）
                        {
                            let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                            for (cid, pk) in &sess.pockets {
                                for it in pk.carrying.iter().chain(pk.worn.iter()) {
                                    let k = it.name.trim().to_lowercase();
                                    if k.is_empty() { continue; }
                                    if let Some(prev) = seen.get(&k) {
                                        if prev != cid {
                                            vs.push(GuardViolation {
                                                severity: GuardSeverity::High,
                                                dim: "物品",
                                                msg: format!("双持吃书「{}」同时在 {} 与 {} 口袋", it.name, prev, cid),
                                            });
                                        }
                                    } else {
                                        seen.insert(k, cid.clone());
                                    }
                                }
                            }
                        }
                        vs
                    };
                    // P2-2 冲突处理：high 打回（阻止跳章推进，留在当前节点）；记录 guard_events 供排查/展示
                    // 2026-08-14：打回仅在 Canon（剧情推进/跳章）语义下生效——说话/行为模式记录不阻断（无跳章可阻）
                    if !guard_violations.is_empty() {
                        // 2026-08-16 降噪: medium 重复提示（同角色「未出现」每轮刷屏）
                        // 只在首次出现时记录，后续相同 msg 不再追加——保留首次信号，
                        // 避免 pack presentCharacters 过宽时每轮重复噪音。
                        let deduped: Vec<GuardViolation> = guard_violations
                            .iter()
                            .filter(|v| {
                                if v.severity == GuardSeverity::High {
                                    return true;
                                }
                                let fmt = format!("[{}][{}] {}", "med", v.dim, v.msg);
                                !sess.guard_events.iter().any(|e| e.contains(&fmt))
                            })
                            .cloned()
                            .collect();
                        sess.guard_events.extend(
                            deduped
                                .iter()
                                .map(|v| {
                                    format!(
                                        "[{}][{}] {}{}",
                                        if v.severity == GuardSeverity::High { "high" } else { "med" },
                                        v.dim,
                                        v.msg,
                                        if engine_tag == kaleido_core::EngineTag::Canon { "" } else { "（记录）" }
                                    )
                                }),
                        );
                        // G15 (吞噬 denova retainedTurnsForInteractiveCompaction): 保留策略——
                        // 窗口 100 条，超窗优先淘汰 med（high 保底），长会话不无限增长。
                        sess.guard_events = kaleido_core::retain_guard_events(&sess.guard_events, 100);
                        let high_count = guard_violations
                            .iter()
                            .filter(|v| v.severity == GuardSeverity::High)
                            .count();
                        if high_count > 0 && engine_tag == kaleido_core::EngineTag::Canon {
                            *llm_advance_to = None; // 打回：不推进跳章，留在当前节点
                            tracing::warn!(
                                high = high_count,
                                violations = ?guard_violations,
                                "叙界守卫: high 冲突，阻止跳章推进"
                            );
                        } else {
                            tracing::warn!(
                                violations = ?guard_violations,
                                "叙界守卫: medium 提示，不阻断"
                            );
                        }
                    }

                    kaleido_core::try_advance_node(&pack, sess, engine_tag, &mut ext, llm_advance_to.take());
                    kaleido_core::apply_extraction(&mut sess, &ext, engine_tag);
                    // ST-19: persist heuristic extraction to cross-session memory
                    // [审计修复 核心叙事引擎-7] persist 失败不再被静默吞掉
                    if let Err(e) = kaleido_core::persist_cross_session(
                        &cross_dir,
                        &sess.pack_id,
                        session_id_bg,
                        sess.turn,
                        sess.node_id.as_deref(),
                        &sess.present_character_ids,
                        &ext.new_events,
                        &ext.new_facts,
                        &ext.new_edges,
                    ) {
                        tracing::warn!(error = %e, session_id = %session_id_bg, "persist cross-session memory (heuristic extraction) failed");
                    }
                    kaleido_core::ensure_focus_character(&mut sess);
                    let _ = kaleido_core::rotate_focus_character(&mut sess);

                    // ST-22: compute embeddings for new events (BGE semantic cache)
                    if let Some(eb) = &embedding_base_bg {
                        if !eb.trim().is_empty() && !ext.new_events.is_empty() {
                            for ev in &ext.new_events {
                                // Find this event in the session (matched by id)
                                if let Some(stored) = sess.memory_l2.events.iter_mut().find(|e| e.id == ev.id) {
                                    if !stored.embedding.is_empty() {
                                        continue;
                                    }
                                    let text = format!("{} {}", stored.kind, stored.summary);
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
                                        Ok(emb) => stored.embedding = emb,
                                        Err(e) => tracing::warn!(error=%e, id=%ev.id, "event embedding failed"),
                                    }
                                }
                            }
                        }
                    }

                    // P2 (叙界守卫): Canon tag — 注入"回归原著" narrator message
                    if engine_tag == kaleido_core::EngineTag::Canon {
                        let high_person = guard_violations.iter().find(|v| {
                            v.severity == GuardSeverity::High && v.dim == "人物"
                        });
                        let canon_note = if let Some(v) = high_person {
                            format!(
                                "〔回归原著·硬校验〕{}：请立即回到当前节点「{}」对应章节，严格按本章原著情节继续；不要编造新人物、跳章或改写关键事件。",
                                v.msg,
                                ext.advance_to_node_id.as_deref().unwrap_or("?")
                            )
                        } else {
                            format!(
                                "〔回归原著〕剧情回到主线轨道。当前节点：{}。",
                                ext.advance_to_node_id.as_deref().unwrap_or("?")
                            )
                        };
                        let canon_msg = TavernMessage {
                            id: format!("msg-{}", Uuid::new_v4()),
                            role: "assistant".into(),
                            content: canon_note,
                            created_at: Utc::now().to_rfc3339(),
                            options: vec![],
                            engine_tag: None,
                            program: None,
                            reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
            mvu_blocks: Vec::new(),
                        };
                        sess.messages.push(canon_msg);
                    }

                    // L1: scene summary on turns 3, 8, 16...
                    if sess.memory_l1.scene_summary.is_empty()
                        && sess.turn >= 3
                    {
                        sess.memory_l1.scene_summary = format!(
                            "当前场景：{}（第{}回合）",
                            sess.node_id.as_deref().unwrap_or("?"),
                            sess.turn
                        );
                        sess.memory_l1.updated_at_turn = sess.turn;
                    }

                    // U11: 上下文窗口阈值驱动 epoch 压缩（替换机械 turn%8）——
                    // 基于 build_tavern_system_prompt 实际载荷近似（system prompt 字符 + 全量对话
                    // 字符 + 本轮消息 + 初稿），超过阈值才触发；压缩逻辑复用现有实现（零新引擎）。
                    let ctx_chars = estimate_turn_ctx_chars(&sess, &sys_prompt_bg, user_bg, full_text);
                    if should_epoch_compress(&sess, ctx_chars) {
                        // Mechanical trim first, preserving relationship state in the summary
                        // ST-FIX: 弱信号 contains("亲") 会把「母亲」误计为接吻（宿醉事件摘要全含
                        // 「母亲」→ 凭空「接过吻至少15次」），改用强信号词。
                        let kiss_count = sess.memory_l2.events.iter()
                            .filter(|e| e.kind == "romance"
                                || e.summary.contains("接吻") || e.summary.contains("亲嘴")
                                || e.summary.contains("亲吻") || e.summary.contains("亲热")
                                || e.summary.contains("吻"))
                            .count();
                        let rel_line = if kiss_count > 0 {
                            format!(" 关系已确立：至少接过吻（或亲密接触）{} 次，后续亲热描写要体现熟悉感，不要再当第一次。", kiss_count)
                        } else { String::new() };
                        let summary = format!("当前场景：{}（第{}回合记忆压缩）{}", sess.node_id.as_deref().unwrap_or("?"), sess.turn, rel_line);
                        // [V3 2026-08-17] 压缩占位携带章节语义：当前章已有章节摘要
                        // （尤其 manual_edited 确认过）时并入 L1，避免空洞占位污染后续
                        // fallback 提炼；无摘要时保持原占位（由 strip_compression_placeholder
                        // 在读取侧兜底）。diary 累积式（V1）保证此摘要覆盖整章进展。
                        let diary_hint = sess
                            .chapter_diaries
                            .iter()
                            .find(|d| {
                                d.chapter_id == sess.chapter_cursor.as_deref().unwrap_or("")
                                    && !d.summary.trim().is_empty()
                            })
                            .map(|d| {
                                format!(
                                    "\n当前章进展：{}",
                                    d.summary.chars().take(200).collect::<String>()
                                )
                            })
                            .unwrap_or_default();
                        let summary = format!("{summary}{diary_hint}");
                        kaleido_core::apply_memory_compression(&mut sess, &summary);
                        // S7 (P1-1): archive trimmed messages into session vector index BEFORE dropping.
                        // Best-effort; failure only logs. Keeps compacted details recoverable by recall.
                        if !session_id_bg.is_empty() {
                            let sess_key = format!("sess-{session_id_bg}");
                            let archived: Vec<(String, String)> = sess.messages
                                .iter()
                                .filter_map(|m| {
                                    let c = m.content.trim();
                                    if c.is_empty() || c.starts_with('[') || c.starts_with("（第") {
                                        return None;
                                    }
                                    let text = format!("{}: {}", m.role, c);
                                    let uid = format!(
                                        "m-{}-{}",
                                        m.role,
                                        kaleido_core::text_hash(&text).chars().take(10).collect::<String>()
                                    );
                                    Some((uid, text))
                                })
                                .collect();
                            if !archived.is_empty() {
                                let texts: Vec<String> = archived.iter().map(|(_, t)| t.clone()).collect();
                                let vi = state.vector_index.clone();
                                let vi_key = sess_key.clone();
                                let vi_texts = texts.clone();
                                tokio::spawn(async move {
                                    match tokio::task::spawn_blocking(move || crate::embed_local::embed_many(&vi_texts)).await {
                                        Ok(Ok(embeds)) => {
                                            let mut idx = vi.load(&vi_key);
                                            let known: std::collections::HashSet<String> =
                                                idx.entries.iter().map(|e| e.uid.clone()).collect();
                                            let mut added = 0usize;
                                            for ((uid, text), v) in archived.iter().zip(embeds.iter()) {
                                                if !known.contains(uid) && !v.is_empty() {
                                                    idx.entries.push(kaleido_core::VectorIndexEntry {
                                                        uid: uid.clone(),
                                                        world: "history".into(),
                                                        text: text.clone(),
                                                        text_hash: kaleido_core::text_hash(text),
                                                        vector: v.clone(),
                                                    });
                                                    added += 1;
                                                }
                                            }
                                            if added > 0 {
                                                match vi.save(idx) {
                                                    Ok(f) => info!(session = %vi_key, entries = f.entries.len(), "S7 history archived to vector index"),
                                                    Err(e) => warn!(error = %e, "S7 vector archive save failed"),
                                                }
                                            }
                                        }
                                        Ok(Err(e)) => warn!(error = %e, "S7 embed_many failed"),
                                        Err(e) => warn!(error = %e, "S7 embed join failed"),
                                    }
                                });
                            }
                        }
                        // P0-2(审计): 压缩后裁剪 messages 保留最近窗口（与 estimate/prompt 构建一致），
                        // 使 ctx_chars/messages.len() 判据可回落，避免越过阈值后每回合触发压缩。
                        if sess.messages.len() > KEEP_RECENT_MESSAGES {
                            let cut = sess.messages.len() - KEEP_RECENT_MESSAGES;
                            sess.messages.drain(0..cut);
                        }
                        // U11: epoch 代数记账（会话持久化，日志可见触发原因）。
                        sess.epoch += 1;
                        sess.epoch_last_turn = Some(sess.turn);
                        sess.epoch_last_chars = Some(ctx_chars.min(u32::MAX as usize) as u32);
                        info!(turn=sess.turn, epoch=sess.epoch, ctx_chars, "U11 epoch compression applied (context threshold)");

                        // [P6 2026-08-16] 压缩告警：向用户可见侧提示「上下文已压缩，
                        // 早期细节进入摘要」——防止玩家无感跳剧情（宿醉「突然跳到另一个世界」）。
                        let notice = format!(
                            "（系统提示：上下文已压缩，早期细节已归档进记忆摘要。此后剧情以最近 {} 条消息与摘要为准；若需找回早期细节可查看记忆面板或回档。）",
                            KEEP_RECENT_MESSAGES
                        );
                        sess.messages.push(TavernMessage {
                            id: format!("sys-epoch-{}-{}", sess.epoch, sess.turn),
                            role: "assistant".into(),
                            content: notice,
                            created_at: chrono::Utc::now().to_rfc3339(),
                            options: vec![],
                            engine_tag: None,
                            program: None,
                            reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
            mvu_blocks: Vec::new(),
                        });

                        // Non-blocking LLM summarization
                        let (sys_compress, user_compress) = kaleido_core::build_compression_prompt(&sess, &pack);
                        if !sys_compress.trim().is_empty() && !key_bg.trim().is_empty() {
                            // U11: 非阻塞压缩计入本回合 LLM 调用数。
                            *extra_llm_calls += 1;
                            let base_llm2 = base_bg.to_string();
                            let key_llm2 = key_bg.to_string();
                            let model_llm2 = model_bg.to_string();
                            let sess_load2 = sessions_store.clone();
                            let sid2 = session_id_bg.to_string();
                            let prov_kind_c = prov_bg.to_string();
                            tokio::spawn(async move {
                                // P3.1：首发失败立即重试一次（bg_llm_with_retry）。
                                let result = bg_llm_with_retry(
                                    &base_llm2, &key_llm2, &model_llm2, &prov_kind_c,
                                    &sys_compress, &user_compress, 0.1, 16384, 30,
                                )
                                .await;
                                if let Ok(text) = result {
                                    let trimmed = text.trim();
                                    if !trimmed.is_empty() {
                                        if let Ok(mut s) = sess_load2.get(&sid2) {
                                            let summary2 = format!("{}（第{}回合LLM压缩归档）", trimmed, s.turn);
                                            kaleido_core::apply_memory_compression(&mut s, &summary2);
                                            let _ = sess_load2.save(s);
                                            info!(%sid2, "llm memory compression applied");
                                        }
                                    }
                                }
                            });
                        }
                    }
                }

                // AZ-2/AZ-6: realtime author live append (fail-open, never blocks turn)
                if let (Some(live), wid) = (
                    sess.author_live_path.clone(),
                    workspace_id_bg,
                ) {
                    if !live.is_empty() && !wid.is_empty() {
                        crate::author::append_session_live(
                            &works_store,
                            wid,
                            &live,
                            sess.turn,
                            user_bg,
                            full_text,
                            sess.author_live_enabled,
                            sess.author_live_every_n,
                            sess.author_live_write_turns,
                        );
                    }
                }

                // U3: 场记卡——每场摘要变化时持久化快照（去重由 store 保证，不阻塞回合）
                {
                    let _ = state.scene_cards.record_if_changed(
                        &sess.pack_id,
                        &sess.session_id,
                        sess.node_id.as_deref(),
                        sess.turn,
                        &sess.memory_l1.scene_summary,
                    );
                }
                // U11: 回合成本记账 —— 写入 job payload（GET /api/v1/jobs 可见：durationMs /
                // llmCalls / estTokensIn / estTokensOut / estCostUsd / resumed / epoch）+ 会话累计账本。
                // 用现有 merge_job_payload，不改 jobs 文件格式（payload 为旁路字段）。
                {
                    let elapsed_ms_v = elapsed_ms();
                    let est = turn_cost_estimate(
                        model_bg,
                        quality_bg,
                        &sys_prompt_bg,
                        user_bg,
                        full_text,
                        *extra_llm_calls,
                        *used_fallback,
                    );
                    let _ = jobs.merge_job_payload(
                        &run_id_bg,
                        json!({
                            "u11": {
                                "turn": sess.turn,
                                "epoch": sess.epoch,
                                "resumed": resumed_bg,
                                "durationMs": elapsed_ms_v,
                                "llmCalls": est.llm_calls,
                                "estTokensIn": est.est_in_tokens,
                                "estTokensOut": est.est_out_tokens,
                                "estCostUsd": est.est_cost_usd,
                                "error": None::<String>,
                            }
                        }),
                    );
                    sess.turn_cost_ledger.turns += 1;
                    sess.turn_cost_ledger.llm_calls += est.llm_calls;
                    sess.turn_cost_ledger.total_duration_ms += elapsed_ms_v;
                    sess.turn_cost_ledger.est_cost_usd += est.est_cost_usd;
                    // G10 (吞噬 denova D1): 回合正常完成 → 写入诊断摘要（accepted=true）。
                    // H3 (吞噬 humanizer-zh): 去 AI 味确定性检测（纯核，无 LLM）。
                    let hz = kaleido_core::humanize::analyze(full_text);
                    // 声线漂移确定性检测（纯核，无 LLM）：正文按角色名切台词 → 指纹比对。
                    let (vd_score, vd_char, vd_reasons) = (|| {
                        let pack = state.packs.get(&sess.pack_id).ok()?;
                        let fps = kaleido_core::build_all(&pack);
                        let names: Vec<(String, String)> = pack.characters.iter()
                            .map(|c| (c.id.clone(), c.name.clone())).collect();
                        let (cid, rep) = kaleido_core::check_turn_voices(&fps, &names, full_text)?;
                        Some((rep.drift_score, cid, rep.reasons.clone()))
                    })()
                    .unwrap_or((0.0, String::new(), Vec::new()));
                    if vd_score >= 0.25 {
                        tracing::info!(score = vd_score, char = %vd_char, "st voice: 声线漂移");
                    }
                    // 骰子审计三检（吞噬 loreweaver turn_checks 伪造/矛盾口径，纯核零 LLM）。
                    // 本回合真值 = check_history 中 turn == sess.turn 的条目。
                    let (dice_forgery, dice_contradiction, dice_reasons) = (|| {
                        let claims = kaleido_core::dice_audit::extract_roll_claims(full_text);
                        if claims.is_empty() {
                            return (false, false, Vec::new());
                        }
                        let truth: Vec<(u32, f64)> = sess
                            .check_history
                            .iter()
                            .filter(|e| e.turn as u32 == sess.turn)
                            .map(|e| (e.natural, e.total))
                            .collect();
                        let a = kaleido_core::dice_audit::audit(&claims, &truth);
                        if a.forgery || a.contradiction {
                            tracing::info!(forgery = a.forgery, contradiction = a.contradiction, reasons = ?a.reasons, "st dice: 审计命中");
                        }
                        (a.forgery, a.contradiction, a.reasons)
                    })();
                    sess.last_turn_diagnostic = Some(kaleido_core::TurnDiagnostic {
                        turn: sess.turn,
                        accepted: true,
                        duration_ms: elapsed_ms_v,
                        llm_ok: true,
                        humanize_total: hz.total,
                        humanize_grade: hz.grade().into(),
                        humanize_hits: hz.hits.len(),
                        voice_drift_score: vd_score,
                        voice_drift_char: vd_char,
                        voice_drift_reasons: vd_reasons,
                        dice_forgery,
                        dice_contradiction,
                        dice_reasons: dice_reasons.clone(),
                        hallu_names: Vec::new(),
                        conflicts: Vec::new(),
                    });
                }

                // M-2/M-3 (2026-08-14 记忆补强): worldline 初始化 + L2 上限收缩 + L4 情感层提炼
                // 1) 世界线：首回合后恒建 main（原 current_worldline_id 初始 None，只有手动存档/fork 才建）
                if sess.current_worldline_id.is_none() {
                    sess.current_worldline_id = Some("main".into());
                }
                // 2) L2 事件账本上限收缩（原无上限，长会话无限膨胀）：超 200 条按重要性淘汰
                const L2_EVENT_CAP: usize = 200;
                if sess.memory_l2.events.len() > L2_EVENT_CAP {
                    let before_l2 = sess.memory_l2.events.len();
                    sess.memory_l2.events =
                        kaleido_core::retain_l2_events(&sess.memory_l2.events, L2_EVENT_CAP);
                    tracing::info!(
                        session = %session_id_bg,
                        before = before_l2,
                        after = sess.memory_l2.events.len(),
                        "L2 事件账本上限收缩"
                    );
                }
                // 3) L4 情感层增量提炼（原为空壳只读，零写端）：每 5 回合从最近事件提炼
                // affinity/secretsKnown/promises → merge 落库；契约校验失败降级不阻塞回合。
                if sess.turn % 5 == 0 && !sess.memory_l2.events.is_empty() {
                    let recent: String = sess
                        .memory_l2
                        .events
                        .iter()
                        .rev()
                        .take(3)
                        .map(|e| format!("[t{}][{}] {}", e.turn, e.kind, e.summary))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let user_input = format!("最近剧情事件：\n{recent}");
                    if let Ok(raw) = call_llm_nonstream(state, L4_REFINE_SYSTEM, &user_input).await {
                        if let Some(patch) = kaleido_core::st_memory_contract::parse_l4_patch(&raw) {
                            merge_l4_patch(&mut sess.memory_l4, &patch);
                            tracing::info!(
                                session = %session_id_bg,
                                affinity = patch.affinity.len(),
                                secrets = patch.secrets_known.len(),
                                promises = patch.promises.len(),
                                "L4 情感层提炼已落库"
                            );
                        } else {
                            // [fix §10 2026-08-16] 契约校验失败：带差异提示重试一次
                            // （flash-free 偶发复述任务；追加「严格输出 JSON」指令提高遵循率），
                            // 仍失败才降级跳过 + 增强告警（带 raw 前 200 字符供排查）。
                            let retry_input = format!(
                                "最近剧情事件：\n{recent}\n\n注意：直接输出契约 JSON（affinity/secretsKnown/promises），不要复述任务说明、不要思考过程。"
                            );
                            let mut degraded = true;
                            if let Ok(raw2) = call_llm_nonstream(state, L4_REFINE_SYSTEM, &retry_input).await {
                                if let Some(patch2) = kaleido_core::st_memory_contract::parse_l4_patch(&raw2) {
                                    merge_l4_patch(&mut sess.memory_l4, &patch2);
                                    tracing::info!(
                                        session = %session_id_bg,
                                        affinity = patch2.affinity.len(),
                                        secrets = patch2.secrets_known.len(),
                                        promises = patch2.promises.len(),
                                        "L4 提炼重试成功落库"
                                    );
                                    degraded = false;
                                }
                            }
                            if degraded {
                                tracing::warn!(
                                    session = %session_id_bg,
                                    raw = %raw.chars().take(200).collect::<String>(),
                                    "L4 提炼输出不合规（重试后仍失败），降级跳过"
                                );
                            }
                        }
                    }
                }

                // [morphling Wave B3 2026-08-16] 章节剧情摘要账本（吸收自 BakemonoMemory
                // summary-memory-model）：跨章时 + 配置阈值触发兜底提炼（默认每 10 回合或
                // 20 事件；可经 UI/API 调 diaryConfig）。manual_edited 条目不被自动覆盖；
                // 提炼失败静默降级不阻塞回合。
                if let Some(ch_id) = sess.chapter_cursor.clone() {
                    let ch_title = pack
                        .chapters
                        .iter()
                        .find(|c| c.id == ch_id)
                        .map(|c| c.title.clone())
                        .unwrap_or_else(|| ch_id.clone());
                    let crossed = sess
                        .chapter_diaries
                        .last()
                        .map(|d| d.chapter_id != ch_id)
                        .unwrap_or(false);
                    let cfg = sess.diary_config.clone().unwrap_or_default();
                    let due_by_turns = sess.turn % cfg.turn_interval.max(1) == 0;
                    let due_by_events = sess.memory_l2.events.len() as u32 >= cfg.event_threshold.max(1);
                    let idx = sess.chapter_diaries.iter().position(|d| d.chapter_id == ch_id);
                    let should_run = match idx {
                        Some(i) => {
                            let d = &sess.chapter_diaries[i];
                            !d.manual_edited && sess.turn.saturating_sub(d.updated_at_turn) >= 8
                        }
                        None => true,
                    };
                    if (crossed || due_by_turns || due_by_events) && should_run {
                        let (sys_d, user_d) = kaleido_core::build_chapter_diary_prompt(&sess, &ch_title);
                        if let Ok(raw) = call_llm_nonstream(state, &sys_d, &user_d).await {
                            let summary = kaleido_core::parse_chapter_diary_response(&raw);
                            if !summary.is_empty() {
                                let summary_chars = summary.chars().count();
                                match idx {
                                    Some(i) => {
                                        sess.chapter_diaries[i].summary = summary.clone();
                                        sess.chapter_diaries[i].end_turn = sess.turn;
                                        sess.chapter_diaries[i].updated_at_turn = sess.turn;
                                    }
                                    None => {
                                        sess.chapter_diaries.push(kaleido_core::ChapterDiaryEntry {
                                            chapter_id: ch_id.clone(),
                                            title: ch_title,
                                            summary,
                                            start_turn: sess.turn.saturating_sub(9),
                                            end_turn: sess.turn,
                                            updated_at_turn: sess.turn,
                                            manual_edited: false,
                                        });
                                    }
                                }
                                tracing::info!(
                                    session = %session_id_bg,
                                    chapter = %ch_id,
                                    chars = summary_chars,
                                    "章节摘要已提炼"
                                );
                            }
                        }
                    }
                }

}

