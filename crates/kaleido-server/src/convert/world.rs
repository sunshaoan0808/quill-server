//! convert/world —— 世界/事件/规则蒸馏（lore/event packages/actor state/style/node/rule）
#![allow(unused_imports)]

use super::*;

/// 节拍数组解析：过滤空串、截断长度与条数。
pub(crate) fn normalize_world_lore_entry(v: &Value) -> Option<Value> {
    let title = jstr(v, "title", "title");
    let text = jstr(v, "text", "text");
    if title.is_empty() || text.is_empty() {
        return None;
    }
    let importance = jstr(v, "importance", "importance");
    let importance = if importance.is_empty() { "low".into() } else { importance };
    let kind = jstr(v, "type", "type");
    let kind = if kind.is_empty() { "setting".into() } else { kind };
    let confidence = v
        .get("confidence")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.5)
        .clamp(0.0, 1.0);
    let activation = {
        let a = jstr(v, "activation", "activation");
        if a.is_empty() { "keyword".to_string() } else { a }
    };
    let depth = v.get("depth").and_then(|x| x.as_i64()).map(|x| x.clamp(1, 10)).unwrap_or(5);
    Some(json!({
        "title": title,
        "text": text,
        "tags": jstr_arr(v, "tags", "tags"),
        "permanent": v.get("permanent").and_then(|x| x.as_bool()).unwrap_or(true),
        "importance": importance,
        "type": kind,
        "evidence_refs": jstr_arr(v, "evidence_refs", "evidence_refs"),
        "confidence": confidence,
        "activation": activation,
        "depth": depth,
    }))
}

/// 每章节拍：对每章喂正文（截断到 max_chars），LLM 输出该章实际发生的 1-3 条硬节拍。
///
/// 返回 Vec 与 chapters 对齐；某章正文为空时该章产出空 Vec。
pub async fn distill_world_lore(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
) -> Result<Vec<Value>, String> {
    if chapters.is_empty() {
        return Ok(vec![]);
    }
    let inventory_input = build_roster_input(title, chapters, max_chars);
    let inventory_system = "你是小说世界观梳理专家。请从给定小说正文中盘点世界观实体（世界/势力/地点/物品/规则/背景设定），只输出 JSON 数组，每个元素为 {\"name\":\"实体名\",\"type\":\"world|faction|location|item|rule|setting\",\"keywords\":[\"检索用关键词\"],\"importance\":\"high|medium|low\"}。importance 依据对剧情的重要性判断。只输出 JSON。";
    let inv_v = match llm_json(state, inventory_system, &inventory_input, "世界树[实体清单]").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("世界树: 实体清单失败，跳过世界蒸馏: {e}");
            return Ok(vec![]);
        }
    };
    let inventory = as_json_array(inv_v);

    let rank = |im: &str| -> u8 { match im { "high" => 0, "medium" => 1, _ => 2 } };
    let mut entities: Vec<Value> = inventory;
    entities.sort_by_key(|e| rank(&jstr(e, "importance", "importance")));
    entities.truncate(MAX_WORLD_ENTITIES);

    let blocks = build_evidence_blocks(chapters);
    let mut out: Vec<Value> = Vec::new();
    for e in entities {
        let name = jstr(&e, "name", "name");
        if name.is_empty() {
            continue;
        }
        let kind = jstr(&e, "type", "type");
        let mut query = name.clone();
        for kw in jstr_arr(&e, "keywords", "keywords") {
            query.push(' ');
            query.push_str(&kw);
        }
        if !kind.is_empty() {
            query.push(' ');
            query.push_str(&kind);
        }

        // 词袋余弦 top-k 召回原文证据
        let qsig = block_signature(&query.trim());
        let hits = cosine_top_k(&qsig, &blocks, WORLD_EVIDENCE_TOP_K);
        if hits.is_empty() {
            tracing::warn!(name=%name, "世界树: 无召回证据，条目不落库");
            continue;
        }
        let mut evidence = String::new();
        let mut refs: Vec<String> = Vec::new();
        for (bi, _score) in hits {
            evidence.push_str(&format!("【{}】{}\n\n", blocks[bi].chapter, blocks[bi].text));
            refs.push(format!("{}:block{}", blocks[bi].chapter, bi + 1));
        }

        let card_user = format!(
            "小说《{title}》\n世界观实体：{name}（类型：{kind}）\n\
             检索到的原文证据（【ch】前缀指示章节）：\n{evidence}\n\
             请按系统要求基于证据为该实体输出一条严格 JSON 世界树条目。"
        );
        let card_system = "你是世界观设定整理专家。请基于提供的原文证据归纳该实体的描述。\
            核心约束：1) 只许从证据中提炼，禁止编造证据之外的信息；2) text 用 3-5 句描述该实体；\
            3) tags 为 2-4 个关键词；4) importance 为 high|medium|low；\
            5) type 为 world|faction|location|item|rule|setting；\
            6) confidence 为 0.0~1.0 的置信度；\
            7) activation 为 keyword|always|conditional（该实体何时被检索激活，keyword=靠关键词命中，always=常驻，conditional=条件触发）；\
            8) depth 为 1~10 的提示词深度（该实体对主线越核心越靠前）。\
            只输出 JSON，结构（严格）：\
            {\"title\":\"实体名\",\"text\":\"3-5句描述\",\"tags\":[\"关键词\"],\"permanent\":true,\"importance\":\"high|medium|low\",\"type\":\"location\",\"evidence_refs\":[\"ch01:block3\"],\"confidence\":0.9,\"activation\":\"keyword\",\"depth\":3}";
        let card_v = match llm_json(state, card_system, &card_user, &format!("世界树[{name}]")).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(name=%name, "世界树[{name}] 蒸馏失败，不落库: {e}");
                continue;
            }
        };
        let mut entry = match normalize_world_lore_entry(&card_v) {
            Some(e) => e,
            None => {
                tracing::warn!(name=%name, "世界树: 蒸馏条目缺 title/text，不落库");
                continue;
            }
        };
        // 兜底：若 evidence_refs 空则用本次召回补足
        let refs_empty = entry
            .get("evidence_refs")
            .and_then(|x| x.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(true);
        if refs_empty {
            entry["evidence_refs"] = json!(refs);
        }
        out.push(entry);
    }
    Ok(out)
}

