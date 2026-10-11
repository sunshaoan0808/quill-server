//! crawler/parts/p003/stages —— 书架世界蒸馏的阶段函数（角色/世界/节拍/出口/世界线）。
//! [R6-2① 2026-10-07] 自 exec_shelf_distil_world 按阶段拆出；共享上下文经 StageCtx。
#![allow(unused_imports)]

use super::*;
use kaleido_core::StoryPack;
use serde_json::Value;

/// 阶段共享上下文（exec 装配一次，逐阶段传递）。
pub(crate) struct StageCtx<'a> {
    pub(crate) state: &'a AppState,
    pub(crate) jobs: &'a kaleido_core::JobStore,
    pub(crate) run_id: &'a str,
    pub(crate) title: &'a str,
    pub(crate) distil_chapters: &'a [(String, String)],
    pub(crate) max_chars: usize,
}

/// 阶段失败（exec 统一 fail_job 收尾）。
pub(crate) struct StageErr {
    pub(crate) stage: &'static str,
    pub(crate) e: String,
    pub(crate) p: f64,
}

pub(crate) fn prog(ctx: &StageCtx<'_>, msg: &str, p: f64) {
    let _ = ctx.jobs.push_event(
        ctx.run_id,
        JobEvent::progress(msg.to_string(), p),
        Some(p),
        None,
    );
}

