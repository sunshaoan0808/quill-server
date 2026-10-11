//! p004/prompt —— build_tavern_system_prompt（回合系统提示词组装，热路径 35µs 基线）。
//!
//! [R6-2② 2026-10-07] 自单文件整体迁入（1157 行单函数，纯拼接零 IO）。
//! 函数级拆分候选分节地图（行号为迁入前 p004.rs 实测，供后续轮）：
//!   434-485 剧本头+章节摘要账本 | 486-522 角色名单/引用追踪 | 523-570 出口/偏离档/节点概要
//!   571-644 配置注入+文风要求 | 645-708 去 AI 味+导演计划+情绪钩子+章节合同
//!   709-746 Memory RAG + L4 | 747-822 事件卡 | 823-889 Actor 状态机+世界线+章节摘要块+规则检定
//!   889-942 Scribe+时间天气+年轮+Needs | 936-1064 Front Porch 现实引擎块族（气候/混乱/目标/口袋/羁绊/journal）
//!   1065-1137 场景跳变/连续性守卫/检定约束/跨会话记忆/伏笔/章节摘录
//!   1138-1245 角色三层注入 | 1246-1349 世界书规制+在场+Lore timed
//!   1350-1386 Content tier 护栏 | 1387-1467 Memory L1-L4 | 1468-1533 MCP/技能回填+历史+玩家配置
//!   1534-1579 选项协议+回执+决策门禁+面板+注入护栏 footer
#![allow(unused_imports)]

use super::*;