/// 演员状态解析（纯函数）：宽容解析 LLM 输出的 ActorStatePackConfig。
///
/// - 解析失败返回默认空配置；
/// - 模板：仅保留非空 key 且 fields/trait_pools 至少一项的模板；
/// - initial_actors：仅保留 character_id/template_id 非空且模板存在的条目。
/// - [fix 2026-08-15 白名单过滤] `allowed_chars`：角色卡名单（Some 时生效）。
///   丢弃 character_id 不在名单的 initial_actors，并清理无引用的孤立模板
///   （实证：LLM 自创「舍友」模板但角色卡只有「莫旺财」→ 悬空引用）。
pub(crate) fn parse_event_packages(v: &Value) -> Vec<EventPackage> {
    let mut out: Vec<EventPackage> = Vec::new();
    for pkg in as_json_array(v.clone()) {
        let id = jstr(&pkg, "id", "id");
        let name = jstr(&pkg, "name", "name");
        if id.is_empty() || name.is_empty() {
            tracing::warn!("事件包蒸馏: 包缺 id/name，跳过");
            continue;
        }
        let mut cards: Vec<TellerEventCard> = Vec::new();
        let raw_cards = pkg
            .get("cards")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        for c in raw_cards {
            let card_id = jstr(&c, "id", "id");
            let card_title = jstr(&c, "title", "title");
            let card_prompt = jstr(&c, "prompt", "prompt");
            if card_id.is_empty() || card_title.is_empty() || card_prompt.is_empty() {
                tracing::warn!(pkg_id = %id, "事件包蒸馏: 卡缺 id/title/prompt，跳过");
                continue;
            }
            let weight = c
                .get("weight")
                .and_then(|x| x.as_u64())
                .map(|x| x as u32)
                .unwrap_or(1)
                .max(1);
            let enabled = c.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
            let once_per_session = c
                .get("once_per_session")
                .or_else(|| c.get("oncePerSession"))
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            // G7 新字段宽容解析：camel 优先、容忍 snake；全部缺省不跳卡（沿用「缺字段兜底」语义）
            let type_name = c
                .get("typeName")
                .or_else(|| c.get("type_name"))
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let category = jstr(&c, "category", "category");
            let tags = jstr_arr(&c, "tags", "tags");
            let intensity = jstr(&c, "intensity", "intensity");
            let cooldown_turns = c
                .get("cooldownTurns")
                .or_else(|| c.get("cooldown_turns"))
                .and_then(|x| x.as_u64())
                .map(|x| x as u32)
                .unwrap_or(0);
            // A1: 章节范围解析（蒸馏产出 chapterRange，基于切分 chXX 编号）。
            // 容忍 camel/snake，缺省空 = 未标注 → 运行时任意章可抽（A3 兼容旧 pack）。
            let chapter_range = c
                .get("chapterRange")
                .or_else(|| c.get("chapter_range"))
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default();
            cards.push(TellerEventCard {
                id: card_id,
                title: card_title,
                prompt: card_prompt,
                weight,
                enabled,
                once_per_session,
                used_in_session: false,
                type_name,
                category,
                tags,
                intensity,
                cooldown_turns,
                chapter_range,
            });
            if cards.len() >= MAX_EVENT_CARDS {
                break;
            }
        }
        if cards.is_empty() {
            tracing::warn!(pkg_id = %id, "事件包蒸馏: 整包卡全非法，跳过该包");
            continue;
        }
        let description = jstr(&pkg, "description", "description");
        out.push(EventPackage {
            id,
            name,
            description: if description.is_empty() {
                None
            } else {
                Some(description)
            },
            enabled: pkg.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true),
            cards,
        });
        if out.len() >= MAX_EVENT_PACKAGES {
            break;
        }
    }
    out
}

