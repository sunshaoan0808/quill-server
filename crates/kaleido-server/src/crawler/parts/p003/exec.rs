//! crawler/parts/p003/exec —— 书架世界蒸馏执行器：setup + 阶段调度 + finalize。
//! [R6-2① 2026-10-07] 自 1067 行单函数拆出；阶段函数见 stages.rs / stages_assets.rs。
#![allow(unused_imports)]

use super::*;
use super::stages::{prog, StageCtx, StageErr};

pub async fn exec_shelf_distil_world(
    state: AppState,
    run_id: String,
    slug: String,
    title: String,
    resume_meta: Option<Value>,
) {
    let jobs = state.jobs.clone();
    let run_id_job = run_id.clone();
    let progress = |msg: &str, p: f64| {
        let _ = jobs.push_event(
            &run_id_job,
            JobEvent::progress(msg.to_string(), p),
            Some(p),
            None,
        );
    };
    let fail_job = |stage: &str, e: &str, p: f64| {
        let _ = jobs.push_event(
            &run_id_job,
            JobEvent::error(format!("{stage}: {e}")),
            Some(p),
            None,
        );
        let _ = jobs.complete(&run_id_job, "failed", None, Some(format!("{stage}: {e}")));
    };

    // ── 与 handler 相同的 shelf md 解析（恢复续跑时独立重算）──
    let shelf = scan_shelf();
    let entry = match shelf.into_iter().find(|e| e.slug == slug) {
        Some(e) => e,
        None => {
            fail_job("shelf", "novel not found", 0.0);
            return;
        }
    };
    let dir = shelf_dir();
    let slug_tag = shelf_slug(&entry.title);
    let file_path = find_shelf_md_file(&dir, &slug_tag, &entry.title);
    let content = match file_path.and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(c) => c,
        None => {
            fail_job("shelf", "file not found", 0.0);
            return;
        }
    };
    let chapters = split_novel_chapters(&content);
    // A0 完整版 (2026-08-19): 编号用原著章号（缺章自动跳号），与 pack.chapters.id 对齐（D1）。
    let chapter_ids = chapter_id_seq(&chapters);
    let distil_chapters: Vec<(String, String)> = chapters
        .iter()
        .enumerate()
        .map(|(i, (_t, b))| (chapter_ids[i].clone(), b.clone()))
        .collect();
    let max_chars = 6000usize;

    // ── 工作副本：磁盘已有 pack → 复用之；否则新建基底 ──
    let pid = find_existing_pack_for_shelf(&state.packs, &slug, &entry.title).unwrap_or_else(|| {
        format!(
            "pack-shelf-{}-{}",
            slug.chars().take(24).collect::<String>(),
            chrono::Utc::now().timestamp() % 100_000
        )
    });
    let (fresh_pack, bodies) = build_pack_from_chapters(&entry.title, &chapters, &pid, &slug);
    let disk_pack = state.packs.get(&pid).ok();
    let mut pack = disk_pack.clone().unwrap_or_else(|| fresh_pack.clone());
    if let Some(meta) = &resume_meta {
        tracing::info!(job=%run_id, resume_meta=%meta, "shelf distil world resume");
    }
    // 指定角色名单（可选）：非空时跳过角色谱，只按名单蒸馏并合并进 pack
    let named_chars: Vec<String> = resume_meta
        .as_ref()
        .and_then(|m| m.get("characters").cloned())
        .and_then(|v| match v {
            serde_json::Value::Array(a) => Some(a),
            _ => None,
        })
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // 各阶段完成推断（读磁盘产物，幂等续跑）
    // 角色阶段完成判定:存在"非默认蒸馏卡" 且 其中至少一个 importance=high(主角在场)。
    // 修复:旧版 only-any 判定下,只要有任意 1 张旧卡就跳过蒸馏,导致主角(如庄眉)缺失时
    // 重跑永远不触发。现在要求 high 主角在场才视为完成,否则重蒸馏。
    let has_distilled = pack
        .characters
        .iter()
        .any(|c| {
            !c.id.starts_with("c-narrator")
                && !c.id.starts_with("c-player")
                && !c.id.starts_with("c-cast-")
        });
    let has_high_char = pack
        .characters
        .iter()
        .any(|c| {
            let imp = c.importance.to_lowercase();
            let r = c.role.to_lowercase();
            imp == "high" || r.contains("protagonist") || r.contains("主角")
        });
    // 全部 high 角色在场判定: 与角色谱(roster-diag.json)的 high 名单比对,
    // 缺任何一个 high(如庄眉)都必须重蒸馏,不能只凭"有 high"就跳过。
    // roster-diag.json 由 distill_pack_characters 每次角色谱阶段落盘（pack 目录隔离）。
    let roster_high_covered = state
        .packs
        .pack_dir(&pack.id)
        .ok()
        .map(|d| d.join("roster-diag.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("candidates").cloned())
        .and_then(|cands| {
            let roster_high: Vec<String> = cands
                .as_array()?
                .iter()
                .filter(|c| c.get("importance").and_then(|i| i.as_str()).unwrap_or("").eq_ignore_ascii_case("high"))
                .filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .collect();
            if roster_high.is_empty() {
                return Some(true); // 角色谱无 high,不阻塞
            }
            let have_high: Vec<&str> = pack
                .characters
                .iter()
                .filter(|c| {
                    let imp = c.importance.to_lowercase();
                    let r = c.role.to_lowercase();
                    imp == "high" || r.contains("protagonist") || r.contains("主角")
                })
                .map(|c| c.name.as_str())
                .collect();
            // 每个 roster high 名称,在已有 high 卡里能按名字/包含关系找到即覆盖
            Some(roster_high.iter().all(|rh| {
                have_high.iter().any(|h| {
                    h.contains(rh.as_str()) || rh.contains(h)
                })
            }))
        })
        .unwrap_or(true);
    let chars_done = has_distilled && has_high_char && roster_high_covered;
    let world_done = pack.lore_entries.iter().any(|l| {
        l.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s != "lore-blurb")
            .unwrap_or(false)
    });
    let beats_done = pack.nodes.iter().any(|n| !n.locked_beats.is_empty());
    let exits_done = pack
        .nodes
        .iter()
        .any(|n| n.exit.len() > 1 || n.exit.iter().any(|e| e.when != "继续"));
    let mut missing_key_chars: Vec<String> = Vec::new();
    let mut worldline: Value = json!([]);
    let worldline_done = std::fs::read_to_string(
        state
            .app_state
            .data_root()
            .story_packs_dir()
            .join(&pack.id)
            .join("worldline.json"),
    )
    .ok()
    .map(|s| {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            worldline = v;
            // [fix 2026-08-15 覆盖度检查] 只判非空会让旧版不完整产物（如 8-11 只覆盖
            // ch01-04 的 7 条）永久跳过重生成（兔子想吃窝边草 13 章书 2/3 世界线缺失）。
            // 覆盖章节数 ≥ 全书章节数的 1/2 才算完成，否则重跑。
            if worldline_is_empty(&worldline) {
                false
            } else {
                let covered: std::collections::HashSet<String> = worldline
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| e.get("chapter").and_then(|c| c.as_str()))
                            .map(|c| c.to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                let total = distil_chapters.len();
                let coverage = if total > 0 {
                    covered.len() as f32 / total as f32
                } else {
                    1.0
                };
                coverage >= 0.5
            }
        } else {
            false
        }
    })
    .unwrap_or(false);
    let events_done = !pack.event_packages.is_empty();
    let actors_done = !pack.actor_state_config.templates.is_empty();
    let style_done = pack
        .stage_director
        .resolved_snapshot
        .as_ref()
        .map(|s| s.narrative_style.is_some() || s.rule_system.is_some())
        .unwrap_or(false);

    // 0) 基底落盘：0.03 切章完成即写 pack + chapter bodies（任何阶段之前先持久化基础）
    progress("准备完成：章节已切分", 0.03);
    if let Err(e) = save_pack_checkpoint(&state, &pack, &bodies) {
        let _ = jobs.push_event(
            &run_id_job,
            JobEvent::error(format!("base_save: {e}")),
            Some(0.03),
            None,
        );
    }

    // ── 阶段调度（R6-2①：每阶段一个函数，见 stages.rs / stages_assets.rs）──
    let force_relations: bool = resume_meta
        .as_ref()
        .and_then(|m| m.get("force_relations"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let ctx = StageCtx {
        state: &state,
        jobs: &jobs,
        run_id: &run_id_job,
        title: &title,
        distil_chapters: &distil_chapters,
        max_chars,
    };

    // 阶段 0（基底落盘）已在上方 setup 内完成。
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    if let Err(f) = stage_characters(
        &ctx, &mut pack, &bodies, &named_chars, chars_done, &mut missing_key_chars,
    )
    .await
    {
        fail_job(f.stage, &f.e, f.p);
        return;
    }
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    if let Err(f) = stage_world(&ctx, &mut pack, &bodies, world_done).await {
        fail_job(f.stage, &f.e, f.p);
        return;
    }
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    if let Err(f) = stage_beats(&ctx, &mut pack, &bodies, beats_done).await {
        fail_job(f.stage, &f.e, f.p);
        return;
    }
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    if let Err(f) = stage_exits(&ctx, &mut pack, &bodies, exits_done).await {
        fail_job(f.stage, &f.e, f.p);
        return;
    }
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    if let Err(f) = stage_worldline(&ctx, &pack, worldline_done, &mut worldline).await {
        fail_job(f.stage, &f.e, f.p);
        return;
    }
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    stage_relations(&ctx, &mut pack, &bodies, force_relations).await;
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    stage_events(&ctx, &mut pack, &bodies, events_done).await;
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    stage_actors(&ctx, &mut pack, &bodies, actors_done).await;
    if !job_control_gate(&jobs, &run_id_job).await {
        return;
    }
    stage_style(&ctx, &mut pack, &bodies, style_done).await;

    // 最终落盘 pack.json（camelCase，bodies 已在上游保存过则不再重写）
    // 最终落盘 pack.json（camelCase，bodies 已在上游保存过则不再重写）
    match ctx.state.packs.save(pack) {
        Ok(saved) => {
            let beat_count: usize = saved.nodes.iter().map(|n| n.locked_beats.len()).sum();
            let mut result = json!({
                "ok": true,
                "packId": saved.id,
                "title": saved.title,
                "lore_count": saved.lore_entries.len(),
                "beat_count": beat_count,
                "character_count": saved.characters.len(),
                "worldline_count": worldline.as_array().map(|a| a.len()).unwrap_or(0),
                "event_package_count": saved.event_packages.len(),
                "actor_template_count": saved.actor_state_config.templates.len(),
                "has_narrative_style": saved
                    .stage_director
                    .resolved_snapshot
                    .as_ref()
                    .and_then(|s| s.narrative_style.as_ref())
                    .is_some(),
                "rule_check_count": saved
                    .stage_director
                    .resolved_snapshot
                    .as_ref()
                    .and_then(|s| s.rule_system.as_ref())
                    .and_then(|r| r.get("checks"))
                    .and_then(|c| c.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0),
            });
            let report = json!({
                "characters": saved.characters.iter().take(30).map(|c| json!({"id": c.id, "name": c.name, "role": c.role})).collect::<Vec<_>>(),
                "lore": saved.lore_entries.iter().take(30).filter_map(|v| v.get("title").and_then(|t| t.as_str())).map(|t| json!({"title": t})).collect::<Vec<_>>(),
                "beats": {"node_count": saved.nodes.len(), "beat_count": beat_count},
                "exits": saved.nodes.iter().map(|n| n.exit.len()).sum::<usize>(),
                "worldline": worldline.as_array().map(|a| a.iter().take(30).filter_map(|v| {
                    v.get("title").or_else(|| v.get("name")).and_then(|t| t.as_str()).map(|t| json!({"title": t}))
                }).collect::<Vec<_>>()).unwrap_or_default(),
                "event_packages": saved.event_packages.iter().take(30).map(|p| json!({"id": p.id, "name": p.name})).collect::<Vec<_>>(),
                "actor_templates": saved.actor_state_config.templates.iter().take(30).map(|(k, t)| json!({"name": k, "field_count": t.fields.len()})).collect::<Vec<_>>(),
                "narrative_style": saved.stage_director.resolved_snapshot.as_ref()
                    .and_then(|s| s.narrative_style.as_ref())
                    .map(|v| v.to_string()),
                "rule_checks": saved.stage_director.resolved_snapshot.as_ref()
                    .and_then(|s| s.rule_system.as_ref())
                    .and_then(|r| r.get("checks"))
                    .and_then(|c| c.as_array())
                    .map(|a| a.iter().take(30).map(|c| json!({"id": c.get("id").and_then(|x| x.as_str()), "label": c.get("label").and_then(|x| x.as_str()), "dice": c.get("dice").and_then(|x| x.as_str())})).collect::<Vec<_>>())
                    .unwrap_or_default(),
                // A2: 每卡字符数 + 证据引用数，一眼识别空卡/水卡
                "character_card_stats": saved.characters.iter().map(|c| {
                    let content_len = c.personality.len()
                        + c.speech_style.len()
                        + c.motivation.len()
                        + c.example_dialogs.iter().map(|s| s.len()).sum::<usize>()
                        + c.boundaries.iter().map(|s| s.len()).sum::<usize>()
                        + c.relationships.iter().map(|s| s.len()).sum::<usize>()
                        + c.mental_models.iter().map(|s| s.len()).sum::<usize>()
                        + c.decision_heuristics.iter().map(|s| s.len()).sum::<usize>()
                        + c.beliefs.iter().map(|s| s.len()).sum::<usize>();
                    json!({
                        "name": c.name,
                        "chars_len": content_len,
                        "evidence_refs_len": c.evidence_refs.len(),
                    })
                }).collect::<Vec<_>>(),
                // A1: 缺失关键角色名数组（可为空）
                "missing_key_characters": missing_key_chars,
            });
            result["report"] = report.clone();
            // 落盘蒸馏报告（best-effort，失败仅 warn）
            let pack_dir = ctx.state.app_state.data_root().story_packs_dir().join(&saved.id);
            let report_path = pack_dir.join("distill-report.json");
            if let Ok(text) = serde_json::to_string_pretty(&report) {
                if let Err(e) = std::fs::write(&report_path, text) {
                    tracing::warn!(path = %report_path.display(), error = %e, "write distill report failed");
                }
                // 蒸馏报告同步存档到书架（novel_workspace/），用户可在书架中阅读。
                // 格式复用书架 md 约定：首行 `# 标题`，后续小节按 `- 第N节` 分章；
                // 渲染失败不影响主流程（best-effort）。
                if let Some(md) = render_distill_report_md(&report, &entry.title) {
                    // 文件名用报告完整标题的 slug（含「（蒸馏报告）」后缀，与正文 `# 标题` 一致），
                    // 避免 `__` 分隔符导致 shelf_slug 后 stem 与 slug_tag 不匹配（contains 失败 → file not found）。
                    let shelf_title = format!("{}（蒸馏报告）", entry.title);
                    let shelf_path = crate::crawler::shelf_dir()
                        .join(format!("{}.md", crate::crawler::shelf_slug(&shelf_title)));
                    if let Err(e) = std::fs::write(&shelf_path, md) {
                        tracing::warn!(path = %shelf_path.display(), error = %e, "write distill report shelf md failed");
                    } else {
                        tracing::info!(path = %shelf_path.display(), "distill report archived to bookshelf");
                    }
                }
            }
            prog(&ctx, "保存完成", 1.0);
            let _ = jobs.complete(ctx.run_id, "succeeded", Some(result), None);
        }
        Err(e) => {
            fail_job("pack_save", &e.to_string(), 1.0);
        }
    }
}