/// 阶段 1：角色蒸馏（指定名单 or 全量）+ A1 质检 + 空壳过滤 + present 重建。
pub(crate) async fn stage_characters(
    ctx: &StageCtx<'_>,
    pack: &mut StoryPack,
    bodies: &[(String, String)],
    named_chars: &[String],
    chars_done: bool,
    missing_key_chars: &mut Vec<String>,
) -> Result<(), StageErr> {
    let _ = bodies;
    if !named_chars.is_empty() {
        // 指定角色蒸馏：跳过角色谱与 chars_done 门，只蒸馏名单角色并合并进 pack
        prog(ctx, "指定角色蒸馏中…", 0.15);
        match crate::convert::distill_named_characters(
            ctx.state,
            ctx.title,
            ctx.distil_chapters,
            &named_chars,
            Some(&pack.id),
        )
        .await
        {
            Ok(c) => {
                // A1-static(精简): 指定产物必填字段完整性检查。
                // 不合格(空壳/薄卡)直接丢弃、不入 pack——避免空壳卡覆盖既有同名好卡,
                // 也避免 importance=high 的空壳卡堵塞 chars_done 门导致无法自动重蒸。
                let mut c_valid: Vec<_> = Vec::with_capacity(c.len());
                for ch in c {
                    let mut missing_fields: Vec<&str> = Vec::new();
                    if ch.name.trim().is_empty() { missing_fields.push("name"); }
                    if ch.role.trim().is_empty() { missing_fields.push("role"); }
                    if ch.personality.trim().is_empty() { missing_fields.push("personality"); }
                    if ch.speech_style.trim().is_empty() { missing_fields.push("speech_style"); }
                    if ch.motivation.trim().is_empty() { missing_fields.push("motivation"); }
                    if !missing_fields.is_empty() {
                        tracing::warn!(job=%ctx.run_id, char=%ch.name, missing=?missing_fields, "A1-static: 指定角色蒸馏必填字段缺失，该卡丢弃(不入 pack)");
                    } else {
                        c_valid.push(ch);
                    }
                }
                if c_valid.is_empty() {
                    tracing::warn!(job=%ctx.run_id, "指定角色蒸馏: 名单角色全部失败或无证据");
                }
                // 合并回工作副本：同名替换、其余追加（仅合格卡）
                for ch in &c_valid {
                    pack.characters.retain(|x| x.name != ch.name);
                }
                pack.characters.extend(c_valid);
                remap_node_present_to_distil(&mut pack.nodes, &pack.characters, &bodies);
                prog(ctx, "指定角色蒸馏完成", 0.15);
                if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
                    let _ = ctx.jobs.push_event(
                        ctx.run_id,
                        JobEvent::error(format!("named_characters_save: {e}")),
                        Some(0.15),
                        None,
                    );
                }
            }
            Err(e) => {
                return Err(StageErr { stage: "characters", e: e.to_string(), p: 0.15 });
            }
        }
    } else if chars_done {
        prog(ctx, "角色已完成，跳过", 0.15);
    } else {
        prog(ctx, "角色蒸馏中…", 0.15);
        match crate::convert::distill_pack_characters(ctx.state, ctx.title, ctx.distil_chapters, ctx.max_chars, Some(&pack.id))
            .await
        {
            Ok(c) => {
                // A1: 主角必在断言 — 角色蒸馏完成后检查关键角色是否在场
                // 判定依据:distill 产出的 PackCharacterRef.importance(high/medium/low)
                // 由角色谱写入;若蒸馏结果缺失全部 high 角色,记 missing 供人工检核。
                // 扩展(吸收 TavernWeave validation 方法论):
                //   A1-static   — 模块契约字段完整性(必填字段非空;未知值仅允许 gender/appearance/opening_*)
                //   A1-evidence — personality/beliefs/mental_models 结论带 evidence_refs(缺失→warning)
                //   A1-cover    — 角色谱 high 名单全部覆盖(roster_high_covered,已在 chars_done 判定)
                //   A1-fffd     — 产物无 U+FFFD(convert 重试层已处理,此处兜底检查)
                let high_count = c.iter().filter(|ch| {
                    let imp = ch.importance.to_lowercase();
                    let r = ch.role.to_lowercase();
                    imp == "high" || r.contains("protagonist") || r.contains("主角")
                }).count();
                let non_narrator_count = c.iter().filter(|ch| {
                    let r = ch.role.to_lowercase();
                    !r.contains("narrator") && !r.contains("旁白") && !r.contains("读者") && !r.is_empty()
                }).count();
                // A1-static: 必填字段完整性检查
                let mut static_violations: Vec<String> = Vec::new();
                for ch in &c {
                    let mut missing_fields: Vec<&str> = Vec::new();
                    if ch.name.trim().is_empty() { missing_fields.push("name"); }
                    if ch.role.trim().is_empty() { missing_fields.push("role"); }
                    if ch.personality.trim().is_empty() { missing_fields.push("personality"); }
                    if ch.speech_style.trim().is_empty() { missing_fields.push("speech_style"); }
                    if ch.motivation.trim().is_empty() { missing_fields.push("motivation"); }
                    if !missing_fields.is_empty() {
                        static_violations.push(format!("{}:{}", ch.name, missing_fields.join(",")));
                    }
                }
                if !static_violations.is_empty() {
                    tracing::warn!(
                        violations = ?static_violations,
                        "A1-static: 角色卡必填字段缺失"
                    );
                }
                // A1-evidence: 认知字段结论带证据引用检查
                let mut evidenceless: Vec<String> = Vec::new();
                for ch in &c {
                    let has_refs = !ch.evidence_refs.is_empty();
                    let has_cog = !ch.mental_models.is_empty() || !ch.decision_heuristics.is_empty() || !ch.beliefs.is_empty();
                    if has_cog && !has_refs {
                        evidenceless.push(ch.name.clone());
                    }
                }
                if !evidenceless.is_empty() {
                    tracing::warn!(
                        chars = ?evidenceless,
                        "A1-evidence: 认知结论无 evidence_refs，需人工检核"
                    );
                }
                // A1-fffd: 兜底检查产物无损坏字符
                let fffd_chars: Vec<String> = c.iter()
                    .filter(|ch| {
                        ch.name.contains('\u{fffd}')
                            || ch.personality.contains('\u{fffd}')
                            || ch.motivation.contains('\u{fffd}')
                    })
                    .map(|ch| ch.name.clone())
                    .collect();
                if !fffd_chars.is_empty() {
                    tracing::warn!(
                        chars = ?fffd_chars,
                        "A1-fffd: 产物含 U+FFFD 损坏字符，需人工检核"
                    );
                }
                if high_count == 0 {
                    if c.len() > 0 {
                        // 角色谱标记了 high 但蒸馏结果无 high:全部列入疑似缺失(供人工检核)
                        for ch in &c {
                            missing_key_chars.push(format!("{}(high?)", ch.name));
                        }
                    } else {
                        missing_key_chars.push("(无角色)".to_string());
                    }
                }
                if !missing_key_chars.is_empty() {
                    tracing::warn!(
                        missing = ?missing_key_chars,
                        total = c.len(),
                        high_count, non_narrator_count,
                        static_violations = ?static_violations,
                        evidenceless = ?evidenceless,
                        "角色蒸馏完成，但关键角色可能缺失，需人工检核"
                    );
                }
                // [fix] 空壳/薄卡守卫: 缺核心字段(name/personality/speech_style/motivation)
                // 的卡在整体替换前过滤掉——空壳卡会污染 pack, 且 importance=high 的空壳
                // 会堵塞 chars_done 门, 导致角色再也无法被自动重蒸。
                let c_before = c.len();
                let mut c_valid: Vec<_> = Vec::with_capacity(c.len());
                let mut dropped_shells: Vec<String> = Vec::new();
                for ch in c {
                    if ch.name.trim().is_empty()
                        || ch.personality.trim().is_empty()
                        || ch.speech_style.trim().is_empty()
                        || ch.motivation.trim().is_empty()
                    {
                        dropped_shells.push(format!(
                            "{}({})",
                            if ch.name.trim().is_empty() { "(无名)" } else { ch.name.as_str() },
                            ch.importance
                        ));
                    } else {
                        c_valid.push(ch);
                    }
                }
                if !dropped_shells.is_empty() {
                    tracing::warn!(
                        job=%ctx.run_id,
                        dropped = ?dropped_shells,
                        before = c_before, after = c_valid.len(),
                        "角色蒸馏: 空壳/薄卡被过滤, 不入 pack(防止污染 chars_done 门)"
                    );
                }
                pack.characters = c_valid;
                // [fix] 蒸馏后把 nodes.present 从 c-cast 垃圾占位重建为
                // 每章正文真实出现的蒸馏角色 id，使 build_mainline_opening
                // 能按 present 命中带 opening 的核心角色。
                remap_node_present_to_distil(&mut pack.nodes, &pack.characters, &bodies);
                prog(ctx, "角色蒸馏完成", 0.15);
                if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
                    let _ = ctx.jobs.push_event(
                        ctx.run_id,
                        JobEvent::error(format!("characters_save: {e}")),
                        Some(0.15),
                        None,
                    );
                }
            }
            Err(e) => {
                return Err(StageErr { stage: "characters", e: e.to_string(), p: 0.15 });
            }
        }
    }
    Ok(())
}