/// 文风归一化（纯函数）：非对象返回空对象，字段缺省补空串。
pub(crate) fn parse_actor_state_config(v: &Value, allowed_chars: Option<&std::collections::HashSet<String>>) -> ActorStatePackConfig {
    let mut cfg: ActorStatePackConfig = match serde_json::from_value(v.clone()) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("演员状态蒸馏: 无法解析 ActorStatePackConfig: {e}");
            return ActorStatePackConfig::default();
        }
    };
    cfg.templates.retain(|k, t| {
        !k.trim().is_empty() && (!t.fields.is_empty() || !t.trait_pools.is_empty())
    });
    cfg.initial_actors.retain(|ia| {
        !ia.character_id.trim().is_empty()
            && !ia.template_id.trim().is_empty()
            && cfg.templates.contains_key(&ia.template_id)
            && allowed_chars.map_or(true, |s| s.contains(&ia.character_id.trim().to_string()))
    });
    // 清理无 initial_actors 引用的孤立模板（自创角色名被过滤后残留）
    let referenced: std::collections::HashSet<String> = cfg
        .initial_actors
        .iter()
        .map(|ia| ia.template_id.clone())
        .collect();
    cfg.templates.retain(|k, _| referenced.contains(k));
    cfg.schema_version = 1;
    cfg
}

/// 规则检定解析（纯函数）：宽容解析 RuleSystem，过滤 id/label/dice 空的条，
/// 上限 [`MAX_RULE_CHECKS`]，返回 `{"checks":[...]}`。
pub(crate) fn normalize_narrative_style(v: &Value) -> Value {
    if !v.is_object() {
        return json!({});
    }
    json!({
        "style": jstr(v, "style", "style"),
        "tone": jstr(v, "tone", "tone"),
        "pacing": jstr(v, "pacing", "pacing"),
        "scene_focus": jstr(v, "scene_focus", "sceneFocus"),
        "prose_guidance": jstr(v, "prose_guidance", "proseGuidance"),
    })
}

/// 素材库蒸馏：事件包。LLM 一次生成 2~4 个事件包（奇遇/战斗/社交/修炼/日常等），
/// 每包 3~5 张 `TellerEventCard`，prompt 必须使用小说世界真实元素（盘点实体名注入 system）。
///
/// LLM 失败/解析失败一律 `tracing::warn!` 并返回 Ok(已收集部分或空)，不整批中止。
pub(crate) fn normalize_rule_system(v: &Value) -> Value {
    let raw = v.get("checks").cloned().unwrap_or_else(|| v.clone());
    let sys: RuleSystem = match serde_json::from_value(json!({ "checks": raw })) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("规则检定: 解析失败，返回空: {e}");
            return json!({ "checks": [] });
        }
    };
    let mut out: Vec<Value> = Vec::new();
    for c in sys.checks {
        if c.id.trim().is_empty() || c.label.trim().is_empty() || c.dice.trim().is_empty() {
            continue;
        }
        out.push(serde_json::to_value(c).unwrap_or_else(|_| json!({})));
        if out.len() >= MAX_RULE_CHECKS {
            break;
        }
    }
    if out.len() < MIN_RULE_CHECKS {
        tracing::warn!(
            got = out.len(),
            "规则检定: 有效条数低于下限，仍返回已收集部分"
        );
    }
    json!({ "checks": out })
}

