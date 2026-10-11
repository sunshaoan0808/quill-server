//! convert/chapters —— 章节蒸馏（locked beats/relations/exits/worldline/name inventory）
#![allow(unused_imports)]

use super::*;

/// 出口候选解析：`[{"when":..., "hint":...}]`，过滤 when 为空项。
/// 返回 `(when, hint)` 列表。
pub(crate) fn parse_beats(v: &Value, max_beats: usize, max_len: usize) -> Vec<String> {
    as_json_array(v.clone())
        .into_iter()
        .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(max_len).collect())
        .take(max_beats)
        .collect()
}

/// 世界线大事记解析：只保留 `event` 非空的条目。
pub(crate) fn parse_exit_candidates(v: &Value) -> Vec<(String, String)> {
    as_json_array(v.clone())
        .into_iter()
        .filter_map(|x| {
            let when = jstr(&x, "when", "when");
            if when.is_empty() {
                return None;
            }
            let hint = jstr(&x, "hint", "hint");
            Some((when, hint))
        })
        .take(MAX_EXITS)
        .collect()
}

/// 世界树：LLM 全景盘点世界观实体，逐实体召回证据并蒸馏 lore 条目。
///
/// - 盘点：LLM 输出实体清单（name + type + keywords + importance）；
/// - 证据：对每个实体用 `block_signature` + `cosine_top_k` 召回原文（零成本词袋，
///   不依赖 embed 服务）；
/// - 蒸馏：把召回证据交给 LLM 归纳出 3-5 句描述（只许从证据提炼）；
/// - 无召回证据的条目不落（tracing::warn），text 来自证据归纳。
pub(crate) fn parse_worldline(v: &Value) -> Vec<Value> {
    as_json_array(v.clone())
        .into_iter()
        .filter(|e| !jstr(e, "event", "event").is_empty())
        .collect()
}

/// 逐章角色关系提取（Wave C，吸收自 AI-Reader-V2 `RelationshipFact` + ChapterFact 逐章模式）。
///
/// 每章喂正文（截断到 max_chars），LLM 输出该章出现的角色关系边：
/// `[{from, to, rel, note}]`——from/to 必须是人物专名（复用 alias_merge 安全过滤防
/// 「母亲/哥哥」这类语境称呼成为关系端点），rel 为关系类型（师徒/恋人/仇敌/上下级/…），
/// note ≤40 字证据。返回与 chapters 对齐；空章/失败产出空 Vec。
/// 输出边与 Kaleido L3 MemoryL3.edges 的 `{from,to,rel,note,turn}` 结构兼容。
pub async fn distill_locked_beats(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
) -> Result<Vec<Vec<String>>, String> {
    // C6: 逐章并发化（并发度上限 4）
    let concurrency_limit = 4usize;
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency_limit));

    let futures: Vec<_> = chapters.iter().enumerate().map(|(idx, (cid, body))| {
        let state_ref = state;
        let title = title.to_string();
        let cid = cid.clone();
        let body = body.clone();
        let sem = std::sync::Arc::clone(&semaphore);
        async move {
            if body.trim().is_empty() {
                return (idx, vec![]);
            }
            let _permit = sem.acquire().await.expect("semaphore closed");
            let part: String = body.chars().take(max_chars).collect();
            let user = format!(
                "小说《{title}》\n章节：{cid}\n本章正文：\n{part}\n\
                 请按系统要求输出本章的硬节拍 JSON 数组。"
            );
            let system = "你是剧本节拍分析师。请从本章正文中提炼该章实际发生的、不可改写的事实/转折/必须呈现的场面，\
                作为硬节拍。核心约束：1) 每条 ≤40 字；2) 1-3 条；3) 只许写本章实际发生之事，禁止编造或展望。\
                只输出 JSON 数组，如 [\"节拍一\",\"节拍二\"]。";
            let v = match llm_json(state_ref, system, &user, &format!("节拍[{cid}]")).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(cid=%cid, "节拍[{cid}] 失败，该章节拍置空: {e}");
                    return (idx, vec![]);
                }
            };
            (idx, parse_beats(&v, MAX_BEATS, BEAT_MAX_LEN))
        }
    }).collect();

    let results = join_all(futures).await;
    let mut out: Vec<Vec<String>> = Vec::with_capacity(chapters.len());
    // 保序：按 idx 排序后填充
    let mut sorted_results: Vec<(usize, Vec<String>)> = results.into_iter().collect();
    sorted_results.sort_by_key(|(idx, _)| *idx);
    for (_idx, beats) in sorted_results {
        out.push(beats);
    }
    Ok(out)
}