/// 阶段 2：世界树 → pack.lore_entries。
pub(crate) async fn stage_world(
    ctx: &StageCtx<'_>,
    pack: &mut StoryPack,
    bodies: &[(String, String)],
    world_done: bool,
) -> Result<(), StageErr> {
    if world_done {
        prog(ctx, "世界树/世界书已完成，跳过", 0.40);
    } else {
        prog(ctx, "生成世界树/世界书…", 0.40);
        match crate::convert::distill_world_lore(ctx.state, ctx.title, ctx.distil_chapters, ctx.max_chars).await {
            Ok(v) => {
                pack.lore_entries = v;
                prog(ctx, "世界树/世界书完成", 0.40);
                if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
                    let _ = ctx.jobs.push_event(
                        ctx.run_id,
                        JobEvent::error(format!("world_save: {e}")),
                        Some(0.40),
                        None,
                    );
                }
            }
            Err(e) => {
                return Err(StageErr { stage: "world_lore", e: e.to_string(), p: 0.40 });
            }
        }
    }
    Ok(())
}

/// 阶段 3：每章节拍 → nodes[].locked_beats。
pub(crate) async fn stage_beats(
    ctx: &StageCtx<'_>,
    pack: &mut StoryPack,
    bodies: &[(String, String)],
    beats_done: bool,
) -> Result<(), StageErr> {
    if beats_done {
        prog(ctx, "节拍已完成，跳过", 0.55);
    } else {
        prog(ctx, "生成节拍…", 0.55);
        let beats =
            match crate::convert::distill_locked_beats(ctx.state, ctx.title, ctx.distil_chapters, ctx.max_chars)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    return Err(StageErr { stage: "locked_beats", e: e.to_string(), p: 0.55 });
                }
            };
        for (i, node) in pack.nodes.iter_mut().enumerate() {
            if let Some(b) = beats.get(i) {
                node.locked_beats = b.clone();
            }
        }
        prog(ctx, "节拍完成", 0.55);
        if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
            let _ = ctx.jobs.push_event(
                ctx.run_id,
                JobEvent::error(format!("beats_save: {e}")),
                Some(0.55),
                None,
            );
        }
    }
    Ok(())
}