/// 素材库蒸馏：演员状态。LLM 一次生成完整 ActorStatePackConfig JSON，
/// 按角色卡为各角色建 template（字段含数值范围/枚举/更新说明）并声明 initial_actors。
///
/// LLM 失败/解析失败一律 `tracing::warn!` 并返回 Ok(默认空配置)，不整批中止。
pub async fn distill_event_packages(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
) -> Result<Vec<EventPackage>, String> {
    if chapters.is_empty() {
        return Ok(vec![]);
    }
    let input = build_roster_input(title, chapters, max_chars);
    // 盘点实体/人物名（失败降级为空，不影响主流程）
    let names = distill_name_inventory(state, title, chapters, max_chars).await;
    let entity_hint = if names.is_empty() {
        String::new()
    } else {
        format!(
            "\n本小说的关键实体/人物（事件卡 prompt 必须尽量使用这些真实名字，而非泛称）：\n{}",
            names.join("、")
        )
    };
    let system = format!(
        "你是小说演出事件设计师。请从给定小说正文归纳该世界的演出事件包。\
         每个事件包是一组\"事件卡\"，在每回合演出时按权重抽取一张，注入 LLM 演出提示。\
         核心约束：1) 按故事类型组织事件包（如奇遇/战斗/社交/修炼/日常/悬疑），2-4 个包；\
         2) 每包 3-5 张事件卡；3) 事件卡 prompt 必须使用小说世界的真实元素（地名/人物/境界/势力），\
         描述该事件如何在本世界演出，禁止空泛模板；4) enabled 默认 true；\
         5) 对\"一次性的奇遇/大事件\"卡 oncePerSession 设 true，日常可重复卡设 false；\
         6) weight 为抽取权重（1-10）；\
         7) typeName 为用户可见的事件类型名（如「外门考核打脸」）；category 从包名/卡性质归纳\
         （如 打脸/奇遇/秘境/恋爱/冲突…）；\
         8) intensity 三档 low/medium/high；tags 给 2-3 个短标签便于检索；\
         9) cooldownTurns 为冷却回合：可重复卡设 2-3（抽过后 N 回合内不再抽），\
         一次性大事件卡设 0 或高值。\
         10) chapterRange 标注该事件发生的章节范围，格式为切分序号 chXX 列表\
         （如 \"ch01\"、[\"ch01\",\"ch03\"]）。输入章节前缀【chXX:原著标题】中的 chXX\
         即切分序号，必须使用该编号体系（严禁写中文数字或原著标题）。\
         日常/持续性事件可标较宽范围，一次性大事件（如首次登场/名场面）标精确章节甚至单章。{entity_hint}\
         只输出 JSON 数组，结构（严格）：\
         [{{\"id\":\"pkg-adventure\",\"name\":\"奇遇包\",\"description\":\"描述\",\"enabled\":true,\
         \"cards\":[{{\"id\":\"card-1\",\"title\":\"标题\",\"prompt\":\"演出提示\",\"weight\":3,\
         \"enabled\":true,\"oncePerSession\":true,\"typeName\":\"外门考核打脸\",\"category\":\"打脸\",\
         \"tags\":[\"门派\",\"考核\"],\"intensity\":\"medium\",\"cooldownTurns\":2,\"chapterRange\":[\"ch01\",\"ch03\"]}}]}}]"
    );
    let v = match llm_json(state, &system, &input, "事件包[生成]").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("事件包: 生成失败，返回空: {e}");
            return Ok(vec![]);
        }
    };
    Ok(parse_event_packages(&v))
}

