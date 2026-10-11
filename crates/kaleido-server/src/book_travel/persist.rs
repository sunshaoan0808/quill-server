//! book_travel/persist —— 落盘 Works、取消判定、模板输出族、启发式分类词表。（R6④a 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn slugify_title(title: &str) -> String {
    let s: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c.is_whitespace() || c == '-' || c == '_' {
                '-'
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "book-travel".into()
    } else {
        s.chars().take(48).collect()
    }
}


pub(crate) fn complete_if_not_cancelled(
    jobs: &kaleido_core::JobStore,
    run_id: &str,
    status: &str,
    result: Option<Value>,
    error: Option<String>,
) {
    if cancelled(jobs, run_id) && status != "cancelled" {
        let _ = jobs.complete(run_id, "cancelled", result, error.or_else(|| Some("已取消".into())));
        return;
    }
    let _ = jobs.complete(run_id, status, result, error);
}

/// Persist pipeline artifact under works/{ws}/book-travel/{slug}-{runShort}/
pub(crate) fn persist_pipeline_to_works(state: &AppState, run_id: &str, result: &mut Value) {
    let Some(job) = state.jobs.get(run_id) else {
        return;
    };
    let title = result
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled Book Travel");
    let slug = slugify_title(title);
    let short = run_id.chars().take(8).collect::<String>();
    let dir = format!("book-travel/{slug}-{short}");
    let md_path = format!("{dir}/journey.md");
    let json_path = format!("{dir}/result.json");

    let mut md = String::new();
    md.push_str(&format!("# {title}\n\n"));
    md.push_str(&format!("runId: `{run_id}`\n\n"));
    if let Some(stages) = result.get("stages").and_then(|v| v.as_array()) {
        for (i, st) in stages.iter().enumerate() {
            let step = st.get("step").and_then(|v| v.as_str()).unwrap_or("?");
            md.push_str(&format!("## {}. {step}\n\n", i + 1));
            if let Some(res) = st.get("result") {
                if let Some(plan) = res.get("plan").and_then(|v| v.as_str()) {
                    md.push_str(plan);
                    md.push_str("\n\n");
                }
                if let Some(sp) = res.get("scenePlan") {
                    md.push_str("```json\n");
                    md.push_str(&serde_json::to_string_pretty(sp).unwrap_or_default());
                    md.push_str("\n```\n\n");
                }
                if let Some(w) = res.get("writerOutput") {
                    if let Some(content) = w
                        .pointer("/beat/content")
                        .and_then(|v| v.as_str())
                    {
                        md.push_str(content);
                        md.push_str("\n\n");
                    } else {
                        md.push_str("```json\n");
                        md.push_str(&serde_json::to_string_pretty(w).unwrap_or_default());
                        md.push_str("\n```\n\n");
                    }
                }
                if let Some(e) = res.get("ending") {
                    md.push_str("```json\n");
                    md.push_str(&serde_json::to_string_pretty(e).unwrap_or_default());
                    md.push_str("\n```\n\n");
                }
                if let Some(m) = res.get("memory") {
                    md.push_str("```json\n");
                    md.push_str(&serde_json::to_string_pretty(m).unwrap_or_default());
                    md.push_str("\n```\n\n");
                }
            }
        }
    }

    let ws = &job.workspace_id;
    let json_body = serde_json::to_string_pretty(result).unwrap_or_else(|_| "{}".into());
    // WorksFs requires parents to exist (mkdir first).
    let _ = state.works.mkdir(ws, "book-travel");
    let _ = state.works.mkdir(ws, &dir);
    let _ = state.works.mkdir(ws, "book-travel/steps");
    let wrote_md = match state.works.write_text(ws, &md_path, &md) {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(target: "book_travel", error=%e, path=%md_path, "persist journey.md failed");
            false
        }
    };
    let wrote_json = match state.works.write_text(ws, &json_path, &json_body) {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(target: "book_travel", error=%e, path=%json_path, "persist result.json failed");
            false
        }
    };

    if let Some(obj) = result.as_object_mut() {
        if wrote_md {
            obj.insert("workPath".into(), json!(md_path));
        }
        if wrote_json {
            obj.insert("resultPath".into(), json!(json_path));
        }
        obj.insert(
            "worksDir".into(),
            json!(dir),
        );
        obj.insert("persisted".into(), json!(wrote_md || wrote_json));
    }

    let _ = state.jobs.push_event(
        run_id,
        JobEvent::event(
            "pipeline persisted to works",
            Some(json!({
                "workPath": md_path,
                "resultPath": json_path,
                "worksDir": dir,
            })),
        ),
        Some(0.96),
        Some("pipeline:persist".into()),
    );
}

