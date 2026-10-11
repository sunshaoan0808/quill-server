#![allow(unused_imports)]
use axum::{
    extract::State,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
use kaleido_core::JobStore;
use crate::encoding_sniff::decode_text;
use crate::error_codes::*;
use super::*;
use kaleido_core::{is_terminal_job_status, normalize_job_status, JobEvent, JobListFilter};
use crate::{map_core_err, session_from, AppState};

pub(crate) fn extract_cast_names(chapters: &[(String, String)]) -> Vec<String> {
    // Match a short CJK name immediately before a dialogue verb.
    // The regex crate has no look-around, so the clause-boundary check
    // (previous char must not be CJK or an interpunct) is done manually:
    // a match with a CJK/· predecessor is inside a longer run, and so is
    // every inner start position, hence the whole match can be skipped.
    let re = regex::Regex::new(r"[\u{4e00}-\u{9fff}·]{2,4}(?:说|道|问|答|喊|叫)").ok();
    let mut seen = std::collections::BTreeSet::new();
    seen.insert("旁白".into());
    seen.insert("读者".into());
    seen.insert("玩家".into());
    let mut names = Vec::new();
    let junk = [
        "露出", "眼角", "换鞋", "随口", "轻声", "低头", "抬起", "转身", "伸手", "走过去", "看向",
        "听见", "突然", "只是", "已经", "然后", "因为", "所以", "连忙", "依旧", "还是",
        "你也没", "有一", "第一道", "第二道", "第三道", "第四道", "第五道", "第六道", "第七道",
        "第八道", "第九道", "第一张", "第二张", "第三张", "第四张", "第五张", "第六张", "第七张",
        "第八张", "第九张", "跟莫",
    ];
    if let Some(re) = re {
        for (_t, body) in chapters.iter().take(40) {
            for m in re.find_iter(body) {
                // manual look-behind: previous char must not be CJK or ·,
                // i.e. the match must start at a clause boundary
                let mut prev = body[..m.start()].chars();
                if let Some(c) = prev.next_back() {
                    if ('\u{4e00}'..='\u{9fff}').contains(&c) || c == '\u{00b7}' {
                        continue;
                    }
                }
                let n = m.as_str().chars().take(m.as_str().chars().count().saturating_sub(1)).collect::<String>();
                if n.is_empty() || seen.contains(&n) {
                    continue;
                }
                if junk.iter().any(|p| n.starts_with(p)) {
                    continue;
                }
                if !is_clean_name(&n) {
                    continue;
                }
                // sanity: an extracted name should not contain dialogue verbs itself
                if n.chars().any(|c| "说道问答喊叫".contains(c)) {
                    continue;
                }
                seen.insert(n.clone());
                names.push(n);
                if names.len() >= 6 {
                    return names;
                }
            }
        }
    }
    names
}

/// 重映射 nodes.present 到蒸馏角色：按每个 node 所在章节正文实际出现的
/// 蒸馏角色名（c-distil-*）重构 present，替换 build_pack_from_chapters 遗留的
/// c-cast 垃圾占位引用。
///
/// 背景：build_pack_from_chapters 用 extract_cast_names（正则抓「说/道」前短语）
/// 生成 c-cast-{i} 占位角色，质量差（常是「微微鞠躬」这类非名字），且 nodes.present
/// 全量克隆同一份列表。真正角色由 distill_pack_characters 以 LLM 抽出为 c-distil-*。
/// 此前 nodes.present 一直是 c-cast 垃圾引用 → build_mainline_opening 按 present
/// 找角色 opening 永远匹配不上蒸馏角色，开局个性化开场退化为通用模板。
/// 修复：distill 后按正文里的真实角色名重构每个 node 的 present。
/// `bodies` 与 nodes 按顺序对应（i 章 → node n{i+1} body 为 bodies[i].1）。
pub(crate) fn remap_node_present_to_distil(
    nodes: &mut [kaleido_core::StoryNode],
    characters: &[kaleido_core::PackCharacterRef],
    bodies: &[(String, String)],
) {
    // 蒸馏角色（c-distil-*）名字列表
    let distil: Vec<(&str, &str)> = characters
        .iter()
        .filter(|c| c.id.starts_with("c-distil-"))
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect();
    if distil.is_empty() {
        return;
    }
    for (i, node) in nodes.iter_mut().enumerate() {
        let body = bodies.get(i).map(|b| b.1.as_str()).unwrap_or("");
        let mut present: Vec<String> = Vec::new();
        for (id, name) in &distil {
            // 名字 ≥2 字且在该章正文出现 → 在场
            if name.chars().count() >= 2 && body.contains(name) {
                present.push((*id).to_string());
            }
        }
        if !present.is_empty() {
            node.present_characters = present;
        } else {
            // 正文无任何蒸馏角色直名（旁白/抒情代词章）→ 清空 present，
            // 不保留 build_pack_from_chapters 遗留的 c-cast 悬空引用。
            node.present_characters = Vec::new();
        }
    }
}

pub(crate) fn build_pack_from_chapters(
    title: &str,
    chapters: &[(String, String)],
    pack_id: &str,
    source_ref: &str,
) -> (kaleido_core::StoryPack, Vec<(String, String)>) {
    use kaleido_core::{
        NodeExit, PackCharacterRef, PackSource, PlayMode, StoryChapter, StoryNode, StoryPack,
    };
    let now = chrono::Utc::now().to_rfc3339();
    let mut chars = vec![
        PackCharacterRef {
            id: "c-narrator".into(),
            name: "旁白".into(),
            role: "narrator".into(),
            importance: "low".into(),
            gender: "未知".into(),
            appearance: "未知".into(),
            opening_scene: "未知".into(),
            opening_lines: "".into(),
            nsfw_profile: String::new(),
            content_tier: None,
            example_dialogs: vec![],
            boundaries: vec![],
            personality: "旁白".into(),
            speech_style: String::new(),
            voice_profile: String::new(),
            motivation: String::new(),
            relationships: vec![],
            evidence_refs: vec![],
            mental_models: vec![],
            decision_heuristics: vec![],
            beliefs: vec![],
            expressions: Default::default(),
            voice: None,
            archive: None,
            avatar: None,
            starting_wardrobe: Default::default(),
        },
        PackCharacterRef {
            id: "c-player".into(),
            name: "读者".into(),
            role: "player".into(),
            importance: "low".into(),
            gender: "未知".into(),
            appearance: "未知".into(),
            opening_scene: "未知".into(),
            opening_lines: "".into(),
            nsfw_profile: String::new(),
            content_tier: None,
            example_dialogs: vec![],
            boundaries: vec![],
            personality: "你自己".into(),
            speech_style: String::new(),
            voice_profile: String::new(),
            motivation: String::new(),
            relationships: vec![],
            evidence_refs: vec![],
            mental_models: vec![],
            decision_heuristics: vec![],
            beliefs: vec![],
            expressions: Default::default(),
            voice: None,
            archive: None,
            avatar: None,
            starting_wardrobe: Default::default(),
        },
    ];
    for (i, n) in extract_cast_names(chapters).into_iter().enumerate() {
        chars.push(PackCharacterRef {
            id: format!("c-cast-{i}"),
            name: n,
            role: if i == 0 { "protagonist".into() } else { "supporting".into() },
            importance: "medium".into(),
            gender: "未知".into(),
            appearance: "未知".into(),
            opening_scene: "未知".into(),
            opening_lines: "".into(),
            nsfw_profile: String::new(),
            content_tier: None,
            example_dialogs: vec![],
            boundaries: vec![],
            personality: String::new(),
            speech_style: String::new(),
            voice_profile: String::new(),
            motivation: String::new(),
            relationships: vec![],
            evidence_refs: vec![],
            mental_models: vec![],
            decision_heuristics: vec![],
            beliefs: vec![],
            expressions: Default::default(),
            voice: None,
            archive: None,
            avatar: None,
            starting_wardrobe: Default::default(),
        });
    }
    let present: Vec<String> = chars
        .iter()
        .filter(|c| c.role != "narrator" && c.role != "player")
        .map(|c| c.id.clone())
        .collect();
    let mut story_chapters = Vec::new();
    let mut nodes = Vec::new();
    let mut bodies = Vec::new();
    // A0 完整版 (2026-08-19): chXX 用原著章号（缺章自动跳号对齐标题），
    // 修复 D1「章号错位」——此前用切分序号 i+1，源缺章时 ch05 起整体前移错位，
    // 污染 worldline/relations/事件卡等带章节产物的引用。
    let chapter_ids = chapter_id_seq(chapters);
    for (i, (ctitle, body)) in chapters.iter().enumerate() {
        let ch_id = chapter_ids[i].clone();
        let node_id = format!("n{}", i + 1);
        let body_path = format!("chapters/{ch_id}.md");
        story_chapters.push(StoryChapter {
            id: ch_id.clone(),
            title: ctitle.chars().take(120).collect(),
            order: (i + 1) as u32,
            goals: vec![],
            node_ids: vec![node_id.clone()],
            body_path: body_path.clone(),
            image_path: String::new(), // U10
        });
        let mut exits = Vec::new();
        if i + 1 < chapters.len() {
            exits.push(NodeExit {
                id: format!("e{}", i + 1),
                when: "继续".into(),
                next: format!("n{}", i + 2),
            });
        }
        nodes.push(StoryNode {
            id: node_id,
            chapter_id: ch_id,
            title: ctitle.chars().take(80).collect(),
            entry: "本章开始".into(),
            exit: exits,
            locked_beats: vec![],
            allowed_divergence: "branch".into(),
            present_characters: present.clone(),
            location_id: None,
            summary: body.chars().take(400).collect(),
        });
        bodies.push((body_path, body.clone()));
    }
    let blurb: String = chapters
        .first()
        .map(|(_, b)| b.chars().take(140).collect())
        .unwrap_or_default();
    let lore = if blurb.is_empty() {
        vec![]
    } else {
        vec![json!({"id":"lore-blurb","title":"简介","text": blurb, "range":"","permanent":true})]
    };
    let pack = StoryPack {
        id: pack_id.into(),
        title: title.into(),
        source: PackSource {
            source_type: "novel".into(),
            refs: vec![source_ref.into()],
        },
        characters: chars,
        world_book_ids: vec![],
        chapters: story_chapters,
        nodes,
        lore_entries: lore,
        event_packages: vec![],
        actor_state_config: kaleido_core::ActorStatePackConfig::default(),
        default_mode: PlayMode::Mainline,
        max_tier: kaleido_core::ContentTier::Open,
        language: "zh".into(),
        created_at: now.clone(),
        updated_at: now,
        stage_director: Default::default(),
        worldline: vec![], // T 层：旁挂 worldline.json 由 PackStore::load 填充，构造时空
    };
    (pack, bodies)
}

pub(crate) fn find_existing_pack_for_shelf(
    packs: &kaleido_core::PackStore,
    slug: &str,
    title: &str,
) -> Option<String> {
    let list = packs.list().ok()?;
    for s in list {
        // cheap: get full and check source.refs
        if let Ok(p) = packs.get(&s.id) {
            if p.source.refs.iter().any(|r| r == slug || r.contains(slug)) {
                return Some(p.id);
            }
            if p.title == title && p.source.source_type == "novel" {
                return Some(p.id);
            }
        }
    }
    None
}

pub(crate) fn write_shelf_markdown(title: &str, text: &str, chapters: &[(String, String)]) -> Result<(String, std::path::PathBuf), String> {
    let dir = shelf_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let slug = shelf_slug(title);
    if slug.is_empty() {
        return Err("invalid title".into());
    }
    let mut md = String::new();
    md.push_str(&format!("# {title}\n\n"));
    md.push_str("## 目录\n");
    for (i, (ct, _)) in chapters.iter().enumerate() {
        let disp = if ct.starts_with('第') {
            ct.clone()
        } else {
            format!("第{}章 {ct}", i + 1)
        };
        md.push_str(&format!("- {disp}\n"));
    }
    md.push('\n');
    // Prefer original text if it already has structure; else stitch chapters
    if text.lines().any(|l| l.starts_with("第") && (l.contains('章') || l.contains('节'))) {
        md.push_str(text.trim());
        md.push('\n');
    } else {
        for (i, (ct, body)) in chapters.iter().enumerate() {
            let disp = if ct.starts_with('第') {
                ct.clone()
            } else {
                format!("第{}章 {ct}", i + 1)
            };
            md.push_str(&format!("## {disp}\n\n{body}\n\n"));
        }
    }
    // chapter_count scanner uses lines starting with "- 第"
    // ensure at least those exist (already in 目录)
    // Filename dedup: if same-title file exists with different content, append _2, _3...
    let base_name = format!("{title}.md").replace(['/', '\\'], "_");
    let mut path = dir.join(&base_name);
    if path.exists() {
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        // If existing file doesn't start with same title heading, it's a different book
        let existing_title = existing.lines().find(|l| l.starts_with("# ")).unwrap_or("");
        if existing_title != format!("# {title}") {
            let stem = format!("{title}").replace(['/', '\\'], "_");
            for i in 2..100 {
                let candidate = dir.join(format!("{stem}_{i}.md"));
                if !candidate.exists() {
                    path = candidate;
                    break;
                }
            }
        }
    }
    std::fs::write(&path, md).map_err(|e| e.to_string())?;
    Ok((slug, path))
}

/// 将蒸馏报告 JSON 渲染为书架可读的 markdown。
/// 书架 md 约定：首行 `# 标题`（scan_shelf 用其生成 slug/标题），
/// 小节按 `- 第N节` 分章（chapter_count 统计）。报告各字段转成可读小节。
pub(crate) fn render_distill_report_md(report: &serde_json::Value, title: &str) -> Option<String> {
    fn s(v: &serde_json::Value, key: &str) -> String {
        v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
    }
    #[allow(dead_code)] // [P7] JSON 数组长度辅助预留
    fn arr_len(v: &serde_json::Value, key: &str) -> usize {
        v.get(key).and_then(|x| x.as_array()).map(|a| a.len()).unwrap_or(0)
    }
    let mut md = String::new();
    md.push_str(&format!("# {}（蒸馏报告）\n\n", title));
    md.push_str("> 本文件由小说蒸馏流水线自动生成，记录角色/世界观/节点/检定等全部蒸馏产物。\n\n");
    let mut sec = 0usize;
    // 角色
    if let Some(chars) = report.get("characters").and_then(|c| c.as_array()) {
        if !chars.is_empty() {
            sec += 1;
            md.push_str(&format!("## 一、角色卡（{} 人）\n\n", chars.len()));
            for c in chars {
                let name = s(c, "name");
                if name.is_empty() { continue; }
                let mut card = format!("### {}\n\n", name);
                let role = s(c, "role");
                if !role.is_empty() { card.push_str(&format!("- 身份/定位：{}\n", role)); }
                let personality = s(c, "personality");
                if !personality.is_empty() { card.push_str(&format!("- 性格：{}\n", personality)); }
                let speech = s(c, "speechStyle");
                if !speech.is_empty() { card.push_str(&format!("- 说话风格：{}\n", speech)); }
                let appearance = s(c, "appearance");
                if !appearance.is_empty() { card.push_str(&format!("- 外貌：{}\n", appearance)); }
                let motivation = s(c, "motivation");
                if !motivation.is_empty() { card.push_str(&format!("- 动机：{}\n", motivation)); }
                if let Some(dialogs) = c.get("exampleDialogs").and_then(|d| d.as_array()) {
                    if !dialogs.is_empty() {
                        card.push_str("- 示例对白：\n");
                        for d in dialogs.iter().take(8) {
                            if let Some(t) = d.as_str() {
                                if !t.trim().is_empty() {
                                    card.push_str(&format!("  - 「{}」\n", t.trim()));
                                }
                            }
                        }
                    }
                }
                if let Some(rels) = c.get("relationships").and_then(|r| r.as_array()) {
                    if !rels.is_empty() {
                        card.push_str("- 关系：\n");
                        for r in rels.iter().take(8) {
                            if let Some(t) = r.as_str() {
                                card.push_str(&format!("  - {}\n", t.trim()));
                            }
                        }
                    }
                }
                md.push_str(&card);
                md.push('\n');
            }
        }
    }
    // 世界观
    if let Some(lore) = report.get("lore").and_then(|l| l.as_array()) {
        if !lore.is_empty() {
            sec += 1;
            md.push_str(&format!("## 二、世界观/词条（{} 条）\n\n", lore.len()));
            for l in lore.iter().take(50) {
                let t = s(l, "title");
                if !t.is_empty() {
                    md.push_str(&format!("- {}\n", t));
                }
            }
            md.push('\n');
        }
    }
    // 世界线
    if let Some(wl) = report.get("worldline").and_then(|w| w.as_array()) {
        if !wl.is_empty() {
            sec += 1;
            md.push_str(&format!("## 三、世界线/关键节点（{} 条）\n\n", wl.len()));
            for w in wl.iter().take(50) {
                let t = s(w, "title");
                if !t.is_empty() {
                    md.push_str(&format!("- {}\n", t));
                }
            }
            md.push('\n');
        }
    }
    // 章节/节点
    let node_count = report.get("beats").and_then(|b| b.get("node_count")).and_then(|n| n.as_u64()).unwrap_or(0);
    if node_count > 0 {
        sec += 1;
        md.push_str(&format!("## 四、章节/节点（{} 节点）\n\n", node_count));
        md.push_str(&format!("- 节点数：{}\n", node_count));
        let beat_count = report.get("beats").and_then(|b| b.get("beat_count")).and_then(|n| n.as_u64()).unwrap_or(0);
        md.push_str(&format!("- 硬节拍数：{}\n", beat_count));
        let exits = report.get("exits").and_then(|e| e.as_u64()).unwrap_or(0);
        md.push_str(&format!("- 出口数：{}\n\n", exits));
    }
    // 检定规则
    if let Some(checks) = report.get("rule_checks").and_then(|r| r.as_array()) {
        if !checks.is_empty() {
            sec += 1;
            md.push_str(&format!("## 五、规则检定（{} 个）\n\n", checks.len()));
            for c in checks.iter().take(30) {
                let label = s(c, "label");
                let id = s(c, "id");
                let dice = s(c, "dice");
                if !label.is_empty() {
                    md.push_str(&format!("- {}（{}）骰{}\n", label, id, dice));
                }
            }
            md.push('\n');
        }
    }
    // 事件包
    if let Some(eps) = report.get("event_packages").and_then(|e| e.as_array()) {
        if !eps.is_empty() {
            sec += 1;
            md.push_str(&format!("## 六、事件包（{} 个）\n\n", eps.len()));
            for p in eps.iter().take(30) {
                let name = s(p, "name");
                let id = s(p, "id");
                if !name.is_empty() {
                    md.push_str(&format!("- {}（{}）\n", name, id));
                }
            }
            md.push('\n');
        }
    }
    // 角色卡质量统计
    if let Some(stats) = report.get("character_card_stats").and_then(|c| c.as_array()) {
        if !stats.is_empty() {
            sec += 1;
            md.push_str(&format!("## 七、角色卡质量（{} 张）\n\n", stats.len()));
            md.push_str("| 角色 | 卡片字数 | 证据引用数 |\n|---|---|---|\n");
            for st in stats {
                let name = s(st, "name");
                let chars = st.get("chars_len").and_then(|x| x.as_u64()).unwrap_or(0);
                let refs = st.get("evidence_refs_len").and_then(|x| x.as_u64()).unwrap_or(0);
                md.push_str(&format!("| {} | {} | {} |\n", name, chars, refs));
            }
            md.push('\n');
        }
    }
    // 叙事风格
    let ns = s(report, "narrative_style");
    if !ns.is_empty() {
        sec += 1;
        md.push_str(&format!("## 八、叙事风格\n\n{}\n\n", ns));
    }
    // 缺失关键角色
    if let Some(missing) = report.get("missing_key_characters").and_then(|m| m.as_array()) {
        if !missing.is_empty() {
            sec += 1;
            md.push_str(&format!("## 九、缺失关键角色（{} 个）\n\n", missing.len()));
            for m in missing.iter().take(20) {
                if let Some(t) = m.as_str() {
                    md.push_str(&format!("- {}\n", t));
                }
            }
            md.push('\n');
        }
    }
    // 演员模板
    if let Some(tpls) = report.get("actor_templates").and_then(|t| t.as_array()) {
        if !tpls.is_empty() {
            sec += 1;
            md.push_str(&format!("## 十、角色状态模板（{} 个）\n\n", tpls.len()));
            for t in tpls.iter().take(30) {
                let name = s(t, "name");
                let fc = t.get("field_count").and_then(|x| x.as_u64()).unwrap_or(0);
                if !name.is_empty() {
                    md.push_str(&format!("- {}（{} 字段）\n", name, fc));
                }
            }
            md.push('\n');
        }
    }
    if sec == 0 {
        return None;
    }
    Some(md)
}

/// POST /api/v1/crawler/novels — import TXT/MD onto bookshelf (+ optional Story Pack).
pub async fn novels_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ShelfImportBody>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    // 优先使用 `data`（base64 原始字节）走编码嗅探，避免 GBK/Big5 源 txt 被按 UTF-8 硬解成乱码；
    // 否则回退到调用方已解码好的 `text`。
    let text = match body.data.as_deref() {
        Some(b64) if !b64.trim().is_empty() => {
            use base64::{engine::general_purpose::STANDARD as B64, Engine};
            match B64.decode(b64.trim()) {
                Ok(raw) => decode_text(&raw).0,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"ok": false, "error": "invalid base64 in data"})),
                    )
                        .into_response();
                }
            }
        }
        _ => body.text.clone(),
    };
    let text = text.trim();
    if text.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": "empty text"})),
        )
            .into_response();
    }
    if text.len() > 20_000_000 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"ok": false, "error": "text too large (max 20M chars)"})),
        )
            .into_response();
    }
    // P7: reject script-capable HTML in imported plain text.
    let threats = kaleido_core::inspect_imported_plain_text(text);
    if !threats.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"ok": false, "error": "UNSAFE_IMPORT_CONTENT", "threats": threats})),
        )
            .into_response();
    }
    let title = body
        .title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            text.lines()
                .find(|l| l.starts_with("# "))
                .map(|l| l.trim_start_matches("# ").trim().to_string())
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "未命名".into());
    let chapters = split_novel_chapters(text);
    let (slug, path) = match write_shelf_markdown(&title, text, &chapters) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"ok": false, "error": e})),
            )
                .into_response();
        }
    };
    let mut pack_id: Option<String> = None;
    let do_pack = body.to_pack.unwrap_or(true);
    if do_pack {
        let pid = format!(
            "pack-shelf-{}-{}",
            slug.chars().take(24).collect::<String>(),
            chrono::Utc::now().timestamp() % 100_000
        );
        let (pack, bodies) = build_pack_from_chapters(&title, &chapters, &pid, &slug);
        match state.packs.save(pack) {
            Ok(saved) => {
                for (rel, content) in bodies {
                    let _ = state.packs.write_chapter_body(&saved.id, &rel, &content);
                }
                pack_id = Some(saved.id.clone());
                // 自动角色抽取（异步，不阻塞导入；失败静默）
                spawn_auto_cast_extraction(state.clone(), saved.id);
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"ok": false, "error": format!("pack save: {e}"), "slug": slug})),
                )
                    .into_response();
            }
        }
    }
    Json(json!({
        "ok": true,
        "slug": slug,
        "title": title,
        "chapterCount": chapters.len(),
        "path": path.display().to_string(),
        "packId": pack_id,
    }))
    .into_response()
}