/// 素材库蒸馏：文风引导。LLM 归纳全书作者文风，
/// 输出 `{style,tone,pacing,scene_focus,prose_guidance}` 写入 resolved_snapshot.narrative_style。
///
/// LLM 失败/解析失败一律 `tracing::warn!` 并返回 Ok(空对象)，不整批中止。
pub async fn distill_actor_state(
    state: &AppState,
    title: &str,
    characters: &[PackCharacterRef],
    chapters: &[(String, String)],
    max_chars: usize,
) -> Result<ActorStatePackConfig, String> {
    let cfg = ActorStatePackConfig::default();
    if chapters.is_empty() {
        return Ok(cfg);
    }
    let input = build_roster_input(title, chapters, max_chars);
    let mut char_roster = String::new();
    for c in characters {
        char_roster.push_str(&format!(
            "- {}（{}）\n",
            c.name,
            if c.role.is_empty() { "未知" } else { &c.role }
        ));
    }
    let system = "你是小说角色状态配置专家。请为给定小说的角色设计 ActorState 模板与初始状态。\
        每个角色一个模板（字段：数值状态如修为/境界/伤势/好感/金钱，含 valueType/min/max/value；\
        字符串状态如身份；枚举状态如势力立场；可附带特质池 traitPools）。\
        核心约束：1) 主角与重要配角各建一个模板；2) 每个模板对应 initialActors 中一条\
        （characterId 用角色卡名，templateId 指向 templates 的 key）；\
        3) valueType 为 number|string|bool|enum|object|list，number 必须给 min/max；\
        4) value 为初始值；5) display 为该字段展示名；6) updateInstruction 说明剧情中该字段如何变化。\
        [fix 2026-08-15 白名单] characterId 必须严格取自下方「角色卡清单」中的名字，\
        禁止自创清单外的角色名（如「舍友」——若清单里是「莫旺财」就必须用「莫旺财」）；\
        只输出 JSON，结构（严格）：\
        {\"schemaVersion\":1,\"initialActors\":[{\"characterId\":\"角色名\",\"templateId\":\"tpl-xxx\"}],\
        \"templates\":{\"tpl-xxx\":{\"fields\":{\"修为\":{\"valueType\":\"number\",\"value\":10,\"min\":0,\"max\":100,\
        \"display\":\"修为\",\"updateInstruction\":\"修炼/战斗后变化\"}},\
        \"traitPools\":[{\"id\":\"pool-1\",\"name\":\"特质\",\"description\":\"...\",\
        \"traits\":[{\"id\":\"t1\",\"name\":\"...\",\"summary\":\"...\",\"weight\":1}]}]}}}";
    let user = format!(
        "{input}\n角色卡清单：\n{char_roster}\n请按系统要求为这些角色生成 ActorStatePackConfig JSON。"
    );
    let v = match llm_json(state, system, &user, "演员状态[生成]").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("演员状态: 生成失败，返回空: {e}");
            return Ok(cfg);
        }
    };
    // [DIAG] 落盘 LLM 原始返回 + 白名单过滤详情，排查「白名单后全空」场景
    {
        let allow: std::collections::HashSet<String> = characters
            .iter()
            .map(|c| c.name.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let raw_names: Vec<String> = v
            .get("initialActors")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|e| e.get("characterId").and_then(|c| c.as_str()))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        let rejected: Vec<&String> = raw_names
            .iter()
            .filter(|n| !allow.contains(n.as_str()))
            .collect();
        tracing::info!(
            raw_count = raw_names.len(),
            rejected_count = rejected.len(),
            rejected = ?rejected,
            "演员状态[白名单诊断]"
        );
    }
    Ok(parse_actor_state_config(
        &v,
        Some(
            &characters
                .iter()
                .map(|c| c.name.trim().to_string())
                .filter(|n| !n.is_empty())
                .collect::<std::collections::HashSet<_>>(),
        ),
    ))
}

/// 方案A(2026-08-16)：`node.entry` 叙事入口蒸馏。
///
/// 回推最初设计（12c748ca）：`node.entry` 语义应为"玩家进入该节点第一眼看到的
/// 叙事入口句"（如 `雨夜，玩家抵达旧茶馆门前`），而非 `build_pack_from_chapters`
/// 里硬编码的占位 `"本章开始"`。这里用单次 LLM 调用为每个节点生成 1~2 句叙事入口。
///
/// 失败不阻主线：返回 `HashMap<node_id, entry>`；解析/LLM 失败返回 Err，
/// 由调用方 `tracing::warn!` 后保留原占位 entry。
pub async fn distill_narrative_style(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
) -> Result<Value, String> {
    if chapters.is_empty() {
        return Ok(json!({}));
    }
    let input = build_roster_input(title, chapters, max_chars);
    let system = "你是小说文风分析师。请从给定小说正文归纳作者的叙事文风，只输出 JSON 对象，字段：\
        style（叙事风格，如\"第三人称全知\"\"第一人称沉浸\"）；\
        tone（整体基调，如\"热血\"\"悬疑\"\"日常治愈\"）；\
        pacing（叙事节奏，如\"快节奏爽文\"\"慢热细腻\"）；\
        scene_focus（描写侧重，如\"战斗场面\"\"人物心理\"\"环境氛围\"）；\
        prose_guidance（对 LLM 演出时的行文指引，2-3 句，指出该仿照的句式/修辞/人称习惯）。\
        只输出 JSON。";
    let v = match llm_json(state, system, &input, "文风[生成]").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("文风: 生成失败，返回空: {e}");
            return Ok(json!({}));
        }
    };
    Ok(normalize_narrative_style(&v))
}

