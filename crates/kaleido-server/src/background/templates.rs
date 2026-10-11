//! background/templates —— 取消判定 + 各阶段模板输出 + 摘要助手。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn cancelled(jobs: &kaleido_core::JobStore, run_id: &str) -> bool {
    jobs.get(run_id)
        .map(|j| is_terminal_job_status(&j.status))
        .unwrap_or(true)
}

/// Structured stage_one aligned with upstream BackgroundStageOneResponse.
pub(crate) fn template_stage_one(title: &str, premise: &str, include_names: bool) -> Value {
    let short = premise_brief(premise, 80);
    let mut names = Vec::new();
    if include_names {
        // Prefer explicit "角色名候选：A、B" / "角色：A、B、C" lists in premise.
        for marker in ["角色名候选：", "角色名候选:", "角色：", "角色:"] {
            if let Some(pos) = premise.find(marker) {
                let tail = &premise[pos + marker.len()..];
                let chunk = tail.split('\n').next().unwrap_or(tail);
                for part in chunk.split(|c: char| {
                    c == '、' || c == ',' || c == '，' || c == '/' || c == '|' || c == '；' || c == ';'
                }) {
                    let n = part
                        .trim()
                        .trim_matches(|c: char| c == '。' || c == '.' || c == '》' || c == '《')
                        .to_string();
                    // 2–8 CJK / word-ish tokens; skip long clauses
                    let cc = n.chars().count();
                    if (2..=8).contains(&cc)
                        && n.chars().any(|c| c.is_alphanumeric())
                        && !names.iter().any(|x| x == &n)
                    {
                        names.push(n);
                    }
                }
                if !names.is_empty() {
                    break;
                }
            }
        }
        if names.is_empty() {
            for candidate in ["主角", "伙伴", "导师"] {
                if premise.contains(candidate) {
                    names.push(candidate.to_string());
                }
            }
        }
        if names.is_empty() {
            names = vec!["林晚星".into(), "顾沉舟".into()];
        }
    }
    json!({
        "worldBooks": [{
            "name": format!("{title} · 世界设定集"),
            "fields": {
                "theme": if short.is_empty() { "原创奇幻".into() } else { short.clone() },
                "era": "待定时代",
                "techLevel": "与前提一致",
                "magicLevel": "与前提一致",
                "geography": format!("由前提推导：{short}"),
                "keyScenes": "开场场景 / 冲突爆发点 / 终局舞台",
                "culturalFeatures": "待深化（items / 角色卡阶段可补）",
                "history": "关键转折待填",
                "conflict": format!("核心矛盾围绕：{short}")
            }
        }],
        "characterNames": names,
    })
}

/// Full extract (worldBooks + characterCards) for items stage.
pub(crate) fn template_items(title: &str, premise: &str) -> Value {
    let stage = template_stage_one(title, premise, true);
    let names = stage
        .get("characterNames")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut cards = Vec::new();
    for n in names.iter().take(3) {
        let name = n.as_str().unwrap_or("未命名").to_string();
        cards.push(template_character_card(&name, premise, ""));
    }
    if cards.is_empty() {
        cards.push(template_character_card("林晚星", premise, ""));
    }
    json!({
        "worldBooks": stage.get("worldBooks").cloned().unwrap_or(json!([])),
        "characterCards": cards,
    })
}



/// Short human premise for templates: drop trailing "角色名候选/角色：" lists, collapse whitespace.
pub(crate) fn premise_brief(premise: &str, max_chars: usize) -> String {
    let mut s = premise.trim().to_string();
    for marker in ["角色名候选：", "角色名候选:", "角色：", "角色:"] {
        if let Some(pos) = s.find(marker) {
            s = s[..pos].trim_end().trim_end_matches('。').trim_end_matches('.').to_string();
            break;
        }
    }
    let collapsed: String = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    collapsed.chars().take(max_chars).collect()
}

/// Plain-text world-book hint for character deepen (never raw JSON — that leaked into coreDesire).
pub(crate) fn world_book_context_summary(world_books: &Value, premise: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(arr) = world_books.as_array() {
        for wb in arr.iter().take(2) {
            if let Some(n) = wb.get("name").and_then(|v| v.as_str()) {
                let n = n.trim();
                if !n.is_empty() {
                    parts.push(format!("世界书：{n}"));
                }
            }
            if let Some(fields) = wb.get("fields").and_then(|v| v.as_object()) {
                // Prefer compact theme; skip conflict/geography when they restate premise.
                if let Some(s) = fields.get("theme").and_then(|v| v.as_str()) {
                    let s = s.trim();
                    if !s.is_empty() {
                        parts.push(format!("主题：{s}"));
                    }
                }
                if let Some(s) = fields.get("era").and_then(|v| v.as_str()) {
                    let s = s.trim();
                    if !s.is_empty() && s != "待定时代" {
                        parts.push(format!("时代：{s}"));
                    }
                }
            } else if let Some(c) = wb.get("content").and_then(|v| v.as_str()) {
                let s: String = c.chars().take(120).collect();
                if !s.trim().is_empty() {
                    parts.push(s);
                }
            }
        }
    }
    if parts.is_empty() {
        premise_brief(premise, 120)
    } else {
        let joined = parts.join("；");
        joined.chars().take(240).collect()
    }
}

/// Single character card (upstream GeneratedBackgroundItem shape).
pub(crate) fn template_character_card(name: &str, premise: &str, world_book_context: &str) -> Value {
    let hint = if world_book_context.trim().is_empty() {
        premise_brief(premise, 80)
    } else {
        // Already a plain summary from world_book_context_summary; still cap.
        world_book_context.chars().take(120).collect::<String>()
    };
    json!({
        "name": name,
        "fields": {
            "age": "未知",
            "gender": "未知",
            "race": "人类",
            "birthplace": "与世界书一致",
            "occupation": "待定",
            "socialClass": "待定",
            "identityTags": ["核心角色", "可互动"],
            "heightBuild": "中等",
            "iconicFeatures": format!("{name} 的标志性特征（模板）"),
            "clothingStyle": "贴合时代与身份",
            "overallVibe": "鲜明且可辨识",
            "externalPersonality": "外在表现克制",
            "internalPersonality": "内在层次更丰富",
            "coreDesire": format!("与「{hint}」相关的核心驱动力"),
            "fearWeakness": "与核心欲望相对的软肋",
            "moralValues": "有底线、可冲突",
            "quirk": "习惯性小动作",
            "skills": "与设定自洽的专长",
            "backgroundStory": format!("{name} 的身世：由前提「{hint}」概括生成（模板，可后续 LLM 深化）"),
            "relationships": "与主角/用户的关系网待补",
            "speakingStyle": "口语化、有口头禅",
            "typicalReactions": "压力下的典型反应",
            "userRelationType": "可成长关系",
            "userInteractionModel": "先疏后密 / 可被改写",
            "userRelationBottomLine": "不无底线越界",
            "keyEvents": "与用户的关键里程碑待展开"
        }
    })
}

// ---------------------------------------------------------------------------
// Apply background result → Partner store
// ---------------------------------------------------------------------------