pub(crate) fn persist_step_snapshot_to_works(
    state: &AppState,
    run_id: &str,
    step: &str,
    title: &str,
    result: &Value,
) -> Option<String> {
    let job = state.jobs.get(run_id)?;
    let slug = slugify_title(title);
    let short = run_id.chars().take(8).collect::<String>();
    let path = format!("book-travel/steps/{slug}-{short}-{step}.json");
    let body = serde_json::to_string_pretty(result).ok()?;
    let ws = &job.workspace_id;
    let _ = state.works.mkdir(ws, "book-travel");
    let _ = state.works.mkdir(ws, "book-travel/steps");
    state.works.write_text(ws, &path, &body).ok()?;
    Some(path)
}


pub(crate) fn cancelled(jobs: &kaleido_core::JobStore, run_id: &str) -> bool {
    jobs.get(run_id)
        .map(|j| is_terminal_job_status(&j.status))
        .unwrap_or(true)
}

// ---------------------------------------------------------------------------
// Templates (upstream-shaped)
// ---------------------------------------------------------------------------

pub(crate) fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub(crate) fn template_assemble_plan(title: &str, premise: &str, labels: &Value) -> String {
    let genre = labels
        .get("genre")
        .and_then(|v| v.as_str())
        .unwrap_or("general_fiction");
    let tone = labels
        .get("tone")
        .and_then(|v| v.as_str())
        .unwrap_or("neutral");
    format!(
        "# {title} — Book Travel Plan\n\n\
         ## Premise\n{premise}\n\n\
         ## Labels\n\
         - Genre: {genre}\n\
         - Tone: {tone}\n\n\
         ## Travel Beats\n\
         1. Entrance — establish world and protagonist stake\n\
         2. First turn — introduce core conflict\n\
         3. Midpoint mirror — reframe goal\n\
         4. Crisis — force a choice\n\
         5. Exit / coda — land theme\n"
    )
}

pub(crate) fn template_scene_plan(
    title: &str,
    premise: &str,
    user_input: &str,
    labels: &Value,
    context: &Value,
) -> Value {
    let id = Uuid::new_v4().to_string();
    let hint = if user_input.trim().is_empty() {
        short(premise, 80)
    } else {
        short(user_input, 80)
    };
    let genre = labels
        .get("genre")
        .and_then(|v| v.as_str())
        .unwrap_or("general_fiction");
    let prior = context
        .get("scenePlan")
        .or_else(|| context.get("plan"))
        .cloned();
    let progress = prior
        .as_ref()
        .and_then(|p| p.get("storyProgress"))
        .and_then(|v| v.as_u64())
        .unwrap_or(10)
        .saturating_add(15)
        .min(95) as u32;

    json!({
        "id": id,
        "title": format!("{title} · 场景"),
        "summary": format!("围绕「{hint}」推进的下一场景（{genre}）"),
        "currentSituation": format!("承接前提：{}", short(premise, 100)),
        "time": "故事时间线 · 下一拍",
        "location": "与世界书一致的关键场景",
        "activeCharacters": ["主角", "关键配角"],
        "stateChanges": {
            "tension": "上升",
            "information": "部分揭示"
        },
        "divergence": "相对原大纲可偏移，但保留主线张力",
        "storyProgress": progress,
        "endingStatus": null,
        "sceneGoals": [
            "推进核心矛盾",
            "给出可操作的选择点",
            "留下下一拍钩子"
        ],
        "entryBeatGuidance": format!("开场切入：{hint}"),
        "writerInstructions": "保持角色声线一致；动作优先；避免总结腔。"
    })
}

