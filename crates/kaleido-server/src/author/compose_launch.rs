//! author/compose_launch —— 组合剧本包（build_composed_pack）+ launch 启动档。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComposeBody {
    #[serde(default)]
    playable: Option<String>,
    #[serde(default)]
    character_ids: Option<Vec<String>>,
    #[serde(default)]
    world_book_ids: Option<Vec<String>>,
    #[serde(default)]
    source_doc_paths: Option<Vec<String>>,
    #[serde(default)]
    title: Option<String>,
    /// If true (default), build/refresh a StoryPack for P3/P2/P4.
    #[serde(default = "default_true")]
    build_pack: Option<bool>,
}

pub(crate) fn default_true() -> Option<bool> {
    Some(true)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LaunchBody {
    #[serde(default)]
    playable: Option<String>,
    #[serde(default)]
    play_mode: Option<String>,
    #[serde(default)]
    adult_confirmed: Option<bool>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    user_tier: Option<String>,
    #[serde(default)]
    entry: Option<EntryConfig>,
    #[serde(default)]
    player_display_name: Option<String>,
    #[serde(default)]
    live_enabled: Option<bool>,
    #[serde(default)]
    live_every_n: Option<u32>,
    #[serde(default)]
    live_write_turns: Option<bool>,
    #[serde(default)]
    live_write_summary_every_n: Option<u32>,
}

pub(crate) fn parse_playable(s: &str) -> Result<Playable, Response> {
    match s.trim().to_uppercase().as_str() {
        "P1" | "" => Ok(Playable::P1),
        "P2" => Ok(Playable::P2),
        "P3" => Ok(Playable::P3),
        "P4" => Ok(Playable::P4),
        _ => Err(bad_request("AUTHOR_BAD_PLAYABLE", format!("invalid playable: {s}"))),
    }
}

pub(crate) fn parse_play_mode(s: Option<&str>, playable: Playable) -> PlayMode {
    match s.map(|x| x.trim().to_ascii_lowercase()).as_deref() {
        Some("mainline") => PlayMode::Mainline,
        Some("side") => PlayMode::Side,
        Some("free") => PlayMode::Free,
        _ => match playable {
            Playable::P3 => PlayMode::Mainline,
            Playable::P4 | Playable::P1 | Playable::P2 => PlayMode::Free,
        },
    }
}

pub(crate) fn parse_tier(s: Option<&str>) -> ContentTier {
    match s.map(|x| x.trim().to_ascii_lowercase()).as_deref() {
        Some("safe") => ContentTier::Safe,
        Some("open") => ContentTier::Open,
        _ => ContentTier::Standard,
    }
}

pub(crate) fn field_str(fields: &Option<serde_json::Value>, key: &str) -> String {
    fields
        .as_ref()
        .and_then(|f| f.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

pub(crate) fn partner_to_pack_char(item: &kaleido_core::PartnerItem) -> PackCharacterRef {
    let fields = &item.fields;
    let personality = field_str(fields, "personality");
    let speech = field_str(fields, "speechStyle");
    let speech = if speech.is_empty() {
        field_str(fields, "speech_style")
    } else {
        speech
    };
    PackCharacterRef {
        id: item.id.clone(),
        name: item.name.clone(),
        role: field_str(fields, "occupation"),
        importance: "medium".into(),
        gender: "未知".into(),
        appearance: "未知".into(),
        opening_scene: "未知".into(),
        opening_lines: "".into(),
        nsfw_profile: String::new(),
        content_tier: Some(ContentTier::Standard),
        example_dialogs: vec![],
        boundaries: vec![],
        personality,
        speech_style: speech,
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
    }
}

pub(crate) fn split_chapters(text: &str) -> Vec<(String, String)> {
    // heuristic: 第x章 / Chapter N / # headings; else two chunks by size
    let t = text.trim();
    if t.is_empty() {
        return vec![
            ("开端".into(), "（空文稿占位）故事从这里开始。".into()),
            ("后续".into(), "（空文稿占位）故事继续。".into()),
        ];
    }
    // line-based chapter split (no regex dep)
    {
        let mut titles: Vec<(usize, String)> = Vec::new();
        let mut offset = 0usize;
        for line in t.split_inclusive('\n') {
            let trimmed = line.trim();
            let is_heading = trimmed.starts_with('#')
                || trimmed.starts_with("第") && trimmed.contains('章')
                || trimmed.to_ascii_lowercase().starts_with("chapter ");
            if is_heading {
                let title = trimmed.trim_start_matches('#').trim().to_string();
                titles.push((offset, title));
            }
            offset += line.len();
        }
        if titles.len() >= 2 {
            let mut out = Vec::new();
            for i in 0..titles.len() {
                let start = titles[i].0;
                let end = if i + 1 < titles.len() {
                    titles[i + 1].0
                } else {
                    t.len()
                };
                let body = t[start..end].trim().to_string();
                if !body.is_empty() {
                    out.push((titles[i].1.clone(), body));
                }
            }
            if out.len() >= 2 {
                return out.into_iter().take(12).collect();
            }
        }
    }
    // fallback: split roughly in half by chars
    let mid = t.chars().count() / 2;
    let mut acc = 0;
    let mut split_at = t.len() / 2;
    for (i, _) in t.char_indices() {
        if acc >= mid {
            split_at = i;
            break;
        }
        acc += 1;
    }
    // prefer newline near mid
    if let Some(rel) = t[split_at..].find('\n') {
        split_at += rel + 1;
    }
    let a = t[..split_at].trim().to_string();
    let b = t[split_at..].trim().to_string();
    vec![
        ("第一章".into(), if a.is_empty() { t.chars().take(800).collect() } else { a }),
        (
            "第二章".into(),
            if b.is_empty() {
                "故事仍在继续。".into()
            } else {
                b
            },
        ),
    ]
}

pub(crate) fn build_composed_pack(
    project: &AuthorProject,
    title: &str,
    chars: Vec<PackCharacterRef>,
    world_book_ids: Vec<String>,
    chapter_bodies: Vec<(String, String)>,
) -> StoryPack {
    let now = now();
    let pack_id = format!("pack-az-{}", Uuid::new_v4());
    let mut chapters = Vec::new();
    let mut nodes = Vec::new();
    let char_ids: Vec<String> = chars.iter().map(|c| c.id.clone()).collect();
    let n = chapter_bodies.len().max(2);
    for (i, (ch_title, body)) in chapter_bodies.into_iter().enumerate() {
        let order = (i + 1) as u32;
        let ch_id = format!("ch{:02}", order);
        let node_id = format!("n{order}");
        let body_path = format!("chapters/{ch_id}.md");
        let summary: String = body.chars().take(200).collect();
        chapters.push(StoryChapter {
            id: ch_id.clone(),
            title: ch_title,
            order,
            goals: vec![format!("推进{ch_id}")],
            node_ids: vec![node_id.clone()],
            body_path: body_path.clone(),
            image_path: String::new(), // U10
        });
        let mut exits = vec![];
        if i + 1 < n {
            exits.push(NodeExit {
                id: format!("e{order}"),
                when: "continue".into(),
                next: format!("n{}", order + 1),
            });
        }
        nodes.push(StoryNode {
            id: node_id,
            chapter_id: ch_id,
            title: format!("节点{order}"),
            entry: summary.clone(),
            exit: exits,
            locked_beats: vec![],
            allowed_divergence: "branch".into(),
            present_characters: char_ids.clone(),
            location_id: None,
            summary,
        });
    }
    // ensure >=2 chapters
    while chapters.len() < 2 {
        let order = (chapters.len() + 1) as u32;
        let ch_id = format!("ch{:02}", order);
        let node_id = format!("n{order}");
        chapters.push(StoryChapter {
            id: ch_id.clone(),
            title: format!("第{order}章"),
            order,
            goals: vec![],
            node_ids: vec![node_id.clone()],
            body_path: format!("chapters/{ch_id}.md"),
            image_path: String::new(), // U10
        });
        nodes.push(StoryNode {
            id: node_id,
            chapter_id: ch_id,
            title: format!("节点{order}"),
            entry: "占位".into(),
            exit: vec![],
            locked_beats: vec![],
            allowed_divergence: "branch".into(),
            present_characters: char_ids.clone(),
            location_id: None,
            summary: "占位".into(),
        });
    }
    // link last gap if needed
    if nodes.len() >= 2 {
        for i in 0..nodes.len() - 1 {
            if nodes[i].exit.is_empty() {
                let next = nodes[i + 1].id.clone();
                nodes[i].exit.push(NodeExit {
                    id: format!("auto-{i}"),
                    when: "continue".into(),
                    next,
                });
            }
        }
    }

    StoryPack {
        id: pack_id,
        title: title.to_string(),
        source: PackSource {
            source_type: "author-compose".into(),
            refs: vec![project.id.clone()],
        },
        characters: chars,
        world_book_ids,
        chapters,
        nodes,
        lore_entries: vec![],
        event_packages: vec![],
        actor_state_config: kaleido_core::ActorStatePackConfig::default(),
        default_mode: PlayMode::Mainline,
        max_tier: ContentTier::Standard,
        language: "zh".into(),
        created_at: now.clone(),
        updated_at: now,
        stage_director: Default::default(),
        worldline: vec![], // T 层：作者流 pack 无静态时间线蒸馏，空
    }
}

pub(crate) async fn compose_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ComposeBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let id = match safe_project_id(&id) {
        Ok(i) => i,
        Err(r) => return r,
    };

    // Hold index lock for entire compose RMW (prevents lost updates under concurrent create/compose).
    let _index_guard = AUTHOR_INDEX_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let mut list = match load_index_unlocked(&state) {
        Ok(l) => l,
        Err(r) => return r,
    };
    let Some(pos) = list.iter().position(|p| {
        p.id == id && (p.workspace_id.is_empty() || p.workspace_id == session.workspace_id)
    }) else {
        return not_found("AUTHOR_NOT_FOUND", format!("project not found: {id}"));
    };

    let playable_s = body
        .playable
        .clone()
        .or_else(|| list[pos].default_playable.clone())
        .unwrap_or_else(|| "P1".into());
    let playable = match parse_playable(&playable_s) {
        Ok(p) => p,
        Err(r) => return r,
    };

    // merge character / worldbook ids onto project
    if let Some(ids) = body.character_ids.clone() {
        list[pos].character_ids = ids;
    }
    if let Some(ids) = body.world_book_ids.clone() {
        list[pos].world_book_ids = ids;
    }
    if let Some(t) = body.title.clone() {
        let t = t.trim().to_string();
        if !t.is_empty() {
            list[pos].title = t;
        }
    }
    list[pos].default_playable = Some(match playable {
        Playable::P1 => "P1",
        Playable::P2 => "P2",
        Playable::P3 => "P3",
        Playable::P4 => "P4",
    }.into());
    list[pos].updated_at = now();

    // validate partner assets when ids present (C2: per-user isolation)
    let partner = match state.partner.clone().scoped(&session.user_id).load() {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let mut pack_chars: Vec<PackCharacterRef> = Vec::new();
    for cid in &list[pos].character_ids {
        if let Some(cc) = partner.character_cards.iter().find(|c| &c.id == cid) {
            pack_chars.push(partner_to_pack_char(cc));
        } else {
            // allow unknown ids as soft refs (compose still works for demos)
            pack_chars.push(PackCharacterRef {
                id: cid.clone(),
                name: cid.clone(),
                role: "character".into(),
                importance: "medium".into(),
                gender: "未知".into(),
                appearance: "未知".into(),
                opening_scene: "未知".into(),
                opening_lines: "".into(),
                nsfw_profile: String::new(),
                content_tier: Some(ContentTier::Standard),
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
    }
    for wid in &list[pos].world_book_ids {
        if partner.world_books.iter().all(|w| &w.id != wid) {
            // soft allow
        }
    }

    match playable {
        Playable::P1 if list[pos].character_ids.is_empty() && pack_chars.is_empty() => {
            return bad_request("AUTHOR_COMPOSE_EMPTY",  "P1 compose requires >=1 characterId");
        }
        Playable::P2 if list[pos].character_ids.len() < 2 => {
            return bad_request("AUTHOR_COMPOSE_EMPTY",  "P2 compose requires >=2 characterIds");
        }
        Playable::P3 if list[pos].character_ids.is_empty() => {
            return bad_request("AUTHOR_COMPOSE_EMPTY",  "P3 compose requires >=1 characterId");
        }
        Playable::P4
            if list[pos].character_ids.is_empty()
                && list[pos].world_book_ids.is_empty()
                && body
                    .source_doc_paths
                    .as_ref()
                    .map(|v| v.is_empty())
                    .unwrap_or(true) =>
        {
            return bad_request(
                "AUTHOR_COMPOSE_EMPTY",
                "P4 compose requires worldBookId and/or sourceDocPaths (BG) or >=1 characterId",
            );
        }
        _ => {}
    }

    let build_pack = body.build_pack.unwrap_or(true);
    // AZ residual: all playables (incl. P1) build a StoryPack on compose so
    // publish lore/chapter has a packId without a separate launch.
    let need_pack = build_pack;

    let mut pack_id = list[pos].pack_id.clone();
    let mut pack_playable = false;

    if need_pack || matches!(playable, Playable::P3) {
        // gather chapter text from source docs or placeholder
        let mut combined = String::new();
        if let Some(paths) = &body.source_doc_paths {
            for path in paths {
                match state.works.read_text(&session.workspace_id, path) {
                    Ok(f) => {
                        combined.push_str(&f.content);
                        combined.push_str("\n\n");
                    }
                    Err(e) => {
                        return bad_request("AUTHOR_SOURCE_DOC_READ", format!("sourceDoc read failed {path}: {e}"));
                    }
                }
            }
        }
        // also try project imports/
        if combined.trim().is_empty() {
            let import_dir = format!("projects/{id}/imports");
            if let Ok(listing) = state.works.list(&session.workspace_id, &import_dir, 1) {
                for ch in listing.children {
                    if ch.kind == "file" {
                        if let Ok(f) = state.works.read_text(&session.workspace_id, &ch.path) {
                            combined.push_str(&f.content);
                            combined.push_str("\n\n");
                        }
                    }
                }
            }
        }
        if pack_chars.is_empty() {
            // P3 needs characters — already validated non-empty ids
        }
        let chapters = split_chapters(&combined);
        // stash bodies for write
        let chapter_bodies = chapters.clone();
        let mut pack = build_composed_pack(
            &list[pos],
            &list[pos].title,
            pack_chars.clone(),
            list[pos].world_book_ids.clone(),
            chapters,
        );
        // ensure playable: >=1 char, >=2 ch, linked nodes
        if pack.characters.is_empty() {
            pack.characters.push(PackCharacterRef {
                id: "cc-anon".into(),
                name: "旅人".into(),
                role: "player".into(),
                importance: "low".into(),
                gender: "未知".into(),
                appearance: "未知".into(),
                opening_scene: "未知".into(),
                opening_lines: "".into(),
                nsfw_profile: String::new(),
                content_tier: Some(ContentTier::Standard),
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
        match state.packs.save(pack.clone()) {
            Ok(saved) => {
                for (i, (_title, body)) in chapter_bodies.iter().enumerate() {
                    let order = (i + 1) as u32;
                    let rel = format!("chapters/ch{:02}.md", order);
                    let content = if body.trim().is_empty() {
                        format!("# {}\n\n（作者组合占位章）\n", _title)
                    } else if body.starts_with('#') {
                        body.clone()
                    } else {
                        format!("# {}\n\n{}\n", _title, body)
                    };
                    let _ = state.packs.write_chapter_body(&saved.id, &rel, &content);
                }
                // pad missing chapter files to 2
                for order in 1..=saved.chapters.len().max(2) {
                    let rel = format!("chapters/ch{:02}.md", order);
                    let _ = state.packs.write_chapter_body(
                        &saved.id,
                        &rel,
                        &format!("# 第{order}章\n\n（占位）\n"),
                    );
                }
                pack_playable = saved.is_playable();
                pack_id = Some(saved.id.clone());
                list[pos].pack_id = Some(saved.id);
            }
            Err(e) => return map_core_err(e),
        }
    } else if matches!(playable, Playable::P1) && list[pos].pack_id.is_none() {
        // P1 can use demo pack as lightweight stage if desired — leave pack empty;
        // launch will use ensure_demo when pack missing for non-P1 only.
    }

    list[pos].updated_at = now();
    let project = list[pos].clone();
    if let Err(r) = write_project_mirror(&state.works, &session.workspace_id, &project) {
        return r;
    }
    if let Err(r) = save_index_unlocked(&state, &list) {
        return r;
    }

    let launch = json!({
        "playable": match playable {
            Playable::P1 => "P1",
            Playable::P2 => "P2",
            Playable::P3 => "P3",
            Playable::P4 => "P4",
        },
        "packId": pack_id,
        "characterIds": project.character_ids,
        "worldBookIds": project.world_book_ids,
        "projectId": project.id,
        "playablePack": pack_playable,
    });

    Json(json!({
        "ok": true,
        "projectId": project.id,
        "project": project,
        "packId": pack_id,
        "launch": launch,
    }))
    .into_response()
}

pub(crate) async fn launch_project(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<LaunchBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let id = match safe_project_id(&id) {
        Ok(i) => i,
        Err(r) => return r,
    };
    let list = match load_index(&state) {
        Ok(l) => filter_workspace(l, &session.workspace_id),
        Err(r) => return r,
    };
    let Some(project) = list.iter().find(|p| p.id == id) else {
        return not_found("AUTHOR_NOT_FOUND", format!("project not found: {id}"));
    };

    let playable_s = body
        .playable
        .clone()
        .or_else(|| project.default_playable.clone())
        .unwrap_or_else(|| "P1".into());
    let playable = match parse_playable(&playable_s) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let play_mode = parse_play_mode(body.play_mode.as_deref(), playable);
    let user_tier = parse_tier(body.user_tier.as_deref());

    // resolve pack
    let mut pack_id = project.pack_id.clone();
    if pack_id.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
        // P1 without pack: use demo so session engine has a stage
        match state.packs.ensure_demo_pack() {
            Ok(p) => pack_id = Some(p.id),
            Err(e) => return map_core_err(e),
        }
    }
    let pack_id = pack_id.unwrap();

    let title = body
        .title
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| format!("{} · launch", project.title));

    let req = CreateSessionRequest {
        pack_id: pack_id.clone(),
        playable,
        play_mode,
        user_tier,
        global_tier: None,
        entry: body.entry.clone(),
        player_display_name: body.player_display_name.clone(),
        adult_confirmed: body.adult_confirmed.unwrap_or(true),
        title: Some(title),
        quality: kaleido_core::Quality::Lite,
        owner: Some(session.user_id.clone()),
        author_project_id: Some(project.id.clone()),
        // R6: launch 传 work_id（= author project id），U13 自动挂载该作品的罗盘
        work_id: Some(project.id.clone()),
    };

    let mut tavern = match state.sessions_tavern.create_from_pack(&state.packs, req) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };

    // ensure live path + session dir (same as bind)
    let live_path = format!(
        "projects/{}/sessions/{}/live.md",
        project.id, tavern.session_id
    );
    let sess_dir = format!("projects/{}/sessions/{}", project.id, tavern.session_id);
    let _ = state.works.mkdir(&session.workspace_id, &sess_dir);
    if state.works.stat(&session.workspace_id, &live_path).is_err() {
        let header = format!(
            "# {} · live\n\n> session `{}` · project `{}` · playable {:?}\n\n",
            project.title, tavern.session_id, project.id, tavern.playable
        );
        let _ = state
            .works
            .write_text(&session.workspace_id, &live_path, &header);
    }
    tavern.author_project_id = Some(project.id.clone());
    tavern.author_live_path = Some(live_path.clone());
    // present characters from project when pack has generic cast
    if !project.character_ids.is_empty() {
        tavern.present_character_ids = project.character_ids.clone();
        tavern.focus_character_id = project.character_ids.first().cloned();
    }
    // AZ-6 playable tactics
    match playable {
        Playable::P2 => {
            tavern.speaker_rotation = true;
            if tavern.present_character_ids.len() < 2 {
                return bad_request("AUTHOR_LAUNCH_ARITY",  "P2 launch requires >=2 characterIds on project");
            }
        }
        Playable::P4 => {
            tavern.speaker_rotation = tavern.present_character_ids.len() >= 2;
        }
        Playable::P1 => {
            tavern.speaker_rotation = false;
        }
        Playable::P3 => {
            tavern.speaker_rotation = true;
        }
    }

    // AZ-6 live policy (project + launch overrides)
    let mut pol = project.live_policy.clone().normalize();
    if let Some(v) = body.live_enabled {
        pol.enabled = v;
    }
    if let Some(v) = body.live_every_n {
        pol.every_n = if v == 0 { 1 } else { v };
    }
    if let Some(v) = body.live_write_turns {
        pol.write_turns = v;
    }
    if let Some(v) = body.live_write_summary_every_n {
        pol.write_summary_every_n = if v == 0 { None } else { Some(v) };
    }
    tavern.author_live_enabled = pol.enabled;
    tavern.author_live_every_n = pol.every_n.max(1);
    // turn slices + summary refresh share write_turns gate; summary cadence uses every_n
    tavern.author_live_write_turns = pol.write_turns || pol.write_summary_every_n.is_some();
    if let Some(sn) = pol.write_summary_every_n {
        // if only summary configured, still honor live every_n for live.md
        let _ = sn;
    }

    let saved = match state.sessions_tavern.save(tavern) {
        Ok(s) => s,
        Err(e) => return map_core_err(e),
    };

    // docIndex touch
    let _index_guard = AUTHOR_INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut full = match load_index_unlocked(&state) {
        Ok(l) => l,
        Err(r) => return r,
    };
    if let Some(p) = full.iter_mut().find(|p| p.id == id) {
        let entry = AuthorDocEntry {
            path: live_path.clone(),
            kind: "live".into(),
            source_session_id: Some(saved.session_id.clone()),
            playable: Some(match saved.playable {
                Playable::P1 => "P1".into(),
                Playable::P2 => "P2".into(),
                Playable::P3 => "P3".into(),
                Playable::P4 => "P4".into(),
            }),
            updated_at: now(),
        };
        if let Some(ex) = p.doc_index.iter_mut().find(|d| d.path == live_path) {
            *ex = entry;
        } else {
            p.doc_index.push(entry);
        }
        p.updated_at = now();
        let _ = write_project_mirror(&state.works, &session.workspace_id, p);
    }
    let _ = save_index_unlocked(&state, &full);

    Json(json!({
        "ok": true,
        "sessionId": saved.session_id,
        "session": saved,
        "projectId": id,
        "packId": pack_id,
        "authorLivePath": live_path,
    }))
    .into_response()
}


// ─── AZ-5 publish / inject ───────────────────────────────────────────────────