/// 阶段 4：多出口 → nodes[].exit（单链 continue 替换，已有出口补齐）。
pub(crate) async fn stage_exits(
    ctx: &StageCtx<'_>,
    pack: &mut StoryPack,
    bodies: &[(String, String)],
    exits_done: bool,
) -> Result<(), StageErr> {
    if exits_done {
        prog(ctx, "多出口已完成，跳过", 0.65);
    } else {
        prog(ctx, "生成多出口…", 0.65);
        let node_ids: Vec<String> = pack.nodes.iter().map(|n| n.id.clone()).collect();
        let exits = match crate::convert::distill_exits(
            ctx.state,
            ctx.title,
            ctx.distil_chapters,
            &node_ids,
            ctx.max_chars,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                return Err(StageErr { stage: "exits", e: e.to_string(), p: 0.65 });
            }
        };
        for (i, node) in pack.nodes.iter_mut().enumerate() {
            if let Some(new_exits) = exits.get(i) {
                if !new_exits.is_empty() {
                    let is_single_continue = node.exit.len() == 1 && node.exit[0].when == "继续";
                    if is_single_continue || node.exit.is_empty() {
                        node.exit = new_exits.clone();
                    } else {
                        let existing: Vec<String> = node.exit.iter().map(|e| e.when.clone()).collect();
                        for ne in new_exits {
                            if !existing.contains(&ne.when) {
                                node.exit.push(ne.clone());
                            }
                        }
                    }
                }
            }
        }
        prog(ctx, "多出口完成", 0.65);
        if let Err(e) = save_pack_checkpoint(ctx.state, &pack, &bodies) {
            let _ = ctx.jobs.push_event(
                ctx.run_id,
                JobEvent::error(format!("exits_save: {e}")),
                Some(0.65),
                None,
            );
        }
    }
    Ok(())
}

/// 阶段 5：世界线 → worldline.json（旁挂文件，不进 pack.json）。
pub(crate) async fn stage_worldline(
    ctx: &StageCtx<'_>,
    pack: &StoryPack,
    worldline_done: bool,
    worldline: &mut Value,
) -> Result<(), StageErr> {
    if worldline_done {
        prog(ctx, "世界线已完成，跳过", 0.75);
    } else {
        prog(ctx, "生成世界线…", 0.75);
        let wl = match crate::convert::distill_worldline(ctx.state, ctx.title, ctx.distil_chapters, ctx.max_chars)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                return Err(StageErr { stage: "worldline", e: e.to_string(), p: 0.75 });
            }
        };
        *worldline = serde_json::Value::Array(wl);
        let pack_dir = ctx.state
            .app_state
            .data_root()
            .story_packs_dir()
            .join(&pack.id);
        if let Err(e) = write_worldline_atomic(&pack_dir, &worldline) {
            return Err(StageErr { stage: "worldline_write", e: e.to_string(), p: 0.75 });
        }
        prog(ctx, "世界线完成", 0.75);
    }
    Ok(())
}