pub(crate) fn template_writer_output(
    title: &str,
    premise: &str,
    user_input: &str,
    flow: &str,
    _context: &Value,
) -> Value {
    let beat_id = Uuid::new_v4().to_string();
    let directive = if user_input.trim().is_empty() {
        short(premise, 60)
    } else {
        short(user_input, 60)
    };
    let content = match flow {
        "insert-beat" => format!(
            "【插入 Beat · {title}】\n\
             在当前场景缝隙中插入一拍：{directive}\n\
             角色做出细微却关键的反应，信息量增加，但不跳过冲突。\n\
             （模板输出，可后续接 LLM 深化）"
        ),
        _ => format!(
            "【改写场景 · {title}】\n\
             按用户意图改写当前场景：{directive}\n\
             保留已确立的人物关系，调整事件走向与张力曲线。\n\
             （模板输出，可后续接 LLM 深化）"
        ),
    };
    json!({
        "beat": {
            "id": beat_id,
            "content": content
        },
        "stableMemoryPatch": {
            "note": format!("flow={flow} 固化：用户意图「{directive}」")
        },
        "volatileMemoryPatch": {
            "lastBeat": flow,
            "directive": directive
        }
    })
}

pub(crate) fn template_memory(title: &str, premise: &str, context: &Value) -> Value {
    let prior_choices = context
        .get("keyChoices")
        .cloned()
        .unwrap_or_else(|| json!(["进入书中世界", "第一次关键抉择"]));
    json!({
        "summary": format!(
            "《{title}》穿书记忆摘要：以「{}」为起点，当前主线张力持续上升。",
            short(premise, 60)
        ),
        "keyChoices": prior_choices,
        "unresolvedConflicts": ["核心矛盾未决", "关键配角立场不明"],
        "divergenceFromOutline": "已产生可感知偏移，但尚未彻底脱离主线"
    })
}

pub(crate) fn template_ending(title: &str, premise: &str, labels: &Value, _context: &Value) -> Value {
    let tone = labels
        .get("tone")
        .and_then(|v| v.as_str())
        .unwrap_or("neutral");
    let ending = match tone {
        "dark" => "苦涩收束：代价已付，真相留下余烬",
        "light" => "轻盈收束：冲突化解，关系向前",
        "epic" => "史诗收束：世界线改写，主角成为传说的一部分",
        _ => "开放收束：主线落地，支线仍有余韵",
    };
    json!({
        "finalEnding": format!("《{title}》· {ending}"),
        "userKeyChoices": [
            format!("基于前提「{}」的关键选择", short(premise, 40)),
            "中段转向",
            "终局一搏"
        ],
        "originalOutlineComparison": "相对原大纲有中等偏移，主题落地方式被用户选择改写",
        "characterOutcomes": [
            "主角：完成弧光",
            "关键配角：立场落定"
        ],
        "worldlineName": format!("{title}-世界线-A"),
        "divergenceScore": 42
    })
}

// ---------------------------------------------------------------------------
// Heuristic classify
// ---------------------------------------------------------------------------

