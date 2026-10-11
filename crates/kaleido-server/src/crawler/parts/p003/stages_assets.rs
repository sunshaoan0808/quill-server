//! crawler/parts/p003/stages_assets —— 关系图谱 + 素材库蒸馏阶段（事件包/演员状态/文风规则）。
//! [R6-2① 2026-10-07] 自 exec_shelf_distil_world 按阶段拆出。
#![allow(unused_imports)]

use super::*;
use super::stages::{prog, StageCtx};
use kaleido_core::StoryPack;
use serde_json::Value;

/// 阶段 5.5：角色关系图谱 → relations.json + entities.json + graph.sqlite 桥 +
/// 角色卡 relationships 回填。独立失败仅 warn，不中止整批。
pub(crate) async fn stage_relations(
    ctx: &StageCtx<'_>,
    pack: &mut StoryPack,
    bodies: &[(String, String)],
    force_relations: bool,
) {
    // 5.5) 角色关系图谱 → relations.json（Wave C，吸收自 AI-Reader-V2 RelationshipFact）
    //      逐章提取 {from,to,rel,note} 边，旁挂文件 + 回填角色卡 relationships。
    let relations_done = std::fs::metadata(
        ctx.state
            .app_state
            .data_root()
            .story_packs_dir()
            .join(&pack.id)
            .join("relations.json"),
    )
    .map(|_| true)
    .unwrap_or(false);
    if relations_done && !force_relations {
        prog(ctx, "关系图谱已完成，跳过", 0.78);
    } else {
        prog(ctx, "生成角色关系图谱…", 0.78);
        match crate::convert::distill_chapter_relations(ctx.state, ctx.title, ctx.distil_chapters, ctx.max_chars)
            .await
        {
            Ok(chapter_edges) => {
                // 聚合去重：同一 (from,to,rel) 只保留首见 note；跨章同对关系合并 rel 集；
                // [L0] 顺带聚合 chapters 章标题数组（写 graph 演化用）。
                let mut seen: std::collections::HashMap<(String, String), serde_json::Value> =
                    std::collections::HashMap::new();
                for (ci, edges) in chapter_edges.iter().enumerate() {
                    let cid = ctx.distil_chapters
                        .get(ci)
                        .map(|(c, _)| c.clone())
                        .unwrap_or_default();
                    for e in edges {
                        let from = e.get("from").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let to = e.get("to").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let rel = e.get("rel").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let key = (format!("{from}::{to}"), rel.clone());
                        let note = e.get("note").and_then(|x| x.as_str()).unwrap_or("");
                        let entry = seen.entry(key).or_insert_with(|| {
                            let mut chs = serde_json::json!([]);
                            if !cid.is_empty() {
                                chs.as_array_mut()
                                    .unwrap()
                                    .push(serde_json::Value::String(cid.clone()));
                            }
                            serde_json::json!({ "from": from, "to": to, "rel": rel, "note": note, "chapters": chs })
                        });
                        if let Some(o) = entry.as_object_mut() {
                            if !cid.is_empty() {
                                let chs = o.entry("chapters").or_insert_with(|| serde_json::json!([]));
                                if let Some(arr) = chs.as_array_mut() {
                                    if !arr.iter().any(|x| x.as_str() == Some(cid.as_str())) {
                                        arr.push(serde_json::Value::String(cid.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
                // [ENT] mut：后续要写入 from_id/to_id/kind 字段。
                let mut edges: Vec<serde_json::Value> = seen.into_values().collect();
                if edges.is_empty() {
                    tracing::warn!("关系图谱蒸馏: 结果为空，跳过关系字段");
                } else {
                    // 旁挂 relations.json（与 worldline.json 同模式）
                    let pack_dir = ctx.state
                        .app_state
                        .data_root()
                        .story_packs_dir()
                        .join(&pack.id);
                    // [ENT] 实体解析层：先把每条边端点解析到具体角色卡 id（from_id/to_id），
                    // 再做幽灵对账 / 稀疏告警 / 角色卡回填。
                    let cards: Vec<(String, String, String, String)> = pack
                        .characters
                        .iter()
                        .map(|c| {
                            (
                                c.id.clone(),
                                c.name.clone(),
                                c.role.clone(),
                                c.importance.clone(),
                            )
                        })
                        .collect();
                    use kaleido_core::entity_resolve::{
                        classify_endpoint, collect_ghosts, find_sparse_characters,
                        resolve_entity_endpoint, EndpointKind,
                    };
                    // [ENT] BTreeMap：id 排序迭代，entities.json 输出确定性稳定。
                    let mut entity_alias_index: std::collections::BTreeMap<
                        String,
                        std::collections::BTreeSet<String>,
                    > = std::collections::BTreeMap::new();
                    // [ENT] 每条边：解析 from_id/to_id 并写入边（None 时序列化跳过）。
                    for e in edges.iter_mut() {
                        let from = e.get("from").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let to = e.get("to").and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let from_id = resolve_entity_endpoint(&from, &cards);
                        let to_id = resolve_entity_endpoint(&to, &cards);
                        if let Some(id) = &from_id {
                            if let Some(o) = e.as_object_mut() {
                                o.insert("from_id".into(), serde_json::Value::String(id.clone()));
                            }
                            entity_alias_index.entry(id.clone()).or_default().insert(from.clone());
                        }
                        if let Some(id) = &to_id {
                            if let Some(o) = e.as_object_mut() {
                                o.insert("to_id".into(), serde_json::Value::String(id.clone()));
                            }
                            entity_alias_index.entry(id.clone()).or_default().insert(to.clone());
                        }
                    }
                    // [ENT] id → 卡名 映射（回填 other 用显示身份，避免裸"我"上卡）。
                    let id_to_name: std::collections::HashMap<String, String> = pack
                        .characters
                        .iter()
                        .map(|c| (c.id.clone(), c.name.clone()))
                        .collect();
                    // [ENT] 幽灵清单：解析不出卡 id 的端点 → 写 entities.json + warn。
                    let ghosts = collect_ghosts(&edges, &cards);
                    if !ghosts.is_empty() {
                        let pack_title = pack.title.clone();
                        let entities: Vec<serde_json::Value> = {
                            let mut list = Vec::new();
                            for (id, aliases) in &entity_alias_index {
                                let canonical_name = pack
                                    .characters
                                    .iter()
                                    .find(|c| &c.id == id)
                                    .map(|c| c.name.clone())
                                    .unwrap_or_default();
                                // [ENT] 实体 kind：按 canonical 名端点分级（母亲→kin，夏文嘉→proper）。
                                let kind = match classify_endpoint(&canonical_name) {
                                    EndpointKind::Kin => "kin",
                                    _ => "proper",
                                };
                                list.push(serde_json::json!({
                                    "id": id,
                                    "canonicalName": canonical_name,
                                    "aliases": aliases.iter().cloned().collect::<Vec<_>>(),
                                    "kind": kind,
                                }));
                            }
                            list
                        };
                        // {schemaVersion, packTitle, narratorAliases, entities, ghosts}
                        let entities_doc = serde_json::json!({
                            "schemaVersion": 1,
                            "packTitle": pack_title,
                            "narratorAliases": ["我", "我们", "叙述者", "主角"],
                            "entities": entities,
                            "ghosts": ghosts,
                        });
                        if let Err(e) = std::fs::write(
                            pack_dir.join("entities.json"),
                            serde_json::to_string_pretty(&entities_doc).unwrap_or_default(),
                        ) {
                            tracing::warn!("entities.json 写入失败: {e}");
                        }
                        tracing::warn!(ghosts = ?ghosts, "关系图谱幽灵端点: 无对应角色卡");
                    }
                    // [ENT] 稀疏告警：有卡但未出现在任何边端点。
                    let sparse = find_sparse_characters(&cards, &edges);
                    if !sparse.is_empty() {
                        tracing::warn!(chars = ?sparse, "关系稀疏告警: 以下角色未出现在任何关系边");
                    }
                    if let Err(e) = std::fs::write(
                        pack_dir.join("relations.json"),
                        serde_json::to_string_pretty(&edges).unwrap_or_default(),
                    ) {
                        tracing::warn!("关系图谱写入失败: {e}");
                    } else {
                        // [ENT] 回填角色卡 relationships：优先按 from_id/to_id 匹配（避免
                        // "我→陈妹妹"这类回填到错误卡），无 id 时退回 name 匹配（旧兼容）。
                        // other 一方用解析出的 id 映射到 canonical 卡名（不显示裸"我"）。
                        for c in pack.characters.iter_mut() {
                            let mut rels: Vec<String> = c.relationships.clone();
                            for e in &edges {
                                let from = e.get("from").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let to = e.get("to").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let rel = e.get("rel").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                let from_id = e.get("from_id").and_then(|x| x.as_str()).unwrap_or("");
                                let to_id = e.get("to_id").and_then(|x| x.as_str()).unwrap_or("");
                                // 当前卡命中（id 优先；无 id 时 name 兜底）
                                let self_is_from = from_id == c.id || (from_id.is_empty() && from == c.name);
                                let self_is_to = to_id == c.id || (to_id.is_empty() && to == c.name);
                                if !(self_is_from || self_is_to) {
                                    continue;
                                }
                                // other 一方：优先用解析 id → canonical 卡名，否则原始串。
                                let (other_raw, other_id) = if self_is_from {
                                    (to.clone(), to_id)
                                } else {
                                    (from.clone(), from_id)
                                };
                                let other = id_to_name
                                    .get(other_id)
                                    .cloned()
                                    .unwrap_or(other_raw);
                                let s = format!("{other}（{rel}）");
                                if !rels.contains(&s) {
                                    rels.push(s);
                                }
                            }
                            c.relationships = rels;
                        }
                        if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
                            let _ = ctx.jobs.push_event(
                                ctx.run_id,
                                JobEvent::error(format!("relations_save: {e}")),
                                Some(0.78),
                                None,
                            );
                        }
                    }
                }
                // [L0] 蒸馏→graph.sqlite 桥：把解析出实体 id 的边写入运行时图谱。
                // 运行时 story_tavern 只读 graph.sqlite；不落库则蒸馏关系对运行时不可见
                // （宿醉此前即如此：蒸馏 4 边，graph.sqlite 零记录，注入区块为空）。
                // 幽灵端点（无卡 id）跳过：不为"我/陈妹妹"这类建幽灵角色卡。
                // 注：内层块已构建过 id_to_name（仅供回填），此处重建一份传给桥。
                let id_to_name: std::collections::HashMap<String, String> = pack
                    .characters
                    .iter()
                    .map(|c| (c.id.clone(), c.name.clone()))
                    .collect();
                if let Err(e) = bridge_relations_to_graph(&ctx.state.graph, &pack.id, &edges, &id_to_name)
                {
                    tracing::warn!("关系图谱→graph.sqlite 桥失败（不影响旁挂文件）: {e}");
                }
                prog(ctx, "关系图谱完成", 0.78);
            }
            Err(e) => {
                tracing::warn!("关系图谱蒸馏: 失败，跳过该字段: {e}");
            }
        }
    }
}


/// 阶段 6a：素材库 · 事件包。
pub(crate) async fn stage_events(
    ctx: &StageCtx<'_>,
    pack: &mut StoryPack,
    bodies: &[(String, String)],
    events_done: bool,
) {
    // 6) 素材库蒸馏：事件包 / 演员状态 / 文风 / 规则检定
    //    每个字段独立失败，仅 warn + 跳过该字段，绝不让单条 LLM 失败中止整批。
    if events_done {
        prog(ctx, "事件包已完成，跳过", 0.85);
    } else {
        prog(ctx, "素材库：事件包…", 0.85);
        let event_packages = match crate::convert::distill_event_packages(
            ctx.state,
            ctx.title,
            ctx.distil_chapters,
            ctx.max_chars,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("素材库蒸馏[事件包]: 失败，跳过该字段: {e}");
                vec![]
            }
        };
        if event_packages.is_empty() {
            tracing::warn!("素材库蒸馏[事件包]: 结果为空，跳过事件包字段");
        } else {
            pack.event_packages = event_packages.clone();
            pack.stage_director.modules.event_package_ids = event_packages
                .iter()
                .filter(|p| p.enabled)
                .map(|p| p.id.clone())
                .collect();
        }
        prog(ctx, "事件包完成", 0.85);
        if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
            let _ = ctx.jobs.push_event(
                ctx.run_id,
                JobEvent::error(format!("events_save: {e}")),
                Some(0.85),
                None,
            );
        }
    }
}


/// 阶段 6b：素材库 · 演员状态。
pub(crate) async fn stage_actors(
    ctx: &StageCtx<'_>,
    pack: &mut StoryPack,
    bodies: &[(String, String)],
    actors_done: bool,
) {
    if actors_done {
        prog(ctx, "演员状态已完成，跳过", 0.92);
    } else {
        prog(ctx, "演员状态…", 0.92);
        let actor_state = match crate::convert::distill_actor_state(
            ctx.state,
            ctx.title,
            &pack.characters,
            ctx.distil_chapters,
            ctx.max_chars,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("素材库蒸馏[演员状态]: 失败，跳过该字段: {e}");
                kaleido_core::ActorStatePackConfig::default()
            }
        };
        if actor_state.templates.is_empty() {
            tracing::warn!("素材库蒸馏[演员状态]: 模板为空，跳过 actor_state_config");
        } else {
            pack.actor_state_config = actor_state.clone();
        }
        prog(ctx, "演员状态完成", 0.92);
        if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
            let _ = ctx.jobs.push_event(
                ctx.run_id,
                JobEvent::error(format!("actors_save: {e}")),
                Some(0.92),
                None,
            );
        }
    }
}


/// 阶段 6c：素材库 · 文风与规则检定。
pub(crate) async fn stage_style(
    ctx: &StageCtx<'_>,
    pack: &mut StoryPack,
    bodies: &[(String, String)],
    style_done: bool,
) {
    if style_done {
        prog(ctx, "文风与规则检定已完成，跳过", 0.97);
    } else {
        prog(ctx, "文风与规则检定…", 0.97);
        let narrative_style = match crate::convert::distill_narrative_style(
            ctx.state,
            ctx.title,
            ctx.distil_chapters,
            ctx.max_chars,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("素材库蒸馏[文风]: 失败，跳过该字段: {e}");
                json!({})
            }
        };
        let has_narrative_style = narrative_style
            .as_object()
            .map(|o| !o.is_empty())
            .unwrap_or(false);
        if !has_narrative_style {
            tracing::warn!("素材库蒸馏[文风]: 结果为空，跳过 narrative_style");
        }

        let rule_system = match crate::convert::distill_rule_system(
            ctx.state,
            ctx.title,
            ctx.distil_chapters,
            ctx.max_chars,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("素材库蒸馏[规则检定]: 失败，跳过该字段: {e}");
                json!({ "checks": [] })
            }
        };
        let rule_check_count = rule_system
            .get("checks")
            .and_then(|c| c.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if rule_check_count == 0 {
            tracing::warn!("素材库蒸馏[规则检定]: 结果为空，跳过 rule_system");
        }

        if has_narrative_style || rule_check_count > 0 {
            let snapshot = pack
                .stage_director
                .resolved_snapshot
                .get_or_insert_with(Default::default);
            if has_narrative_style {
                snapshot.narrative_style = Some(narrative_style);
            }
            if rule_check_count > 0 {
                snapshot.rule_system = Some(rule_system);
            }
        }
        prog(ctx, "文风与规则检定完成", 0.97);
        if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
            let _ = ctx.jobs.push_event(
                ctx.run_id,
                JobEvent::error(format!("style_save: {e}")),
                Some(0.97),
                None,
            );
        }
    }
}