/// 素材库蒸馏：规则检定。LLM 输出 4~8 条与小说世界匹配的 RuleCheck（d20/d100），
/// 结果 `{"checks":[...]}` 写入 resolved_snapshot.rule_system。
///
/// LLM 失败/解析失败一律 `tracing::warn!` 并返回 Ok(空 checks)，不整批中止。
pub async fn distill_node_entries(
    state: &AppState,
    title: &str,
    nodes: &[kaleido_core::StoryNode],
    chapters: &[(String, String)],
) -> Result<std::collections::HashMap<String, String>, String> {
    let client = reqwest::Client::new();
    let llm = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return Err("节点入口蒸馏: LLM 未配置".into());
    }
    let chapter_map: std::collections::HashMap<&str, &str> =
        chapters.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut blocks = Vec::new();
    for n in nodes {
        let head = chapter_map
            .get(n.chapter_id.as_str())
            .map(|b| b.chars().take(600).collect::<String>())
            .unwrap_or_default();
        if head.trim().is_empty() {
            continue;
        }
        blocks.push(json!({
            "nodeId": n.id,
            "chapter": n.chapter_id,
            "title": n.title,
            "opening": head,
        }));
    }
    if blocks.is_empty() {
        return Ok(Default::default());
    }
    let system = "你是小说剧本节点的'叙事入口'提炼助手。为每个节点从对应章节正文开头提炼一句'入口'(entry)：玩家进入该节点第一眼看到的、有画面感、带氛围/微悬疑的场景句，2~30 字，第三人称。示例：'雨夜，玩家抵达旧茶馆门前'、'推门见林晚，对上约定'。\
规则：1) 只能从该章开头已发生/已在场的事实提炼，不得编造人物或事件；2) 不得剧透本章后续的大事件/转折；3) 不用将来时、不用推测语气；4) 该章开头纯抒情/过渡、无实质场景时 entry 置空字符串。\
只输出 JSON 数组，元素为 {\"nodeId\":\"输入中的值\",\"entry\":\"入口句或空\"}，nodeId 必须与输入一一对应，不要输出任何解释文字。";
    let user_json = serde_json::to_string(&json!({
        "title": title,
        "nodes": blocks,
    }))
    .unwrap_or_default();
    let raw = crate::llm_stream::chat_completion_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        &state.provider_kind,
        system,
        &user_json,
        0.1, 16384, LLM_TIMEOUT_SECS,
        &client,
    )
    .await
    .map_err(|e| format!("节点入口蒸馏: {e}"))?;
    let v = crate::llm_stream::extract_json_value(&raw)
        .ok_or_else(|| "节点入口蒸馏: 无法解析 LLM 返回的 JSON".to_string())?;
    let arr = match v {
        Value::Array(a) => a,
        other => vec![other],
    };
    let mut out = std::collections::HashMap::new();
    for e in arr {
        let id = e.get("nodeId").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let entry = e.get("entry").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if !id.is_empty() && !entry.trim().is_empty() {
            out.insert(id, entry.trim().to_string());
        }
    }
    Ok(out)
}

pub async fn distill_rule_system(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
) -> Result<Value, String> {
    if chapters.is_empty() {
        return Ok(json!({ "checks": [] }));
    }
    let input = build_roster_input(title, chapters, max_chars);
    let system = "你是小说规则检定设计专家。请从给定小说正文归纳该世界适用的规则检定（d20 或 d100）。\
        核心约束：1) 4-8 条，与小说世界匹配（如修炼突破/战斗/交涉/盗窃/察觉/炼丹等）；\
        2) 每条字段：id（唯一）、label（检定名）、dice（d20 或 d100）、modifier（修正值，可负）、\
        failurePolicy（\"retry|block|escalate\"）、difficultyGuidance（难度指引）、\
        stateEffectGuidance（状态影响说明）、trigger（触发条件）、mustCheckExamples（必须检定的场景）、\
        skipCheckExamples（免检定的场景）、successHint、failureHint、\
        stateBindings（可选：[{\"field\":\"角色.字段\",\"onSuccess\":\"+1\"}]）。\
        只输出 JSON，结构（严格）：{\"checks\":[...]}。";
    let v = match llm_json(state, system, &input, "规则检定[生成]").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("规则检定: 生成失败，返回空: {e}");
            return Ok(json!({ "checks": [] }));
        }
    };
    Ok(normalize_rule_system(&v))
}