pub(crate) fn heuristic_classify(text: &str, title: Option<&str>, genre_hint: Option<&str>) -> BookTravelLabels {
    let blob = format!("{} {}", title.unwrap_or(""), text).to_lowercase();
    let mut notes: Vec<String> = Vec::new();
    notes.push("classified by keyword heuristics".into());

    let genre = if let Some(h) = genre_hint.map(str::trim).filter(|s| !s.is_empty()) {
        notes.push(format!("genre_hint honored: {h}"));
        h.to_string()
    } else if contains_any(&blob, &["sci-fi", "scifi", "space", "robot", "cyber", "科幻", "星际"]) {
        "science_fiction".into()
    } else if contains_any(&blob, &["fantasy", "magic", "dragon", "elf", "wizard", "奇幻", "魔法"]) {
        "fantasy".into()
    } else if contains_any(&blob, &["mystery", "detective", "murder", "clue", "悬疑", "推理"]) {
        "mystery".into()
    } else if contains_any(&blob, &["romance", "love", "爱情", "恋爱", "言情"]) {
        "romance".into()
    } else if contains_any(&blob, &["horror", "ghost", "恐怖", "惊悚"]) {
        "horror".into()
    } else if contains_any(&blob, &["history", "historical", "王朝", "历史"]) {
        "historical".into()
    } else if contains_any(&blob, &["xianxia", "cultivation", "修仙", "玄幻", "武侠"]) {
        "xianxia".into()
    } else {
        "general_fiction".into()
    };

    let tone = if contains_any(&blob, &["dark", "grim", "tragic", "黑暗", "悲"]) {
        "dark".into()
    } else if contains_any(&blob, &["humor", "comedy", "funny", "幽默", "轻松"]) {
        "light".into()
    } else if contains_any(&blob, &["epic", "grand", "史诗"]) {
        "epic".into()
    } else {
        "neutral".into()
    };

    let pov = if contains_any(&blob, &["first person", "i ", "我", "第一人称"]) {
        "first_person".into()
    } else if contains_any(&blob, &["second person", "you ", "你", "第二人称"]) {
        "second_person".into()
    } else {
        "third_person".into()
    };

    let era = if contains_any(&blob, &["future", "2077", "space age", "未来", "赛博"]) {
        "near_future".into()
    } else if contains_any(&blob, &["medieval", "kingdom", "中世纪", "古代"]) {
        "pre_modern".into()
    } else if contains_any(&blob, &["victorian", "1920", "ww2", "民国"]) {
        "historical_modern".into()
    } else {
        "contemporary".into()
    };

    let mut themes: Vec<String> = Vec::new();
    if contains_any(&blob, &["revenge", "复仇"]) {
        themes.push("revenge".into());
    }
    if contains_any(&blob, &["coming of age", "成长"]) {
        themes.push("coming_of_age".into());
    }
    if contains_any(&blob, &["power", "权力", "政治"]) {
        themes.push("power".into());
    }
    if contains_any(&blob, &["identity", "身份", "自我"]) {
        themes.push("identity".into());
    }
    if contains_any(&blob, &["survival", "生存", "末日"]) {
        themes.push("survival".into());
    }
    if themes.is_empty() {
        themes.push("discovery".into());
    }

    let pacing = if contains_any(&blob, &["slow burn", "slow", "缓慢"]) {
        "slow".into()
    } else if contains_any(&blob, &["fast", "thriller", "紧张", "快节奏"]) {
        "fast".into()
    } else {
        "moderate".into()
    };

    let audience = if contains_any(&blob, &["ya", "young adult", "teen", "青春"]) {
        "young_adult".into()
    } else if contains_any(&blob, &["kids", "children", "儿童"]) {
        "children".into()
    } else {
        "adult".into()
    };

    let mut hits = 0u32;
    if genre != "general_fiction" {
        hits += 1;
    }
    if tone != "neutral" {
        hits += 1;
    }
    if era != "contemporary" {
        hits += 1;
    }
    if themes.first().map(|s| s.as_str()) != Some("discovery") || themes.len() > 1 {
        hits += 1;
    }
    let confidence = (0.35 + 0.15 * hits as f64).min(0.9);

    BookTravelLabels {
        genre,
        tone,
        pov,
        era,
        themes,
        pacing,
        audience,
        mode: "heuristic".into(),
        // S7-W4
        confidence,
        notes,
    }
}

pub(crate) fn contains_any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}