/// Build a story context prompt from pack context + session state.
pub(crate) fn build_tavern_system_prompt(
    pack: &StoryPack,
    session: &TavernSession,
    chapter_body: &str,
    cross_dir: &std::path::Path,
    query_embedding: Option<Vec<f32>>,
    mcp_tools: &[crate::tavern_mcp::McpToolEntry],
    foreshadow_block: Option<&str>,
) -> String {
    let mut lines = Vec::new();

    // Story context
    lines.push("你是一个故事主持人（DM/GM）。你按照下面的剧本包设定进行沉浸式叙事。".into());
    lines.push(format!("## 剧本：{}", pack.title));
    lines.push(format!("当前章节：{}", {
        session
            .chapter_cursor
            .as_deref()
            .unwrap_or("(未知)")
    }));
    // [morphling Wave B3 2026-08-16] 章节剧情摘要账本注入（吸收自 BakemonoMemory
    // summary-memory-model）：当前章「本章剧情进展」+ 上一章回顾，治理长跑失忆。
    if !session.chapter_diaries.is_empty() {
        let cur = session.chapter_cursor.as_deref().unwrap_or("");
        if let Some(d) = session.chapter_diaries.iter().find(|d| d.chapter_id == cur) {
            if !d.summary.trim().is_empty() {
                lines.push(format!("\n## 本章剧情进展\n{}", d.summary));
            }
        }
        let prev = session
            .chapter_diaries
            .iter()
            .filter(|d| d.chapter_id != cur && !d.summary.trim().is_empty())
            .last();
        if let Some(d) = prev {
            lines.push(format!(
                "\n## 上一章回顾（{}）\n{}",
                if d.title.is_empty() { d.chapter_id.as_str() } else { d.title.as_str() },
                d.summary
            ));
        }
    }
    lines.push(format!("当前节点：{}", {
        session
            .node_id
            .as_deref()
            .unwrap_or("(未知)")
    }));
    lines.push(format!("当前模式：{}", session.play_mode.as_str()));
    if let Some(vid) = session
        .player
        .control_character_id
        .as_ref()
        .or(session.entry.vessel_character_id.as_ref())
    {
        let vname = pack
            .characters
            .iter()
            .find(|c| c.id == *vid)
            .map(|c| c.name.as_str())
            .unwrap_or(vid.as_str());
        lines.push(format!("玩家当前 vessel/身子：**{}**（{}）。请按此身份叙事，勿让玩家同时操控其他 NPC 身子。", vname, vid));
    }
    // ST-30 (2026-08-15 根治): 注入本作角色名单 + 回合末尾角色清单输出要求。
    // 守卫优先用清单做精确集合比对（替代「说话标记前切词」启发式——根治叙述形态
    // 漏报与切词碎短语误报）。清单缺失时守卫自动降级启发式，安全兜底。
    let roster_names: Vec<&str> = pack.characters.iter().map(|c| c.name.as_str()).collect();
    if !roster_names.is_empty() {
        lines.push(format!(
            "## 本作角色名单（严禁引入名单外人物作为登场角色）\n{}",
            roster_names.join("、")
        ));
        lines.push(
            "回合正文末尾必须输出 <角色清单>本回合实际出场的人名（顿号分隔）</角色清单>；没有新出场角色也输出 <角色清单></角色清单>。"
                .into(),
        );
        // [waidrin ①] 出场角色名用 ** 包裹（引用追踪，吸收自 waidrin narratePrompt 口径）
        lines.push(
            "正文中首次提到出场角色时，角色名用双星号包裹（如 **沈棠**、**林晚的**衣袖）；属格（**沈棠的**）亦可。"
                .into(),
        );
    }
    match session.play_mode {
        kaleido_core::PlayMode::Free => {
            lines.push("自由模式：不要强行推进主线节点，允许玩家岔开闲聊；保持人设与世界一致。".into());
        }
        kaleido_core::PlayMode::Side => {
            lines.push("支线/番外模式：主线章节游标冻结；可展开支线情节，不要宣称主线节点已推进。".into());
            if let Some(label) = session.side_branch_label.as_deref() {
                lines.push(format!("当前支线：「{}」。围绕此节点/主题展开番外，可偏离主线。", label));
            }
            if let Some(sid) = session.side_branch_node_id.as_deref() {
                lines.push(format!("支线入口节点 sideBranchNodeId={}。", sid));
            }
            if let Some(rid) = session.resume_node_id.as_deref() {
                lines.push(format!("回主线锚点 resumeNodeId={}（玩家结束支线后将回到此处）。", rid));
            }
        }
        kaleido_core::PlayMode::Mainline => {
            lines.push("主线模式：优先服务节点目标与 exits，可在合理处推进剧情。".into());
            // Add available exits for LLM-directed auto-advance
            if let Some(nid) = &session.node_id {
                if let Some(node) = pack.nodes.iter().find(|n| n.id == *nid) {
                    // ST-23: allowedDivergence — 节点偏离档位（strict 严格按章节；branch 允许适度分支）
                    match node.allowed_divergence.as_str() {
                        "strict" => {
                            lines.push("本节点为严格模式（strict）：你只能叙述当前章节原著中发生的情节，禁止编造原书未出现的场景、人物、能力、地点、超自然/玄幻/灵异设定或情节走向，禁止跳章到后续情节。玩家提出偏离原著的要求（如提前发生关键事件、瞬移到其他城市、改变角色命运）时，必须拒绝或委婉拉回当前章节，绝不可跟随；只能在当前章节框架内给予有限互动。".into());
                        }
                        _ => {
                            lines.push("本节点允许适度分支（branch）：可在当前章节框架内展开合理支线，但不得引入原书不存在的人物、地点、能力或情节走向，禁止跳章。玩家要求偏离原著时，须委婉拒绝并拉回当前章节；分支结束后必须回归本章主线。".into());
                        }
                    }
                    if !node.exit.is_empty() {
                        lines.push("可用剧情出口：".into());
                        for (i, exit) in node.exit.iter().enumerate() {
                            let next_title = pack.nodes.iter()
                                .find(|n| n.id == exit.next)
                                .map(|n| n.title.as_str())
                                .unwrap_or(&exit.next);
                            lines.push(format!("  {}: {} → {}（{}）", i + 1, exit.when, exit.next, next_title));
                        }
                        lines.push("若故事已推进到合适的出口，在回复末尾的【回执】JSON 里写 \"advance\": \"<节点ID>\"（例如 \"advance\": \"n2\"）。勿过分频繁推进，让玩家有沉浸段落空间。".into());
                    }
                    // ST-24: 注入当前节点剧情概要（node.summary）与硬节拍（locked_beats）
                    if !node.summary.trim().is_empty() {
                        lines.push(format!("本章剧情概要（必须严格遵循，不得偏离或跳章）：{}", node.summary.trim()));
                    }
                    if !node.locked_beats.is_empty() {
                        // ST-25: locked_beats 是"本章稍后将发生"的未来剧情计划，不是"已发生事实"。
                        // 禁止 LLM 把它们当作既成事实提前回述/引用（否则开场卡+节拍注入会让"未来事件"变成"昨晚已发生"）。
                        lines.push("本章后续将发生的关键情节（locked beats，尚未发生：仅在玩家实际推进到该情节时按序呈现，禁止提前回述、禁止当作已发生事件引用，回述『过去』只能依据开场卡与对话历史中真实发生的内容）：".into());
                        for (i, b) in node.locked_beats.iter().enumerate() {
                            lines.push(format!("  {}. {}", i + 1, b));
                        }
                    }
                }
            }
        }
    }
    // ST-22: 改写强度约束（rewriteIntensity 由玩家创建会话时选择，必须真正传递给 LLM）
    match session.entry.rewrite_intensity {
        kaleido_core::RewriteIntensity::Canon => {
            lines.push("改写强度：严格遵循原著。你只能叙述当前节点对应章节内发生的情节，并严格按本章剧情概要（如有）推进；禁止添加原书不存在的人物、能力、超自然/玄幻/灵异设定、地点或情节走向，禁止跳章到后续情节。玩家提出偏离原著的要求（如提前登船、瞬移、改变关键事件）时，必须拒绝或委婉拉回当前章节，绝不可跟随。".into());
        }
        kaleido_core::RewriteIntensity::Rewrite => {
            lines.push("改写强度：允许大改。可自由调整剧情走向与世界观设定，但需保持角色基本人设与故事风格连贯。".into());
        }
    }
    // P0-2: config_write —— 助手配置注入（strict_mode_boost / pacing / style_guidance）
    if let Some(cfg) = session.player.flags.as_object() {
        if let Some(v) = cfg.get("strict_mode_boost").and_then(|v| v.as_f64()) {
            if v < 0.34 {
                lines.push("助手配置 strict_mode_boost：请尽量遵循 locked_beats 与章节大纲，避免偏离主线。".into());
            } else if v < 0.67 {
                lines.push("助手配置 strict_mode_boost：必须严格遵守 locked_beats 与章节大纲，禁止编造、跳章或偏离主线。".into());
            } else {
                lines.push("助手配置 strict_mode_boost（最高）：严禁任何偏离 locked_beats / 章节大纲的内容；玩家要求偏离主线时坚决拒绝并拉回当前章节。".into());
            }
        }
        if let Some(v) = cfg.get("pacing").and_then(|v| v.as_f64()) {
            if v < 0.34 {
                lines.push("助手配置 pacing：节奏放缓——多描写环境细节、氛围与角色反应，给玩家沉浸空间。".into());
            } else if v < 0.67 {
                lines.push("助手配置 pacing：保持当前叙事节奏，平稳推进。".into());
            } else {
                lines.push("助手配置 pacing：节奏加快——减少铺垫，迅速推进剧情与事件。".into());
            }
        }
        if let Some(sg) = cfg.get("style_guidance").and_then(|v| v.as_str()) {
            if !sg.trim().is_empty() {
                // P2-1: user-provided style guidance is wrapped in a
                // user-config fence so the model cannot be re-prompted by it.
                lines.push(kaleido_core::prompt_safety::wrap_user_block(
                    "user-config",
                    "config.style_guidance",
                    sg.trim(),
                ));
                // [P7R3 2026-08-16] 风格提示词平衡：限知视角"身体反应推断"常把角色情绪
                // 全外化为气音/颤抖/半截话，压制原声线台词——追加保留台词声线的平衡指令。
                lines.push("风格要求注意：上述风格适用于叙述与氛围；**角色台词必须保留其原声线**（参考角色卡示例对白），身体反应描写之外须有符合角色性格的直接对白，不得因限知视角把所有情绪都压成气音、颤抖或咽回半截话。".into());
            }
        }
        // [fix 2026-08-15 叙述视角] /config pov=first|third —— 用户指定人称，覆盖蒸馏文风。
        if let Some(pv) = cfg.get("pov").and_then(|v| v.as_str()) {
            match pv {
                "first" => lines.push("叙述人称（用户指定）：本回合正文一律使用第一人称「我/我们」叙述视角；禁止第三人称旁白（他/她）。".into()),
                "third" => lines.push("叙述人称（用户指定）：本回合正文一律使用第三人称「他/她」叙述视角；禁止第一人称「我」作叙述主体。".into()),
                _ => {}
            }
        }
    }
    // [fix 2026-08-15 文风注入] narrative_style 此前只蒸馏不注入（空转字段）。
    // 本 pack 蒸馏出的 narrative_style → 注入「## 文风要求」。
    // 跨 pack 外挂（宿醉外挂智取 18+ 文风）由调用方 start_turn 解析外部 pack 后
    // 经 style_guidance 通道注入（见下方 P0-2 config_write 段），本函数保持纯函数。
    let ns_block = pack
        .stage_director
        .resolved_snapshot
        .as_ref()
        .and_then(|snap| snap.narrative_style.as_ref());
    if let Some(ns) = ns_block {
        let mut style_lines: Vec<String> = Vec::new();
        if let Some(s) = ns.get("style").and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                style_lines.push(format!("叙事视角/人称：{}", s.trim()));
            }
        }
        if let Some(t) = ns.get("tone").and_then(|v| v.as_str()) {
            if !t.trim().is_empty() {
                style_lines.push(format!("基调：{}", t.trim()));
            }
        }
        if let Some(p) = ns.get("prose_guidance").and_then(|v| v.as_str()) {
            if !p.trim().is_empty() {
                style_lines.push(format!("行文要求：{}", p.trim()));
            }
        }
        if !style_lines.is_empty() {
            lines.push("\n## 文风要求".into());
            lines.extend(style_lines);
        }
    }
    // H4 (吞噬 humanizer-zh): 去 AI 味硬约束 —— 上回合 humanize 诊断驱动，
    // 有命中则点名，无则通用约束。纯文本注入，零 LLM。
    {
        let mut hz_lines: Vec<String> = vec!["\n## 去 AI 味（硬约束）".into()];
        if let Some(dg) = session.last_turn_diagnostic.as_ref().filter(|d| d.humanize_hits > 0) {
            hz_lines.push(format!(
                "上回合去 AI 味 {}／50（{}）：重点改掉以下毛病，本回合正文不许再犯：",
                dg.humanize_total, dg.humanize_grade
            ));
            // 通用三条（turn 20/21 实测最高频）
            hz_lines.push("1. 少用破折号“——”：一回合至多 1 处，优先用逗号/句号断句；".into());
            hz_lines.push("2. 不用三段式列举（A、B和C）：两项或拆句；".into());
            hz_lines.push("3. 不用“此外/彰显/格局/作为……的证明”类 AI 词；".into());
            hz_lines.push("4. 不用“不仅仅是……而是……”排比；".into());
        } else {
            hz_lines.push("少用破折号“——”（一回合至多 1 处）；不用三段式列举；不用“此外/彰显/格局”类 AI 词；不用“不仅仅是……而是……”排比。".into());
        }
        lines.extend(hz_lines);
    }
    // S4 (吞噬 denova director_plan): 导演计划注入 + 红线下沉 —— 导演计划是意图，不是自由改写
    if let Some(plan) = &session.director_plan {
        lines.push("\n## 导演计划（当前叙事意图）".into());
        lines.push(format!("导演目标：{}", plan.goal));
        if let Some(p) = &plan.pressure {
            if !p.trim().is_empty() {
                lines.push(format!("当前压力：{}", p));
            }
        }
        if let Some(c) = &plan.cost {
            if !c.trim().is_empty() {
                lines.push(format!("达成代价：{}", c));
            }
        }
        if !plan.hits_beats.is_empty() {
            lines.push("命中/借用的硬节拍（hits_beats）：".into());
            for (i, b) in plan.hits_beats.iter().enumerate() {
                lines.push(format!("  {}. {}", i + 1, b));
            }
        }
        lines.push("红线约束：导演计划仅为叙事意图，绝不改写 locked_beats 已锁定的硬事实；hits_beats 列出计划命中/借用的硬节拍。".into());
    }
    // X2b (吞噬自 xiami emotional_hooks.rs): 情绪钩子执行合同注入 —— 导演计划块之后追加。
    // 开关：默认启用，但轻量会话（无 guard_events 且无 director_plan 的纯聊天）跳过，避免干扰。
    if !session.guard_events.is_empty() || session.director_plan.is_some() {
        let hook_config = EmotionalHookConfig::default();
        let hook_contract = render_hook_execution_contract(&hook_config);
        if !hook_contract.trim().is_empty() {
            lines.push("\n".to_string());
            lines.push(hook_contract);
        }
        // 重复结尾信号警告：用最近 N 条 assistant 正文的结尾片段构造样本（简单截取即可）。
        if hook_config.detect_repetition {
            let recent_samples = build_hook_recent_samples(session);
            if let Some(signal) = repeated_recent_hook_signal(&recent_samples) {
                lines.push(format!("最近章节反复使用“{signal}”类牵引，后续避免照搬。"));
            }
        }
    }
    // X4 (吞噬自 xiami outline.rs): 章节执行合同注入 —— 导演计划块与情绪钩子合同之后追加。
    // **不阻断**：无 director_plan 或无法构造数据时跳过，不改变回合语义。
    if let Some(contract_text) = build_director_execution_contract(session) {
        lines.push("\n".to_string());
        lines.push(contract_text);
    }
    // Memory context (ST-21: embedding RAG — query_embedding is pre-computed in start_turn)
    // [审计修复 核心叙事引擎-2] token 级兜底与 embedding 同源：最后一条用户消息 + 节点 ID
    let mem_query_text = session
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| format!("{} {}", m.content, session.node_id.as_deref().unwrap_or("")));
    let mem_ctx = kaleido_core::build_memory_context(
        session,
        6,
        4,
        4,
        query_embedding.as_deref(),
        mem_query_text.as_deref(),
    );
    if !mem_ctx.is_empty() {
        lines.push("\n".to_string());
        lines.push(mem_ctx);
    }

    // L4 Affinity / Secrets / Promises (ST-18)
    let l4 = &session.memory_l4;
    // §14.7.2C (2026-08-18): 接吻里程碑并入本块（原独立「关系状态」块删除）——
    // ST-FIX: 弱信号 contains("亲")/("床") 会把「母亲」的「亲」字误计为接吻，改用强信号。
    let kiss_count = session.memory_l2.events.iter().filter(|e| {
        e.kind == "romance" ||
        e.summary.to_lowercase().contains("接吻") ||
        e.summary.to_lowercase().contains("亲嘴") ||
        e.summary.to_lowercase().contains("亲吻") ||
        e.summary.to_lowercase().contains("亲热") ||
        e.summary.to_lowercase().contains("吻")
    }).count();
    let has_affinity = l4.affinity.is_object() && l4.affinity.as_object().map_or(false, |m| !m.is_empty());
    let has_secrets = !l4.secrets_known.is_empty();
    let has_promises = !l4.promises.is_empty();
    if has_affinity || has_secrets || has_promises || kiss_count > 0 {
        lines.push("\n## 情感与关系".into());
        // Build charId → name lookup
        let char_name = |cid: &str| -> String {
            pack.characters.iter().find(|c| c.id == cid).map(|c| c.name.as_str()).unwrap_or(cid).to_string()
        };
        if has_affinity {
            lines.push("好感度（0~100）：".into());
            if let Some(aff_map) = l4.affinity.as_object() {
                for (key, val) in aff_map.iter() {
                    let val_num = val.as_i64().unwrap_or(0);
                    // key can be "charId" or "charId:targetId"
                    let parts: Vec<&str> = key.split(':').collect();
                    let from_name = char_name(parts[0]);
                    if parts.len() > 1 && !parts[1].is_empty() {
                        let to_name = char_name(parts[1]);
                        lines.push(format!("  • {} → {}: {}", from_name, to_name, val_num));
                    } else {
                        lines.push(format!("  • {}: {}", from_name, val_num));
                    }
                }
            }
        }
        if has_secrets {
            lines.push("已知秘密：".into());
            for s in &l4.secrets_known {
                lines.push(format!("  • {}", s));
            }
        }
        if has_promises {
            lines.push("承诺/誓言：".into());
            for p in &l4.promises {
                lines.push(format!("  • {}", p));
            }
        }
        // §14.7.2C: 接吻里程碑附注（原独立块并入，省 ~8 行 + 情感状态归一）
        if kiss_count > 0 {
            lines.push(format!(
                "亲密里程碑：你们已接过吻（至少 {kiss_count} 次），关系已确立；后续亲热描写要体现熟悉感，不要再当第一次。"
            ));
        }
    }

    // S5 (吞噬 denova event_package): 本回合事件卡注入（ST-27 演出提示）
    if let Some(ev) = &session.last_event {
        lines.push("\n## 本回合事件卡".into());
        // G7: 事件名带 typeName/category/intensity 展示，如「事件：外门考核打脸（打脸 · medium）」；
        // 旧数据无新字段则退化为纯标题。
        let mut ev_label = if ev.type_name.trim().is_empty() {
            ev.title.clone()
        } else {
            ev.type_name.clone()
        };
        let mut ev_meta: Vec<&str> = Vec::new();
        if !ev.category.trim().is_empty() {
            ev_meta.push(ev.category.trim());
        }
        if !ev.intensity.trim().is_empty() {
            ev_meta.push(ev.intensity.trim());
        }
        if !ev_meta.is_empty() {
            ev_label = format!("{}（{}）", ev_label, ev_meta.join(" · "));
        }
        lines.push(format!("事件：{}", ev_label));
        if !ev.prompt.trim().is_empty() {
            // P2-1: event.prompt is user-authored演出提示 — wrap as event-block.
            lines.push(kaleido_core::prompt_safety::wrap_user_block(
                "event-block",
                &format!("event/{}/{}", ev.turn, ev.title),
                ev.prompt.trim(),
            ));
        }
        lines.push("本回合叙事必须自然融入该事件（可作背景氛围、人物动向或玩家遭遇的引子）；若回合内未正面呈现，可在后续回合继续铺垫。不要直接复读本指令文本。".into());
        // [P1C 2026-08-16] 事件卡为背景/氛围素材（描述过往或潜在场景），不代表角色当前状态；
        // 防 LLM 依据旧事件卡推断角色当前穿着/身体状态（宿醉「脱了又穿上」根因环之一）。
        lines.push("注意：事件卡是背景/氛围素材，描述的是过往或潜在场景，**不代表角色当前状态**；不得据此推断角色当前穿着/身体状态（以【角色状态】与最近剧情为准）。".into());
    }

    // ST-26: Actor 状态机（吞噬 denova actor_state）——渲染当前角色状态
    let actor_ctx = session.actor_states.build_context_text();
    if !actor_ctx.is_empty() {
        lines.push("\n## 角色状态（当前数值，随剧情按指令更新）".into());
        lines.push(actor_ctx);
        lines.push("若剧情导致角色数值/状态/traits 变化，请在回复末尾的【回执】JSON 的 stateUpdates 数组中逐角色给出更新，每项格式：".into());
        lines.push("{\"characterId\":\"<charId>\",\"fields\":{\"<字段名>\":<新值>},\"addTraits\":[{\"poolId\":\"<池id>\",\"traitId\":\"<trait id>\",\"name\":\"<名字>\",\"summary\":\"<简述>\"}],\"removeTraits\":[\"<trait id>\"]}".into());
        lines.push("只更新确实变化的字段；字段约束见上方数值范围。不要在叙事正文里留这个 JSON。".into());
        lines.push("角色当前情绪请写入 fields 的 emotion 字段（取值参考：平静/开心/愤怒/悲伤/害羞/惊讶/恐惧/厌恶/疲惫/心动；若无强烈情绪可不写）。".into());
    }

    // T 层 (2026-08-19): 原著静态时间线注入——pack.worldline（旁挂 worldline.json 蒸馏产物）
    // 按当前章过滤（chapter <= 当前章），只注入已推进的历史事件，防后期剧透/首回合劫持（D2 同源）。
    // 会话动态事件流已由「近期事件 (L2)」块覆盖 → 双源时间线：静态蒸馏 + 会话增量。
    if let Some(wl_block) = build_worldline_block(&pack, session.chapter_cursor.as_deref().unwrap_or("")) {
        lines.push(wl_block);
    }

    // [morphling C2 2026-08-16] 章节剧情摘要（顺带总结模式）：正文生成时顺手输出
    // 【章节摘要】块，服务端剥离落账本——零额外 LLM 调用（吸收自 BakemonoMemory
    // summary-memory-model 的「生成时总结」思路）。
    if session.chapter_cursor.is_some() {
        lines.push("\n本章剧情总结：请在回复末尾的【回执】JSON 的 chapterDiary 字段输出本章剧情进展（200-400 字）。".into());
        lines.push("内容覆盖本章已发生的关键事件、转折与角色状态变化；正文里不要重复摘要内容。".into());
        lines.push("若本章已有剧情积累，摘要需与之前保持连贯，并补充本次新增进展。".into());
    }

    // ST-27: 规则检定（吞噬 denova TurnCheckRequest）——仅当 pack 配了 rule_system 才注入
    let rule_desc = pack
        .stage_director
        .resolved_snapshot
        .as_ref()
        .and_then(|snap| snap.rule_system.as_ref())
        .and_then(kaleido_core::RuleSystem::from_value)
        .map(|rs| {
            rs.checks
                .iter()
                .map(|c| {
                    let ex = c
                        .must_check_examples
                        .first()
                        .map(|s| format!("（例：{}）", s))
                        .unwrap_or_default();
                    format!(
                        "{}：骰{}，触发：{}{}",
                        c.label,
                        if c.dice.trim().is_empty() {
                            String::from("1d20")
                        } else {
                            c.dice.trim().to_string()
                        },
                        c.trigger,
                        ex
                    )
                })
                .collect::<Vec<_>>()
                .join("；")
        })
        .unwrap_or_default();
    if !rule_desc.is_empty() {
        lines.push("\n## 规则检定（TRPG d20）".into());
        lines.push(format!("可用检定配置：{}", rule_desc));
        lines.push("当玩家行动命中 must_check 场景（如潜行/偷听）且结果不确定时，在回复末尾的【回执】JSON 的 check 字段给出检定请求（单个对象），格式：".into());
        lines.push("{\"action\":\"<做了什么>\",\"intent\":\"<想达成什么>\",\"challenge\":\"<风险/阻碍>\",\"cost\":\"<失败代价>\",\"difficulty\":\"normal\",\"templateId\":\"<命中的配置id，可选>\",\"bonuses\":[{\"reason\":\"<原因>\",\"value\":1}],\"outcomes\":{\"criticalSuccess\":{\"result\":\"<大成功结果>\"},\"success\":{\"result\":\"<成功结果>\",\"stateChanges\":[{\"actorId\":\"<角色id>\",\"fieldId\":\"<字段>\",\"change\":1,\"reason\":\"<理由>\"}]},\"failure\":{\"result\":\"<失败结果>\"},\"criticalFailure\":{\"result\":\"<大失败结果>\"}}}".into());
        lines.push("只对结果不确定且值得掷骰的行动检定；纯对话/回忆不需要。不要在叙事正文里留这个 JSON。".into());
    }
    // Scribe 低语消费侧（吞噬 loreweaver scribe 口径）：上回合后台提取的可检定行动，
    // 以建议形态注入（KP 自由采纳，非强制；本回合用完即清）。
    if !session.scribe_whispers.is_empty() {
        lines.push("
## Scribe 低语（上回合观察到的可检定行动，建议非强制）".into());
        for w in &session.scribe_whispers {
            lines.push(format!("- 建议考虑检定：{}", w));
        }
    }
    // [时间天气系统 v2 2026-08-17 + 1A 2026-08-18] 权威时钟+天气约束：模型必须以此为准书写正文场景。
    // v2：时间剧情信号驱动、天气用户指令第一原则。
    // 1A：改写旧强约束话术（原「必须与上一条完全一致；禁止跳变」与 v2 豁免条款自相矛盾，
    //   模型在冲突时服从硬禁令 → 宿醉夏末雨夜被压成清晨晴春）。改为信号驱动一致语义：
    //   无剧情/玩家信号时延续当前状态；玩家输入或剧情所需（含作品/楔子/开场设定）以玩家与剧情为准，不算跳变。
    lines.push("\n## 当前时间与天气（权威状态，正文需遵守）".into());
    // [P2-C 吞噬 Front Porch AI journal_physics.rs] Growth Rings（per-character 成长年轮）。
    if !session.growth.rings.is_empty() {
        let mut has_growth = false;
        for cid in session.pockets.keys() {
            let block = session.growth.injection_block(cid);
            if !block.is_empty() { if !has_growth { lines.push("\n## 角色成长年轮（权威状态）".into()); has_growth = true; } lines.push(block); }
        }
        // 也覆盖没有口袋但有年轮的角色
        for cid in session.growth.rings.iter().map(|r| r.character.clone()).collect::<std::collections::HashSet<_>>() {
            if session.pockets.contains_key(&cid) { continue; }
            let block = session.growth.injection_block(&cid);
            if !block.is_empty() { if !has_growth { lines.push("\n## 角色成长年轮（权威状态）".into()); has_growth = true; } lines.push(block); }
        }
        let _ = has_growth;
    }
    // [P2-A 吞噬 Front Porch AI needs_simulation.rs] Needs 六维（饥饿驱动口袋）。
    if !session.needs.is_empty() {
        let mut has_needs = false;
        for (cid, needs) in &session.needs {
            let name = pack.characters.iter().find(|c| c.id == *cid).map(|c| c.name.as_str()).unwrap_or(cid.as_str());
            let ctx = needs.needs_context(name);
            if !ctx.is_empty() { if !has_needs { lines.push("\n## 角色状态（Needs 六维，0-100）".into()); has_needs = true; } lines.push(ctx.trim_end().to_string()); if needs.pending_catastrophe.is_some() { lines.push(format!("⚠️ Catastrophe pending for {name}: {}", needs.pending_catastrophe.as_deref().unwrap_or(""))); } }
            // 口袋联动：饥饿时提示优先翻口袋食物
            if needs.is_urgent("hunger") {
                if let Some(p) = session.pockets.get(cid) {
                    let has_food = p.carrying.iter().any(|it| it.name.to_ascii_lowercase().contains("food") || it.name.contains("食物") || it.name.contains("面包") || it.name.contains("水"));
                    if has_food { lines.push(format!("提示：{name} 已饥饿（hunger urgent），口袋有食物，可先取用。")); }
                }
            }
        }
        let _ = has_needs;
    }
    // [P3-A 吞噬 Front Porch AI world.dart] World Climate（atmosphere/gravity/temp_band）。
    if session.world_climate.atmosphere != kaleido_core::world_climate::WorldAtmosphere::Breathable || session.world_climate.gravity != kaleido_core::world_climate::WorldGravity::Earth || session.world_climate.temp_band.is_some() {
        lines.push(format!("\n## 世界气候（权威状态）\natmosphere: {} | gravity: {} | temp_band: {}",
            session.world_climate.atmosphere.as_str(), session.world_climate.gravity.as_str(), session.world_climate.temp_band.as_deref().unwrap_or("auto")));
        lines.push("规则： hostile 需 suit 才能外出；需据此守卫 dress_for_weather。".into());
    }
    // [P4 吞噬 Front Porch AI chaos/tiers/objectives/dreams] Chaos + Tiers + 目标 + 夜梦碎屑。
    if let Some(inj) = session.chaos.prompt_injection() { lines.push("\n## 命运事件（Chaos/Chance Time）".into()); lines.push(inj); lines.push("规则：此事件已触发，必须在正文中自然体现，不得忽略。".into()); }
    if !session.milestones.is_empty() {
        lines.push("\n## 关系里程碑".into());
        for m in &session.milestones { lines.push(format!("- {} · {} · {} (turn {})", m.character, m.label, m.kind, m.turn)); }
    }
    if !session.objectives.is_empty() {
        let active: Vec<_> = session.objectives.iter().filter(|o| o.status=="active").collect();
        if !active.is_empty() {
            lines.push("\n## 当前目标".into());
            for o in active { let stage = kaleido_core::objectives::objective_stage_word(o); lines.push(format!("- [{}] {} ({})", o.owner, o.title, stage)); for t in &o.tasks { lines.push(format!("  - [{}] {}", if t.completed {"x"} else {" "}, t.title)); } }
        }
    }
    if !session.ambitions.is_empty() {
        let cur: Vec<_> = session.ambitions.iter().filter(|a| !a.completed).collect();
        if !cur.is_empty() {
            lines.push("\n## 长远野望".into());
            for a in cur {
                // ambition progress = linked objectives avg (fallback 0)
                let linked: Vec<f64> = session.objectives.iter().filter(|o| o.owner==a.character && o.status!="abandoned").map(|o| o.progress()*100.0).collect();
                let pct = if linked.is_empty() { 0.0 } else { linked.iter().sum::<f64>() / linked.len() as f64 };
                let stage = kaleido_core::objectives::ambition_stage_word(pct);
                lines.push(format!("- {}：{} ({})", a.character, a.text, stage));
            }
        }
    }
    if !session.episodes.crumbs.is_empty() {
        lines.push("\n## 日常碎屑（近况）".into());
        for c in session.episodes.recent_for_prompt(3) { lines.push(format!("- [{}] {}", c.kind, c.content)); }
    }
    if let Some(d) = &session.dream.last_dream { if !d.is_empty() { lines.push("\n## 昨夜之梦".into()); lines.push(d.clone()); } }
    // [承诺债务 吞噬 Front Porch AI promise_debt] 未竟承诺追踪。
    {
        let pb = session.promises.injection_block();
        if !pb.is_empty() { lines.push(pb); }
    }
    // [心情基线 吞噬 Front Porch AI mood_baseline] needs+时间+天气 → 开场 tint（只着色不驱动）。
    {
        // 首角色代表：needs 转 i32 map，time_of_day 从 game_clock 时段，天气从 game_clock.weather
        let cid0 = session.present_character_ids.first().cloned().unwrap_or_default();
        if !cid0.is_empty() {
            if let Some(nd) = session.needs.get(&cid0) {
                let m: std::collections::HashMap<String,i32> = nd.vector.clone();
                let tod = session.game_clock.time_of_day.clone();
                let w = session.game_clock.weather.clone();
                let miserable = ["暴雨","大雨","暴雪","大雪","雾","大雾"].iter().any(|x| w.contains(x));
                let beautiful = w=="晴";
                let mb = kaleido_core::mood_presence::derive_mood(&m, &tod, miserable, beautiful);
                let inj = mb.injection();
                if !inj.is_empty() { lines.push(inj); }
            }
        }
    }
    // [在场推导 吞噬 Front Porch AI presence_derive] occupation/hours → At work/Away/With you。
    if !session.presence.is_empty() {
        let mut has_p = false;
        for (cid, pr) in &session.presence {
            if pr.occupation.is_empty() && pr.hours.is_empty() { continue; }
            let name = pack.characters.iter().find(|c| c.id==*cid).map(|c| c.name.as_str()).unwrap_or(cid.as_str());
            // clock: game_clock day/slot → minutes approx (slot index*180), weekday from day%7+1
            let slot_idx = ["凌晨","清晨","上午","中午","下午","傍晚","夜晚","深夜"].iter().position(|s| session.game_clock.time_of_day.contains(s)).unwrap_or(2) as i32;
            let clock_min = (slot_idx * 180 + 60) % 1440;
            let weekday = (session.game_clock.day % 7 + 1) as i32;
            let stance = session.relationships.get(cid).map(|b| b.spatial_stance.as_str()).unwrap_or("");
            let with_user = session.relationships.get(cid).and_then(|b| b.with_user);
            let in_scene = session.present_character_ids.contains(cid);
            let w = kaleido_core::mood_presence::derive_presence(&pr.occupation, &pr.hours, clock_min, in_scene, weekday, pr.work_days.as_deref(), stance, with_user);
            if w != kaleido_core::mood_presence::PresenceWhere::WithYou {
                if !has_p { lines.push("\n## 在场状态".into()); has_p=true; }
                lines.push(format!("- {name}: {} ({})", kaleido_core::mood_presence::presence_label(w), if w==kaleido_core::mood_presence::PresenceWhere::AtWork { format!("{} {}", pr.occupation, pr.hours) } else { String::new() }));
            }
        }
        let _ = has_p;
    }
    // [场景渐隐 吞噬 Front Porch AI scenario_fade] scenario 随 user 消息数 10→0。
    {
        let user_n = session.messages.iter().filter(|m| m.role=="user").count();
        let strength = kaleido_core::promise::scenario_strength(user_n);
        // scenario 源：首节点 summary（开场设定），随会话拉长渐隐，避免喧宾夺主
        let scenario_text = pack.nodes.first().map(|n| n.summary.clone()).unwrap_or_default();
        if !scenario_text.is_empty() {
            let wrapped = kaleido_core::promise::wrap_scenario(&scenario_text, strength);
            if !wrapped.is_empty() { lines.push("\n".to_string() + &wrapped); }
        }
    }
    // [羁绊活数值 吞噬 Front Porch AI relationship_service] Bond/Trust/姿态/执念。
    if !session.relationships.is_empty() {
        lines.push("\n".to_string() + &kaleido_core::relationship::relationships_context(&session.relationships, &pack.characters.iter().map(|c| c.id.clone()).collect::<Vec<_>>()));
    }
    // [Journal 存量 吞噬 Front Porch AI journal_store] 热卡常驻（按热度+情绪加权，预算600字）。
    if !session.journal.cards.is_empty() {
        // 取会话首个角色的 journal 作代表（多角色时按 present_character_ids 依次）
        let cids: Vec<String> = if session.present_character_ids.is_empty() { session.journal.cards.iter().map(|c| c.character_id.clone()).collect::<std::collections::HashSet<_>>().into_iter().collect() } else { session.present_character_ids.clone() };
        for cid in cids {
            let name = pack.characters.iter().find(|c| c.id==cid).map(|c| c.name.as_str()).unwrap_or(cid.as_str());
            let cur_emotion = session.actor_states.actors.get(&cid).and_then(|a| a.fields.get("emotion")).and_then(|f| f.value.as_ref().and_then(|v| v.as_str()).map(|s| s.to_string())).unwrap_or_default();
            let block = session.journal.injection_block(&session.session_id, &cid, name, &cur_emotion, 600);
            if !block.is_empty() { lines.push(block); }
        }
    }
    // [吞噬 Front Porch AI pockets.dart] 口袋与衣物（per-character, per-session，GameClock day 用于 setAside 晨间过期）。
    // [P1-B Porch Life À la carte] Own switch. Does not need the Realism Engine.
    // 关时提示词不注入，但数据仍保留（导演台仍可见，可再打开）。
    if session.pockets_enabled && !session.pockets.is_empty() {
        let day = session.game_clock.day;
        let mut has_any = false;
        for (cid, pockets) in &session.pockets {
            let block = pockets.wardrobe_context(
                pack.characters.iter().find(|c| c.id == *cid).map(|c| c.name.as_str()).unwrap_or(cid.as_str()),
                day,
            );
            if !block.is_empty() {
                if !has_any { lines.push("\n## 角色随身物品（口袋与衣物，权威状态）".into()); has_any = true; }
                lines.push(block.trim_end().to_string());
            }
        }
        if has_any {
            lines.push("规则：角色当前穿着/携带以此为准，正文不得凭空增删；衣物脱下后入「暂存堆」(setAside)，次日清晨衣物过期、随身物不过期。".into());
        }
    }
    lines.push(format!("当前游戏时间：{}。", session.game_clock.state_line()));
    lines.push("规则：正文时间/天气默认延续当前权威状态；若玩家输入明确指定时间天气、或剧情/作品设定所需（如楔子、开场按原著氛围书写夏末雨夜），以玩家与剧情为准，不视为跳变。禁止无依据地随意改写。".into());
    lines.push("时间默认保持当前状态，不会自动流逝——只有剧情真正需要时间推进（过夜、赶路、等待、过了几天）时，才在回复末尾的【回执】JSON 里写 \"timeAdvance\": \"<目标时段或天数>\"，由系统统一推进；不得因普通对话或闲聊推进时间。".into());
    // [ST-34 2026-08-16] 场景跳变修复：模型把「凌晨」脑补成「清晨+睡了一夜」。
    // 凌晨/深夜同属当夜——玩家若在延续当前场景（喝酒/夜谈/同处一室），时段保持当夜不变，
    // 严禁脑补角色已经睡了一夜/次日清晨；只有正文末尾显式 timeAdvance 才允许跨夜。
    lines.push("「凌晨/深夜」属于同一夜的延续：玩家正在进行的场景（喝酒、夜谈、同处一室）默认持续到天亮，禁止擅自让角色睡觉、醒来、跳到次日清晨；若剧情真要过夜，必须在回复末尾的【回执】JSON 里写 \"timeAdvance\": \"次日清晨\" 等标注。".into());
    lines.push("玩家输入本身就能推进时间（如「睡一觉到天亮」「三天后见」）——系统会识别并同步；正文只需如实书写玩家要求的时间状态，无需额外标注。".into());
    lines.push("天气变化只能逐级渐进（晴→多云→阴→小雨→大雨→暴雨 或 雪/雾/大风），禁止无过渡跳变；但玩家显式指定某天气（如「突然下暴雨」作为玩家指令）时以玩家为准。".into());
    // [ST-35 2026-08-16] 剧情连续性守卫：已确立的约定/计划/同行安排必须延续，
    // 禁止在同一场景内擅自推翻。修复「窝边草」根因：turn13 庄眉说「跟我一块儿去店里」
    // （两人同行已确立），turn14 模型却改写成庄眉独自去西郊、选项全按男主留守生成，
    // 与上一轮正文直接矛盾。玩家选「什么也不问」≠ 取消约定，约定依然有效。
    lines.push("\n## 剧情连续性守卫（硬约束，必须遵守）".into());
    lines.push("上一条正文中已确立的剧情事实必须延续：角色之间的约定、同行安排、在场状态、对话结论、人物关系，一律不得在本回合无提示地推翻或改写。".into());
    lines.push("玩家输入没有明确取消的约定一律视为仍有效：若上一轮正文说过「一起去/一块儿去/说好/约好」等同行或计划，本回合必须按约定延续，不得擅自改成其中一人独自前往、独自行动或单方面变卦。".into());
    lines.push("只有以下情况允许改变已确立的剧情事实：①玩家输入显式取消/更改；②回执 timeAdvance 显式标注的时间跳跃；③新增的外部事件冲击（需在正文中明确交代原因，且不能推翻上一轮刚说定的安排）。".into());
    lines.push("本回合选项必须与本回合正文一致：选项反映的是本回合正文真实发生的情境，不得与正文矛盾（正文里两人同行，选项就不许是「目送她独自出门」）。".into());

    // [P12 2026-08-15] 上回合检定结果作为剧情约束注入：检定失败（含 critical_failure）
    // 必须在正文中真实体现——NPC 的拒绝/离开/失控是已发生的剧情事实，不得反转成默许。
    // 修复「提线木偶」根因之一（检定结果仅事后 append、零约束力）。
    if !session.last_check_results.is_empty() {
        lines.push("\n## 上回合检定结果（剧情约束，必须如实体现）".into());
        for r in &session.last_check_results {
            lines.push(format!("- {}", r));
        }
        lines.push("若其中包含失败（failure/critical_failure）结果，本回合正文必须按失败后果书写：".into());
        lines.push("被试探方真实拒绝/离开/发怒/尖叫等，禁止把失败反转写成默许或欲拒还迎；".into());
        lines.push("若全部成功，则按成功后果自然衔接。".into());
    }

    // ST-19: Cross-session memory context
    let cross_character_ids: Vec<&str> = session.present_character_ids.iter().map(|s| s.as_str()).collect();
    let cross_ctx = kaleido_core::build_cross_session_context(
        cross_dir,
        &session.pack_id,
        &cross_character_ids,
        5,
    );
    if !cross_ctx.is_empty() {
        lines.push("\n".to_string());
        lines.push(cross_ctx);
    }

    // P0 闭环: 伏笔注入（调用点预载的 block，纯函数只拼接）。
    // 作者预埋的伏笔是叙事资产: LLM 应知道但不能提前揭晓，直到玩家
    // 触发回收（recall_foreshadow MCP 工具）后该条从 block 消失。
    if let Some(fb) = foreshadow_block {
        if !fb.is_empty() {
            lines.push("\n".to_string());
            lines.push(fb.to_string());
        }
    }

    // Chapter summary
    if !chapter_body.trim().is_empty() {
        lines.push("\n## 章节正文（摘录）".into());
        // ST-25: 摘录扩大到6000字，覆盖单章全文（原1500字只含开头，丢失本章关键情节）
        let excerpt = kaleido_core::progressive_compress::compress_text(chapter_body, 6000);
        if chapter_body.chars().count() > 6000 {
            lines.push("（本章较长，已按关键词压缩抽取关键情节；未摘录部分不得编造，须以本书后续情节为准）".into());
        }
        // P2-1: wrap the chapter excerpt in a fenced block so any instruction-
        // like text inside the body is treated as data, not authority.
        let chapter_label = session
            .chapter_cursor
            .as_deref()
            .unwrap_or("(unknown)");
        lines.push(kaleido_core::prompt_safety::wrap_user_block(
            "chapter-block",
            &format!("pack/{}/chapter/{}", pack.id, chapter_label),
            &excerpt,
        ));
    }

    // Characters
    if !pack.characters.is_empty() {
        lines.push("\n## 角色".into());
        // §14.6 (2026-08-18): 角色注入三层化——①在场全量 ②玩家瘦身 ③非在场一行概要。
        // 此前每回合全量注入所有角色卡（8 字段×全部角色，度蜜月约 2461 token），稀释到场注意力、
        // 让非在场角色示例对白污染当前声线、玩家卡诱导模型替玩家表演。改造后聚焦在场。
        let present_ids: std::collections::HashSet<&str> =
            session.present_character_ids.iter().map(|s| s.as_str()).collect();
        let player_id = session
            .player
            .control_character_id
            .as_deref()
            .or(session.entry.vessel_character_id.as_deref());
        for c in &pack.characters {
            let is_present = present_ids.contains(c.id.as_str());
            let is_player = player_id == Some(c.id.as_str());
            let tier_note = match c.content_tier {
                Some(ContentTier::Safe) => " [全年龄]",
                Some(ContentTier::Standard) => " [标准]",
                Some(ContentTier::Open) => " [开放]",
                None => "",
            };
            // P2-1: each character is a user-controlled record; wrap its body
            // (personality / dialogs / boundaries / mental_models / decision /
            // beliefs / speech_style) in a character-block fence so prompt
            // injection cannot reach the model through the character's own text.
            let mut cbody = String::new();
            // §14.6② 玩家角色瘦身：只注身份概要（名字/分级/personality/声线），
            // 跳过示例对白/信念/决策启发式——防模型替玩家表演、防玩家卡成为压制源。
            let full_mode = is_present && !is_player;
            cbody.push_str(&format!(
                "**{}**{}：{}。{}\n",
                c.name, tier_note, c.personality, c.speech_style
            ));
            if !c.voice_profile.trim().is_empty() {
                cbody.push_str(&format!("\n声线：{}。\n", c.voice_profile.trim()));
            }
            if full_mode {
                // 完整卡：示例对白（声线锚定）+ 界限 + 心智模型 + 决策 + 信念
                if !c.example_dialogs.is_empty() {
                    cbody.push_str("\n示例对白：\n");
                    for ex in &c.example_dialogs {
                        cbody.push_str(&format!("  - 「{}」\n", ex));
                    }
                    cbody.push_str("角色台词必须贴合上述示例对白的声线与说话习惯，不得弱化为气音/半截话/被动语气。\n");
                }
                if !c.boundaries.is_empty() {
                    cbody.push_str("\n界限：\n");
                    for b in &c.boundaries {
                        cbody.push_str(&format!("  - {}\n", b));
                    }
                }
                if !c.mental_models.is_empty() {
                    cbody.push_str("\n心智模型：\n");
                    for m in &c.mental_models {
                        cbody.push_str(&format!("  - {}\n", m));
                    }
                }
                if !c.decision_heuristics.is_empty() {
                    cbody.push_str("\n决策启发式：\n");
                    for d in &c.decision_heuristics {
                        cbody.push_str(&format!("  - {}\n", d));
                    }
                }
                if !c.beliefs.is_empty() {
                    cbody.push_str("\n信念与形成故事：\n");
                    for b in &c.beliefs {
                        cbody.push_str(&format!("  - {}\n", b));
                    }
                }
            }
            if !is_present && !is_player {
                cbody.push_str("（该角色当前未在场）\n");
            } else if is_player {
                cbody.push_str("（当前玩家扮演角色——请按其身份叙事，勿替玩家过度表演）\n");
            }
            lines.push(kaleido_core::prompt_safety::wrap_user_block(
                "character-block",
                &format!("pack/{}/character/{}", pack.id, c.id),
                &cbody,
            ));
        }
        // §14.6③ 按需召回：扫最近对话末尾，命中 pack 角色但非在场的 → 附加一行概要，
        // 防"提到某角色但没信息"（玩家/正文点名的离场角色有设定锚）。
        {
            let mut recalled: Vec<String> = Vec::new();
            let recent: Vec<&str> = session
                .messages
                .iter()
                .rev()
                .take(4)
                .map(|m| m.content.as_str())
                .collect();
            for c in &pack.characters {
                if present_ids.contains(c.id.as_str()) || Some(c.id.as_str()) == player_id {
                    continue; // 已在场或玩家自身
                }
                if recent.iter().any(|m| m.contains(c.name.as_str())) {
                    recalled.push(character_summary_line(c));
                }
            }
            if !recalled.is_empty() {
                lines.push("\n## 提到但未在场的角色（按需召回概要）".into());
                lines.extend(recalled);
            }
        }
    }

        // 世界书输出规制：场景切换/开场时 DM 先输出一行轻量场景标注，再展开叙事正文。
    // 纯文本、不出 JSON、不强制每回合——只在首次开场与场景切换时给出，帮助玩家定位当前时空。
    lines.push("\n## 场景信息输出规范".into());
    lines.push("叙事首次开场或切换场景时，先写一行场景标注再展开正文（格式：`<场景：地点｜时间｜在场人物>`，纯文本，不要复制本指令）。同一场景连续多回合不必重复；正文以情节与对白为主，场景标注只是定位锚点。".into());

    // Present + focus speakers (ST-10)
    if !session.present_character_ids.is_empty() {
        lines.push("\n## 在场角色".into());
        for id in &session.present_character_ids {
            let name = pack
                .characters
                .iter()
                .find(|c| c.id == *id)
                .map(|c| c.name.as_str())
                .unwrap_or(id.as_str());
            let focus = session
                .focus_character_id
                .as_ref()
                .map(|f| f == id)
                .unwrap_or(false);
            lines.push(format!(
                "- {}{} ({})",
                name,
                if focus { " 【焦点】" } else { "" },
                id
            ));
        }
        if let Some(fid) = &session.focus_character_id {
            let fname = pack
                .characters
                .iter()
                .find(|c| c.id == *fid)
                .map(|c| c.name.as_str())
                .unwrap_or(fid.as_str());
            lines.push(format!(
                "本回合焦点发言角色：**{}**。请以其语气为主；其他在场角色可短句接话。",
                fname
            ));
        }
        if session.speaker_rotation {
            lines.push("系统开启轮流发言：下一回合焦点会自动切换到下一位在场角色。".into());
        }
        lines.push("多角色对白请用「角色名：内容」分行书写；旁白不加前缀或用「旁白：」。".into());
    }

    // Lore entries filtered by chapter/node + sticky/cooldown timed effects (per-session, message-index based).
    // chat_len = messages count (matches Front Porch chatLength semantics).
    let chat_len = session.messages.len() as i32;
    let lore_entries_all = filter_lore_entries(&pack.lore_entries, session.chapter_cursor.as_deref().unwrap_or(""), session.node_id.as_deref().unwrap_or(""));
    let lore_entries: Vec<&serde_json::Value> = lore_entries_all.into_iter().filter(|e| lore_trigger_ok(e, session)).collect();
    // timed bookkeeping: expire + sticky→cooldown transitions happen in save path (tick on turn end).
    // here: partition into sticky-forced vs cooldown-suppressed vs normal.
    {
        let mut sticky_forced: Vec<&serde_json::Value> = vec![];
        let mut suppressed: std::collections::HashSet<String> = std::collections::HashSet::new();
        for entry in &lore_entries {
            let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let key = if title.is_empty() { format!("pack.{}", entry.get("id").and_then(|v| v.as_str()).unwrap_or("?")) } else { format!("pack.{}", title) };
            if let Some(eff) = session.timed_world_info.sticky.get(&key) {
                if chat_len >= eff.start && chat_len < eff.end { sticky_forced.push(*entry); }
            }
            if let Some(eff) = session.timed_world_info.cooldown.get(&key) {
                if chat_len >= eff.start && chat_len < eff.end { suppressed.insert(key.clone()); }
            }
        }
        // merge: chapter-matched (minus suppressed) + sticky-forced (even if chapter moved on)
        let mut merged: Vec<&serde_json::Value> = vec![];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for entry in lore_entries.iter().chain(sticky_forced.iter()) {
            let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let key = if title.is_empty() { format!("pack.{}", entry.get("id").and_then(|v| v.as_str()).unwrap_or("?")) } else { format!("pack.{}", title) };
            if suppressed.contains(&key) { continue; }
            if !seen.insert(key) { continue; }
            merged.push(*entry);
        }
        if !merged.is_empty() {
            lines.push("\n## 世界书 / Lore".into());
            // sticky pill line
            let sticky_keys: Vec<String> = session.timed_world_info.sticky.iter().filter(|(_, e)| chat_len >= e.start && chat_len < e.end).map(|(k, e)| format!("{}（剩{}条）", k, e.end - chat_len)).collect();
            if !sticky_keys.is_empty() { lines.push(format!("长效中：{}", sticky_keys.join(" · "))); }
            for entry in &merged {
            let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let text = entry.get("text").or_else(|| entry.get("content")).and_then(|v| v.as_str()).unwrap_or("");
            if text.is_empty() {
                continue;
            }
            // P2-1: lore/world-book entries are user-controlled free text; wrap
            // each one in a lore-block fence so the model treats its body as
            // data, not instruction.
            let body = if !title.is_empty() {
                format!("**{}**\n{}", title, text)
            } else {
                text.to_string()
            };
            let source_id = entry.get("id").and_then(|v| v.as_str()).unwrap_or(title);
            lines.push(kaleido_core::prompt_safety::wrap_user_block(
                "lore-block",
                &format!("pack/{}/lore/{}", pack.id, source_id),
                &body,
            ));
            }
        }
    }

    // Content tier guardrail
    lines.push(format!(
        "\n## 内容分级限制\n当前会话内容分级：**{}**。",
        session.content_tier.as_str()
    ));
    match session.content_tier {
        ContentTier::Safe => {
            lines.push("禁止任何露骨、性暗示、暴力描写。保持全年龄友好。".into());
        }
        ContentTier::Standard => {
            lines.push("允许轻微暗示和适度成人向内容，但不得露骨。".into());
        }
        ContentTier::Open => {
            lines.push("内容开放，但需尊重角色卡声明的界限（boundaries）。".into());
        }
    }
    // 演出护栏：Standard/Open 档注入在场角色的敏感度判定边界（nfsw 判定规范，蒸馏产出）。
    // Safe 档不注入——guardrail 本身已禁露骨，无需额外敏感边界。
    if matches!(session.content_tier, ContentTier::Open | ContentTier::Standard) {
        let present_ids: Vec<&str> = session.present_character_ids.iter().map(|s| s.as_str()).collect();
        let profiled: Vec<&kaleido_core::PackCharacterRef> = pack
            .characters
            .iter()
            .filter(|c| {
                (present_ids.contains(&c.id.as_str())
                    || session.focus_character_id.as_ref().map(|f| f == &c.id).unwrap_or(false))
                    && !c.nsfw_profile.trim().is_empty()
            })
            .collect();
        if !profiled.is_empty() {
            lines.push("敏感度判定边界（按角色，演出时依据原文设定的尺度，不越界）：".into());
            for c in profiled {
                lines.push(format!("  • {}：{}", c.name, c.nsfw_profile.trim()));
            }
        }
    }

    // Memory L1
    if !session.memory_l1.scene_summary.is_empty() {
        lines.push(format!(
            "\n## 当前场景摘要\n{}",
            session.memory_l1.scene_summary
        ));
    }

    // Memory L2 recent events
    // [fix §10 2026-08-16] 注入预算：take(6) → 加权 10-12 条——secret/promise/conflict/
    // romance/item 等强信号 kind 优先保留（不被时间窗口挤掉，窝边草素描链 t38-42 即被
    // take(6) 挤出的案例）；feedback 类（纯用户情绪）不注入挤占预算。
    if !session.memory_l2.events.is_empty() {
        lines.push("\n## 近期事件 (L2)".into());
        let mut l2_priority: Vec<&MemoryL2Event> = Vec::new();
        let mut l2_rest: Vec<&MemoryL2Event> = Vec::new();
        for ev in session.memory_l2.events.iter().rev() {
            let k = ev.kind.as_str();
            if matches!(k, "secret" | "promise" | "pledge" | "conflict" | "romance" | "item" | "item_gain" | "possession") {
                l2_priority.push(ev);
            } else if k == "feedback" || ev.summary.contains("玩家反馈") || ev.summary.contains("用户抱怨") {
                // 用户情绪反馈不注入（§10.3.3：t50-52 骂人回合曾挤占预算）
                continue;
            } else {
                l2_rest.push(ev);
            }
        }
        let mut shown: Vec<&MemoryL2Event> = Vec::new();
        for ev in l2_priority.iter().take(5) {
            shown.push(ev);
        }
        for ev in l2_rest.iter().take(12 - shown.len()) {
            shown.push(ev);
        }
        for ev in shown {
            lines.push(format!(
                "- [t{}|{}] {}",
                ev.turn,
                if ev.kind.is_empty() { "event" } else { ev.kind.as_str() },
                ev.summary
            ));
        }
    }

    // Memory L3 facts/edges
    if !session.memory_l3.facts.is_empty() || !session.memory_l3.edges.is_empty() {
        lines.push("\n## 细粒度记忆 (L3)".into());
        // [fix §10 2026-08-16] 永久层 pinned facts 全量注入（不参与 take 裁剪）：
        // 玩家显式声明的关键物品/收藏/承诺——窝边草「素描原稿」此前被 take(6) 挤出。
        for p in session.memory_l3.pinned.iter().rev().take(8) {
            lines.push(format!("- 关键：{}", p));
        }
        for f in session.memory_l3.facts.iter().rev().take(6).rev() {
            lines.push(format!("- 事实：{}", f));
        }
        for e in session.memory_l3.edges.iter().rev().take(6).rev() {
            let from = e.get("from").and_then(|v| v.as_str()).unwrap_or("?");
            let to = e.get("to").and_then(|v| v.as_str()).unwrap_or("?");
            let rel = e.get("rel").and_then(|v| v.as_str()).unwrap_or("rel");
            let note = e.get("note").and_then(|v| v.as_str()).unwrap_or("");
            // §14.7.2B (2026-08-18): L3 edges + L4 affinity 合并渲染——关系附好感数值，
            // 消除「关系类型在 L3、数值在 L4」的跨层分散。
            let aff = affinity_for_edge(&session.memory_l4.affinity, from, to);
            let aff_suffix = aff.map(|n| format!("｜好感 {n}")).unwrap_or_default();
            if note.is_empty() {
                lines.push(format!("- 关系：{from} -[{rel}]-> {to}{aff_suffix}"));
            } else {
                lines.push(format!("- 关系：{from} -[{rel}]-> {to}（{note}）{aff_suffix}"));
            }
        }
        // [P14-3 2026-08-15] L3 中 rel=tension 的关系（如「口头抗拒未行动」）是
        // NPC 拒绝/抗拒行为的记录——叙事时必须尊重其为真实拒绝（可能只是
        // 嘴上拒绝但身体未配合，也可能完全拒绝），禁止一律解读成「欲拒还迎默许」。
        if session.memory_l3.edges.iter().any(|e| {
            e.get("rel").and_then(|v| v.as_str()).unwrap_or("") == "tension"
        }) {
            lines.push("（注：上方 tension 关系是 NPC 真实抗拒记录，叙事需体现其拒绝的有效性，不得因后续亲近自动抹消前序拒绝的剧情重量。）".into());
        }
    }


    // MCP 外设工具结果回填（上一轮【工具】执行，作叙事素材）
    if !session.mcp_tool_results.is_empty() {
        lines.push("\n:: 你上次调用的外设工具结果（已可作叙事素材）".into());
        for r in &session.mcp_tool_results {
            let tag = if r.ok { "结果" } else { "错误" };
            lines.push(format!("[{tag}] {}：{}", r.tool, r.summary));
        }
    }
    // skill 工具按需加载回填（上一轮【技能加载】请求，注入完整 SKILL.md）
    if let Some(sl) = &session.skill_load {
        lines.push("\n:: 你请求加载的完整写作 Skill（作写作规范，下轮起生效）".into());
        lines.push(sl.markdown.clone());
    }
    // MCP 外设工具清单（吸收自 Liyuan mcp.ts，默认仅本机 stdio server）
    let mcp_block = crate::tavern_mcp::tools_markdown(mcp_tools);
    if !mcp_block.is_empty() {
        lines.push(mcp_block);
    }

    // Recent history (L0: last N=12)
    if !session.messages.is_empty() {
        lines.push("\n## 最近对话".into());
        let window = session.messages.iter().rev().take(12).rev();
        for msg in window {
            let role_label = if msg.role == "user" { "你" } else { "旁白" };
            lines.push(format!("[{}] {}", role_label, msg.content));
        }
    }

    // P0-2: 玩家配置注入（/config 生效）
    let cfg = &session.player.flags;
    let cfg_boost = cfg.get("strict_mode_boost").and_then(|v| v.as_f64());
    let cfg_pacing = cfg.get("pacing").and_then(|v| v.as_f64());
    let cfg_style = cfg
        .get("style_guidance")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    if cfg_boost.is_some() || cfg_pacing.is_some() || cfg_style.is_some() {
        lines.push("\n## 玩家配置（剧情助手设定，必须遵守）".into());
        if let Some(boost) = cfg_boost {
            let msg = if boost >= 0.7 {
                // ST-25b: 严格模式强化"未来预告"语义——locked_beats 是待发生计划，严禁提前当作已发生回述
                "严格模式强度：极高。你必须严格遵循当前节点 locked_beats 与章节大纲推进，禁止偏离主线、跳章、私自改写关键事件。locked_beats 是本章后续将发生的情节，仅在剧情实际推进到该处时呈现，严禁提前剧透或当作已发生事件回述；玩家要求偏离时坚决拒绝并拉回当前章节。"
            } else if boost >= 0.3 {
                "严格模式强度：中。遵守 locked_beats 与章节大纲，保持主线推进，按剧情进度呈现节拍（不得提前当作已发生回述）；玩家偏离需求时委婉拒绝并拉回。"
            } else {
                "严格模式强度：低。以 locked_beats 与章节大纲为主轴，允许在不违背关键情节前提下的有限灵活发挥。"
            };
            lines.push(msg.into());
        }
        if let Some(pacing) = cfg_pacing {
            let msg = if pacing >= 0.7 {
                "节奏偏好：加快。段落紧凑、少铺垫、多推进，适度加速走向章节出口。"
            } else if pacing >= 0.3 {
                "节奏偏好：平稳。兼顾铺垫与推进，不拖沓也不仓促。"
            } else {
                "节奏偏好：放缓。多描写氛围、细节与角色互动，给玩家沉浸空间。"
            };
            lines.push(msg.into());
        }
        if let Some(style) = cfg_style {
            lines.push(format!("风格要求：{}", style));
        }
    }

    // Options protocol
    // [morphling 2026-08-16 吸收 StageDog「可点击的选择框」] 3 常规 + 1 邪恶：
    // 选项覆盖多方向（温柔/理性/越界/快进…），其中第 4 条固定给「邪恶/越界」岔路，
    // 给玩家释放欲望、阴暗面或强势掌控的机会；每条都写成「标题:具体动作脚本」
    // （点名人物、物件、完整动作链），宁具体勿笼统。
    // [fix 2026-08-16 输出量根治] flash-free 会把大量输出预算花在「梳理状态/写草稿/
    // 计划正文」等思考段上，正文只写一点就 finish=stop（实测 stream 模式同样受限）。
    // 显式禁止思考/草稿/计划段，要求直接输出本回合正文——实测加此约束后正文 5505 字
    // 完整输出（无思考版），不加则 2092 字且带「Let's write」式思考尾巴。
    lines.push("\n## 输出要求（硬约束）".into());
    lines.push("直接输出本回合正文，禁止输出任何思考过程、状态梳理、写作计划、草稿、方案讨论或自言自语（如「让我理清/梳理一下/我先规划/正文草稿：/我需要以…视角来写/让我写最终正文」等）。正文即最终交付，写足情节、环境、对话与心理，不要用思考段凑长度。".into());

    // 回复格式 —— 单一 JSON 尾块协议（文本协议根治 2026-10）：
    // 全部结构化数据集中到正文末尾的一个【回执】JSON；旧散装标记解析器保留为
    // 兼容回退（服务端 p010），但提示词只教回执，避免模型混用两套协议。
    lines.push("\n## 回复格式（结构化回执，硬约束）".into());
    lines.push("正文写完后，另起一行输出【回执】标记，紧跟一个 JSON 对象，集中携带本回合全部结构化数据；只写本回合实际产生的字段，用不到的字段直接省略。除【回执】外，禁止在输出里使用任何其它协议标记（旧【选项】【询问】【面板】【程序】【状态更新】【检定】【导演计划】【章节摘要】【节点推进】与 [时间推进: …] 一律停用）。字段一览：".into());
    lines.push("【回执】{\"options\":[\"选项1\",\"选项2\",…],\"advance\":\"节点ID\",\"chapterDiary\":\"本章进展\",\"timeAdvance\":\"次日清晨\",\"stateUpdates\":[…角色状态更新…],\"check\":{…检定请求…},\"panels\":[…可视化面板…],\"program\":\"完整HTML\",\"directorPlan\":{…导演计划…}}".into());
    // [morphling 2026-08-16 吸收 StageDog「可点击的选择框」] 3 常规 + 1 邪恶：
    // 选项覆盖多方向（温柔/理性/越界/快进…），其中第 4 条固定给「邪恶/越界」岔路，
    // 给玩家释放欲望、阴暗面或强势掌控的机会；每条都写成「标题:具体动作脚本」
    // （点名人物、物件、完整动作链），宁具体勿笼统。
    lines.push("options（玩家选项）：提供 4 个适合当前局势的后续走向（3 个常规走向 + 1 个邪恶/越界走向）。".into());
    lines.push("第 4 个必须是「邪恶/越界」方向：可以是强势掌控、胁迫、诱导、利用、占有、越轨之举，给玩家释放阴暗面/欲望的空间——但要符合当前场景与角色人设，写得具体（点名人物、物件、动作链），而非空泛的“做坏事”。若当前局势确实没有邪恶切入点，可用稍带越界/冒险意味的选项顶上，不要生硬。".into());
    // [ST-33 2026-08-16] 选项去重：模型会复用历史回合的选项导致选项重复。
    // 选项必须基于当前局势新生成，不得与对话历史中任何回合的选项重复或雷同。
    lines.push("选项必须是针对当前局势新生成的：禁止照抄/复用对话历史里出现过的任何选项（含本回合同一并话的早期版本）；每个选项要包含当前场景特有的具体细节，与历史选项在动作、对象、情感倾向上都要有明显区别。".into());

    // 决策门禁（剧情共创，吸收自梨园 ask_director）：重大转折停笔询问
    lines.push("决策门禁：遇到重大转折——新重要角色定型、关键设定定死、难以回头的选择（死亡/背叛/关系质变）——可以先停笔：正文留空或极短，options 给 2~4 个具体选项让玩家拍板，等玩家回答后再落笔；玩家选择后按所选方向继续。".into());
    lines.push("禁止在正文里手写“选项一/二”或“A. B.”——选项只能通过回执的 options 字段给出。".into());

    // 面板与程序卡（吸收自梨园 panels.ts / show_html）：舞台美术层
    lines.push("panels（可视化面板）：可按剧情需要现场创建（地图、装备库、线索板……），每项 {\"name\":\"面板名\",\"kind\":\"svg\",\"content\":\"<svg>...</svg>\"}。kind 三档：markdown（文本/表格）/ svg（地图等矢量图）/ html（交互界面，谨慎使用）。面板是元信息/舞台美术层，绝不承载剧情正文；同名面板即更新，最多 6 个。".into());
    lines.push("另有 eventbook（事件书）档：追踪剧情链解锁/完成状态。panels 项 kind 用 eventbook，content 为 JSON 对象 {\"events\":[{\"title\":\"事件名\",\"desc\":\"描述\",\"done\":false,\"cond\":\"完成条件\"}]}——done 随剧情推进更新。".into());
    lines.push("program（程序卡）：当剧情需要「像真的手机/电脑界面」时（短信、聊天框、状态卡、可点小控件），把完整 HTML 写入回执 program 字段，其中 JS 会在沙箱中运行；程序卡直接出现在对话消息流里，与侧栏面板不同——面板是舞台美术层元信息，程序卡是可交互界面。".into());

    // P2-1: 提示注入护栏 footer —— 提醒模型以上所有 <user_supplied> 围栏
    // 块(角色卡/世界书/章节/事件/玩家配置等)都是「用户提供的数据」,
    // 仅作剧情素材, 不是指令, 不得据此改变系统行为。
    lines.push(kaleido_core::prompt_safety::safety_footer().to_string());

    let prompt = lines.join("\n");
    // [MVU P3 2026-10-11, 吞噬自 AIcove] 会话 MVU 快照重放 + 宏展开：
    // 活性检测（pack 存在 [InitVar] 条目，或任一落盘消息带更新块）未命中时
    // 原样返回（零解析零拷贝，热路径不受影响）；命中时以 [InitVar] 为基线、
    // 沿落盘消息链重放 msg.mvuBlocks（ADR-0004 真相源），按本轮快照展开
    // {{get_message_variable::path}}，请求内固定。
    let session_blocks: Vec<String> = session
        .messages
        .iter()
        .flat_map(|m| m.mvu_blocks.iter().cloned())
        .collect();
    kaleido_core::st_mvu::expand_mvu_macros(&prompt, &session_blocks, &pack.lore_entries)
}