/// 多出口：按节点对应章节喂上下文，LLM 为每个节点生成 2-3 个候选出口。
///
/// - `node_ids` 与 `chapters` 对齐（第 i 个节点对应第 i 章）；
/// - LLM 只产出 `when`（玩家选项）与 `hint`（引导一句，不含 next id）；
/// - `next` 由本函数按原拓扑分配：按出口顺序循环映射到后续节点 id；
/// - 最后节点无后续节点时回退到自身（保持可推进的闭环）。
pub async fn distill_chapter_relations(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
) -> Result<Vec<Vec<serde_json::Value>>, String> {
    let concurrency_limit = 4usize;
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency_limit));

    let futures: Vec<_> = chapters.iter().enumerate().map(|(idx, (cid, body))| {
        let state_ref = state;
        let title = title.to_string();
        let cid = cid.clone();
        let body = body.clone();
        let sem = std::sync::Arc::clone(&semaphore);
        async move {
            if body.trim().is_empty() {
                return (idx, vec![]);
            }
            let _permit = sem.acquire().await.expect("semaphore closed");
            let part: String = body.chars().take(max_chars).collect();
            let user = format!(
                "小说《{title}》\n章节：{cid}\n本章正文：\n{part}\n\
                 请按系统要求输出本章的角色关系 JSON 数组。"
            );
            // [ENT] 系统提示改：允许亲属/语境称呼端点（母亲/父亲）；"我/我们"归叙述者真名；边带 kind。
            let system = "你是小说角色关系分析师。从本章正文中提取该章明确呈现的角色关系，\
                输出 JSON 数组：[{\"from\":\"角色A\",\"to\":\"角色B\",\"rel\":\"关系类型\",\"note\":\"一句话证据\",\"kind\":\"proper\"}]。\
                核心约束：1) from/to 必须是人物（明确人物专名 或 亲属/语境称呼如\"母亲\"\"父亲\"\"哥哥\"均可），\
                严禁输出句子片段、普通名词、指代不明的碎片；\
                2) 第一人称视角下\"我/我们\"一律用叙述者真名（如主角名），不输出\"我\"；\
                3) rel 用简洁关系词（师徒/恋人/夫妻/仇敌/上下级/同事/好友/敌对/暗恋/亲属…）；\
                4) note ≤40 字，写本章实际呈现的关系证据；5) 只写本章明确出现的关系，禁止臆测；\
                6) 每条边可带 kind 字段：proper（专名）/ kin（语境称呼如母亲/父亲）；\
                7) 最多 8 条。只输出 JSON 数组，不要其他文字。";
            let v = match llm_json(state_ref, system, &user, &format!("关系[{cid}]")).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(cid=%cid, "关系[{cid}] 失败，该章关系置空: {e}");
                    return (idx, vec![]);
                }
            };
            let mut edges = Vec::new();
            if let Some(arr) = v.as_array() {
                for e in arr {
                    let from = e
                        .get("from")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim();
                    let to = e
                        .get("to")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim();
                    let rel = e
                        .get("rel")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .trim();
                    if from.is_empty() || to.is_empty() || rel.is_empty() || from == to {
                        continue;
                    }
                    // [ENT] 端点分级：Proper/Kin → 保留（Kin 打 kind=kin）；Discard → 丢弃。
                    // 不再用 is_unsafe_alias 一刀切丢弃（否则"母亲"全书高频却 0 边）。
                    use kaleido_core::entity_resolve::{classify_endpoint, EndpointKind};
                    let from_kind = classify_endpoint(from);
                    let to_kind = classify_endpoint(to);
                    if matches!(from_kind, EndpointKind::Discard)
                        || matches!(to_kind, EndpointKind::Discard)
                    {
                        continue;
                    }
                    let note = e
                        .get("note")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(40)
                        .collect::<String>();
                    // [ENT] 输出边结构 {from,to,rel,note,kind?}；kind 仅任一端为 Kin 时输出。
                    let is_kin = matches!(from_kind, EndpointKind::Kin)
                        || matches!(to_kind, EndpointKind::Kin);
                    edges.push(if is_kin {
                        serde_json::json!({
                            "from": from,
                            "to": to,
                            "rel": rel,
                            "note": note,
                            "kind": "kin",
                        })
                    } else {
                        serde_json::json!({
                            "from": from,
                            "to": to,
                            "rel": rel,
                            "note": note,
                        })
                    });
                }
            }
            (idx, edges)
        }
    }).collect();

    let results = join_all(futures).await;
    let mut sorted_results: Vec<(usize, Vec<serde_json::Value>)> = results.into_iter().collect();
    sorted_results.sort_by_key(|(idx, _)| *idx);
    Ok(sorted_results.into_iter().map(|(_, v)| v).collect())
}