/// POST /api/v1/crawler/novels/{slug}/to-pack — promote shelf novel to Story Tavern pack.
pub async fn novel_to_pack(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    body: Option<Json<ShelfToPackBody>>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let force = body
        .as_ref()
        .and_then(|b| b.force)
        .unwrap_or(false);
    let override_id = body.and_then(|Json(b)| b.pack_id);
    // load content via same path resolution as novel_content
    let shelf = scan_shelf();
    let entry = match shelf.into_iter().find(|e| e.slug == slug) {
        Some(e) => e,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"ok": false, "error": "novel not found"})),
            )
                .into_response();
        }
    };
    if !force {
        if let Some(existing) = find_existing_pack_for_shelf(&state.packs, &slug, &entry.title) {
            return Json(json!({
                "ok": true,
                "packId": existing,
                "title": entry.title,
                "existed": true,
            }))
            .into_response();
        }
    }
    // read file
    let dir = shelf_dir();
    let slug_tag = shelf_slug(&entry.title);
    let file_path = find_shelf_md_file(&dir, &slug_tag, &entry.title);
    let content = match file_path.and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"ok": false, "error": "file not found"})),
            )
                .into_response();
        }
    };
    let chapters = split_novel_chapters(&content);
    let pid = override_id.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| {
        format!(
            "pack-shelf-{}-{}",
            slug.chars().take(24).collect::<String>(),
            chrono::Utc::now().timestamp() % 100_000
        )
    });
    let (pack, bodies) = build_pack_from_chapters(&entry.title, &chapters, &pid, &slug);
    match state.packs.save(pack) {
        Ok(saved) => {
            for (rel, body) in bodies {
                let _ = state.packs.write_chapter_body(&saved.id, &rel, &body);
            }
            Json(json!({
                "ok": true,
                "packId": saved.id,
                "title": saved.title,
                "chapterCount": chapters.len(),
                "existed": false,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}

/// POST /api/v1/crawler/novels/{slug}/distil — 角色蒸馏（LLM + 向量检索）。
///
/// 复用 `novel_to_pack` 的 shelf 解析逻辑（读 shelf md → split_novel_chapters →
/// build_pack_from_chapters 生成基础 pack），随后用 `distill_pack_characters`
/// 蒸馏出有血有肉的角色卡替换 `pack.characters` 并落盘。
pub async fn novel_distil(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    // 读 shelf md（复用 novel_to_pack 的解析逻辑）
    let shelf = scan_shelf();
    let entry = match shelf.into_iter().find(|e| e.slug == slug) {
        Some(e) => e,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"ok": false, "error": "novel not found"})),
            )
                .into_response();
        }
    };
    let dir = shelf_dir();
    let slug_tag = shelf_slug(&entry.title);
    let file_path = find_shelf_md_file(&dir, &slug_tag, &entry.title);
    let content = match file_path.and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"ok": false, "error": "file not found"})),
            )
                .into_response();
        }
    };
    let chapters = split_novel_chapters(&content);
    // 带 chXX 章节号，供证据检索与 evidence_refs 使用
    // A0 完整版 (2026-08-19): 编号用原著章号（缺章自动跳号），与 pack.chapters.id 对齐；
    // 此前用切分序号 i+1，与 build_pack_from_chapters 的编号错位风险（D1）。
    let chapter_ids = chapter_id_seq(&chapters);
    let distil_chapters: Vec<(String, String)> = chapters
        .iter()
        .enumerate()
        .map(|(i, (_t, b))| (chapter_ids[i].clone(), b.clone()))
        .collect();

    // 角色蒸馏（LLM + 向量检索）
    let max_chars = 6000usize;
    // 先确定 pack id（供增量存档：每蒸馏完一个角色立即写回 pack）
    let pid = find_existing_pack_for_shelf(&state.packs, &slug, &entry.title).unwrap_or_else(|| {
        format!(
            "pack-shelf-{}-{}",
            slug.chars().take(24).collect::<String>(),
            chrono::Utc::now().timestamp() % 100_000
        )
    });
    let chars =
        match crate::convert::distill_pack_characters(&state, &entry.title, &distil_chapters, max_chars, Some(&pid))
            .await
        {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(json!({"ok": false, "error": e})),
                )
                    .into_response();
            }
        };

    // 生成基础 pack 结构（章节/节点/正文不变），用蒸馏结果替换角色。
    // [fix] 磁盘已有同 id pack → 以磁盘副本为基底（与 exec_shelf_distil_world 同策略），
    // 仅替换 characters / node.entry 后保存；此前直接拿 build_pack_from_chapters
    // 新建的骨架整体 save，会把该 pack 已蒸馏出的世界树/节拍/多出口/事件包/演员状态清空。
    let (fresh_pack, bodies) = build_pack_from_chapters(&entry.title, &chapters, &pid, &slug);
    let mut pack = state.packs.get(&pid).ok().unwrap_or(fresh_pack);
    pack.characters = chars;
    // 方案A(2026-08-16)：node.entry 叙事入口蒸馏。
    // 回推最初设计：entry 应为叙事入口句（"雨夜，玩家抵达旧茶馆门前"），
    // 而非 build_pack_from_chapters 的占位 "本章开始"。失败仅 warn，不阻主线。
    match crate::convert::distill_node_entries(&state, &entry.title, &pack.nodes, &distil_chapters).await {
        Ok(entries) => {
            for n in &mut pack.nodes {
                if let Some(e) = entries.get(&n.id) {
                    if !e.trim().is_empty() {
                        n.entry = e.trim().to_string();
                    }
                }
            }
        }
        Err(e) => tracing::warn!(err = %e, "节点入口蒸馏失败，保留占位 entry"),
    }
    match state.packs.save(pack) {
        Ok(saved) => {
            for (rel, body) in bodies {
                let _ = state.packs.write_chapter_body(&saved.id, &rel, &body);
            }
            let characters: Vec<Value> = saved
                .characters
                .iter()
                .map(|c| {
                    json!({
                        "name": c.name,
                        "personality": c.personality,
                        "speechStyle": c.speech_style,
                        "exampleDialogs": c.example_dialogs,
                        "boundaries": c.boundaries,
                        "motivation": c.motivation,
                        "relationships": c.relationships,
                        "evidenceRefs": c.evidence_refs,
                        "mentalModels": c.mental_models,
                        "decisionHeuristics": c.decision_heuristics,
                        "beliefs": c.beliefs,
                    })
                })
                .collect();
            Json(json!({
                "ok": true,
                "packId": saved.id,
                "title": saved.title,
                "characterCount": saved.characters.len(),
                "characters": characters,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"ok": false, "error": e.to_string()})),
        )
            .into_response(),
    }
}