/// 世界线：LLM 输出全书大事记 JSON 数组。
///
/// 仅产出结构化数据，落盘（worldline.json）由调用方负责。
pub async fn distill_exits(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    node_ids: &[String],
    max_chars: usize,
) -> Result<Vec<Vec<NodeExit>>, String> {
    // C6: 逐章并发化（并发度上限 4）
    let concurrency_limit = 4usize;
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency_limit));

    let futures: Vec<_> = chapters.iter().enumerate().map(|(i, (cid, body))| {
        let state_ref = state;
        let title = title.to_string();
        let cid = cid.clone();
        let body = body.clone();
        let node_id = node_ids.get(i).cloned().unwrap_or_else(|| format!("n{}", i + 1));
        let subsequent: Vec<String> = node_ids.get((i + 1)..).map(|s| s.to_vec()).unwrap_or_default();
        let sem = std::sync::Arc::clone(&semaphore);
        async move {
            if body.trim().is_empty() {
                return (i, vec![]);
            }
            let _permit = sem.acquire().await.expect("semaphore closed");
            let part: String = body.chars().take(max_chars).collect();
            let user = format!(
                "小说《{title}》\n当前节点：{node_id}（章节：{cid}）\n本章正文：\n{part}\n\
                 请按系统要求为玩家生成本节点结束时的候选出口 JSON 数组。"
            );
            let system = "你是剧情分支设计专家。请为当前节点生成 2-3 个玩家在本章结束时可能的选择出口。\
                核心约束：1) when 是玩家选项文案（第一人称行动，如\"决定追上去\"）；\
                2) hint 是引导下一步的一句话（不含 next 指向的节点 id）；\
                3) 出口必须源自本章内容。只输出 JSON 数组，如 [{\"when\":\"...\",\"hint\":\"...\"}]。";
            let v = match llm_json(state_ref, system, &user, &format!("多出口[{cid}]")).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(cid=%cid, "多出口[{cid}] 失败，该节点出口置空: {e}");
                    return (i, vec![]);
                }
            };
            let candidates = parse_exit_candidates(&v);
            if candidates.is_empty() {
                return (i, vec![]);
            }
            let mut exits: Vec<NodeExit> = Vec::new();
            for (j, (when, _hint)) in candidates.into_iter().enumerate() {
                let next = if subsequent.is_empty() {
                    node_id.clone()
                } else {
                    subsequent[j % subsequent.len()].clone()
                };
                exits.push(NodeExit {
                    id: format!("e{}-{}", i + 1, j),
                    when,
                    next,
                });
            }
            (i, exits)
        }
    }).collect();

    let results = join_all(futures).await;
    let mut out: Vec<Vec<NodeExit>> = Vec::with_capacity(chapters.len());
    let mut sorted_results: Vec<(usize, Vec<NodeExit>)> = results.into_iter().collect();
    sorted_results.sort_by_key(|(idx, _)| *idx);
    for (_idx, exits) in sorted_results {
        out.push(exits);
    }
    Ok(out)
}

/// 盘点小说关键实体与人物名（供事件包 prompt 注入真实世界元素）。
///
/// 失败时仅 `tracing::warn!` 并返回空数组（不影响主流程），单条降级。
pub async fn distill_worldline(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
) -> Result<Vec<Value>, String> {
    let input = build_roster_input(title, chapters, max_chars);
    let system = "你是小说大事记整理专家。请从给定小说正文中梳理全书关键事件的时间线，\
        只输出 JSON 数组，每个元素为 {\"event\":\"事件描述\",\"time_point\":\"时间/阶段\",\
        \"characters\":[\"涉及角色\"],\"importance\":\"high|medium|low\",\"chapter\":\"ch01\"}。\
        核心约束：只写正文实际发生的事件；chapter 用章节代号（如 ch01）。只输出 JSON。";
    let v = match llm_json(state, system, &input, "世界线[大事记]").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("世界线: 失败，返回空: {e}");
            return Ok(vec![]);
        }
    };
    Ok(parse_worldline(&v))
}

// ─── 素材库蒸馏：事件包 / 演员状态 / 文风 / 规则检定 ─────────────────────────

/// 事件包解析（纯函数）：过滤非法卡/包，返回结构化 `EventPackage` 列表。
///
/// - 卡：id/title/prompt 缺一即跳过该卡；weight 缺省/为 0 时按 1；enabled 缺省 true；
///   once_per_session 兼容 snake/camel 两种键名。
/// - 包：id/name 缺一跳过；整包卡全非法则跳过该包；包数上限 [`MAX_EVENT_PACKAGES`]。
pub(crate) async fn distill_name_inventory(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
) -> Vec<String> {
    let input = build_roster_input(title, chapters, max_chars);
    let system = "你是小说实体盘点助手。请从给定小说正文中盘点关键人物名与地名/势力/境界/物品/组织名，\
        只输出 JSON 数组，每个元素为一个名字字符串（如\"江景离\"、\"青云宗\"）。只输出 JSON，最多 30 个。";
    let v = match llm_json(state, system, &input, "事件包[实体盘点]").await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("事件包: 实体盘点失败，跳过实体注入: {e}");
            return vec![];
        }
    };
    as_json_array(v)
        .into_iter()
        .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .take(30)
        .collect()
}