/// 分阶段落盘：pack.json 原子写（PackStore::save 内部 temp+rename）+ chapter bodies。
/// bodies 只在 chapters 目录缺失时一次性全量写入（后续阶段只有 pack 变化，不再重写）。
/// 保存失败不 panic，返回 Err 由调用方 push 错误事件（阶段继续，最终 save 覆盖）。
pub(crate) fn save_pack_checkpoint(
    state: &AppState,
    pack: &kaleido_core::StoryPack,
    bodies: &[(String, String)],
) -> Result<(), String> {
    let saved = state.packs.save(pack.clone()).map_err(|e| e.to_string())?;
    let chapters_dir = state
        .app_state
        .data_root()
        .story_packs_dir()
        .join(&saved.id)
        .join("chapters");
    if !chapters_dir.exists() {
        for (rel, body) in bodies {
            let _ = state.packs.write_chapter_body(&saved.id, rel, body);
        }
    }
    Ok(())
}

/// 世界线旁挂文件写盘（temp + rename 原子替换，避免中途写坏 JSON）。
pub(crate) fn write_worldline_atomic(pack_dir: &std::path::Path, worldline: &Value) -> std::io::Result<()> {
    std::fs::create_dir_all(pack_dir)?;
    // 通用兜底：剔除 U+FFFD 坏字残骸（同 story_tavern.rs::write_atomic）。不破坏 JSON 语法。
    let body = serde_json::to_string_pretty(worldline)
        .unwrap_or_else(|_| "[]".to_string())
        .replace('\u{FFFD}', "");
    let tmp = pack_dir.join("worldline.json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, pack_dir.join("worldline.json"))
}

pub(crate) fn worldline_is_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

/// 人工介入控制门：每个阶段 LLM 调用前调用一次。
/// 返回 `true` = 继续执行；返回 `false` = job 已被取消/置终态，exec 应立即停止。
///
/// 逻辑：
/// 1. 若 job status 已非 active（running/queued）→ 说明被 cancel 或置终态 → 停止。
/// 2. 若 control.action == "pause" → 推事件，自旋等待直到 resume 或 job 被取消。
async fn job_control_gate(jobs: &JobStore, run_id: &str) -> bool {
    // 立即取消检查：job 若已被 cancel() → status=cancelled（非 active）→ 停止
    if let Some(rec) = jobs.get(run_id) {
        let st = kaleido_core::normalize_job_status(&rec.status);
        if !kaleido_core::is_active_job_status(&st) {
            return false;
        }
    }
    // 暂停等待：control=="pause" → 自旋直到 resume 或取消
    loop {
        match jobs.control_action(run_id).as_deref() {
            Some("pause") => {
                let _ = jobs.push_event(
                    run_id,
                    JobEvent::progress("已暂停，等待继续…", 0.0),
                    None,
                    None,
                );
                tokio::time::sleep(StdDuration::from_millis(500)).await;
                // 暂停循环中若被取消，立即退出
                if let Some(rec) = jobs.get(run_id) {
                    let st = kaleido_core::normalize_job_status(&rec.status);
                    if !kaleido_core::is_active_job_status(&st) {
                        return false;
                    }
                }
            }
            Some("resume") => {
                let _ = jobs.clear_control(run_id);
                let _ = jobs.push_event(
                    run_id,
                    JobEvent::progress("已恢复", 0.0),
                    None,
                    None,
                );
                return true;
            }
            _ => return true,
        }
    }
}

/// Shelf 世界蒸馏的后台执行体（可复用）。
///
/// 创建任务后的 tokio::spawn 与服务重启后的恢复调度共用本函数：
/// - 开头重建章节与 pack 基底，读取磁盘既有 pack.json —— 磁盘有则以其为工作副本继续，
///   未完成阶段写进同一 pack 对象；已完成阶段直接跳过（幂等续跑）。
/// - 每个阶段完成即落盘一次，任何阶段完成后服务被杀，该阶段产物已持久化。
/// - 保留 push_event 进度；导出错误统一 complete(failed)。

// [R6-2① 2026-10-07] exec_shelf_distil_world 按蒸馏阶段拆分：
// exec.rs（setup+调度+finalize）/ stages.rs（角色/世界/节拍/出口/世界线）/
// stages_assets.rs（关系图谱/事件包/演员状态/文风规则）。
mod exec;
#[allow(unused_imports)]
pub use exec::*;
mod stages;
#[allow(unused_imports)]
pub(crate) use stages::*;
mod stages_assets;
#[allow(unused_imports)]
pub(crate) use stages_assets::*;
