#![allow(unused_imports)]
use crate::{
    st_compass::{Compass, CompassStore},
    st_skimming::SkimIssue,
    text_hash, CoreError, CoreResult, DataRoot,
};
use crate::world_state::{EntityKind as WsEntityKind, WorldEntity, WorldEvent, WorldState};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};
use uuid::Uuid;
use super::*;

#[cfg(test)]
mod opening_tests {
    use super::super::*;

    fn session_with_focus(focus: Option<&str>, present: &[&str]) -> TavernSession {
        TavernSession {
            session_id: "t".into(),
            pack_id: "p".into(),
            pack_missing: false,
            owner: None,
            quality: Quality::Lite,
            playable: Playable::P2,
            play_mode: PlayMode::Mainline,
            content_tier: ContentTier::Standard,
            user_tier_request: ContentTier::Standard,
            entry: EntryConfig::default(),
            chapter_cursor: None,
            node_id: None,
            resume_node_id: None,
            opening_seeded: false,
            side_branch_node_id: None,
            side_branch_label: None,
            current_worldline_id: None,
            last_restored_save_id: None,
            panels: vec![],
            mcp_tool_results: vec![],
            skill_load: None,
            timeline_id: "main".into(),
            turn: 0,
            present_character_ids: present.iter().map(|s| s.to_string()).collect(),
            focus_character_id: focus.map(|s| s.to_string()),
            speaker_rotation: false,
            player: PlayerState::default(),
            memory_l1: MemoryL1::default(),
            memory_l2: MemoryL2::default(),
            memory_l3: MemoryL3::default(),
            memory_l4: MemoryL4::default(),
            guard_events: vec![],
            messages: vec![],
            active_run_id: None,
            adult_confirmed: true,
            title: "t".into(),
            created_at: String::new(),
            updated_at: String::new(),
            author_project_id: None,
            author_live_path: None,
            author_live_enabled: true,
            author_live_every_n: 1,
            author_live_write_turns: false,
            actor_states: ActorStateSystem::default(),
            director_plan: None,
            director_pending: false,
            director_task: None,
            last_event: None,
            last_check_results: vec![],
            check_history: vec![],
            checkpoints: vec![],
            fold_through_turn: 0,
            epoch: 0,
            epoch_last_turn: None,
            epoch_last_chars: None,
            turn_cost_ledger: TurnCostLedger::default(),
            last_turn_diagnostic: None,
            scribe_whispers: Default::default(),
            skim_issues: Vec::new(),
            skim_sample: String::new(),
            chapter_diaries: Vec::new(),
            turn_progress: None,
            diary_config: None,
            pockets: Default::default(),
            pockets_enabled: true,
            needs: Default::default(),
            growth: Default::default(),
            world_climate: Default::default(),
            chaos: Default::default(),
            milestones: Default::default(),
            objectives: Default::default(),
            ambitions: Default::default(),
            dream: Default::default(),
            episodes: Default::default(),
            journal: Default::default(),
            relationships: Default::default(),
            pending_swipes: Default::default(),
            promises: Default::default(),
            preferences: Default::default(),
            presence: Default::default(),
            timed_world_info: Default::default(),
            event_extract: true,
            world: WorldState::default(),
            game_clock: Default::default(),
        }
    }

    fn char_with_opening(id: &str, name: &str) -> PackCharacterRef {
        PackCharacterRef {
            id: id.into(),
            name: name.into(),
            role: String::new(),
            gender: String::new(),
            appearance: String::new(),
            opening_scene: format!("雨夜的小巷，灯影斑驳，{name} 靠在墙边。"),
            opening_lines: format!("「{name}在这里等你很久了。」她把玩着伞柄，目光在你脸上流连。"),
            nsfw_profile: "接吻以上→nsfw".into(),
            importance: "high".into(),
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
            expressions: std::collections::HashMap::new(),
            voice: None,
            archive: None,
            avatar: None,
                starting_wardrobe: Default::default(),
        }
    }

    fn pack_with_title(title: &str) -> StoryPack {
        StoryPack {
            id: "p".into(),
            title: title.into(),
            source: PackSource::default(),
            characters: vec![],
            world_book_ids: vec![],
            chapters: vec![],
            nodes: vec![],
            lore_entries: vec![],
            default_mode: PlayMode::Mainline,
            max_tier: ContentTier::Standard,
            language: "zh".into(),
            created_at: String::new(),
            updated_at: String::new(),
            stage_director: StageDirectorConfig::default(),
            event_packages: vec![],
            actor_state_config: ActorStatePackConfig::default(),
            worldline: vec![],
        }
    }

    /// 无开场字段的角色卡 → 回退通用模板，不破坏既有行为。
    #[test]
    fn mainline_opening_falls_back_when_no_scene() {
        let mut pack = pack_with_title("测试世界");
        // c1 无开场字段；c2 在场但在场名字列在 c1 之后且无开场
        pack.characters = vec![PackCharacterRef {
            id: "c1".into(),
            name: "路人".into(),
            opening_scene: String::new(),
            opening_lines: String::new(),
            ..char_with_opening("c1", "路人")
        }];
        let s = session_with_focus(Some("c1"), &["c1"]);
        let (body, _opts) = build_mainline_opening(&pack, &s);
        assert!(body.contains("你踏入《测试世界》的世界"), "应回退通用模板, got: {body}");
    }

    /// P14 演出层 L3a：焦点角色带 opening_scene/opening_lines 时，个性化 first_mes 置顶进入开场。
    #[test]
    fn mainline_opening_consumes_personalized_opening() {
        let mut pack = pack_with_title("测试世界");
        pack.characters = vec![char_with_opening("c1", "林九")];
        let s = session_with_focus(Some("c1"), &["c1"]);
        let (body, _opts) = build_mainline_opening(&pack, &s);
        assert!(body.contains("雨夜的小巷"), "开场应含个性化场景画布: {body}");
        assert!(body.contains("在这里等你很久了"), "开场应含个性化 first_mes: {body}");
        assert!(body.contains("你踏入《测试世界》的世界"), "通用模板仍应保留作为补充: {body}");
    }

    /// P14 演出层 L3a：仅 opening_scene（无 opening_lines）也进入开场，不强制配对。
    #[test]
    fn mainline_opening_scene_only_consumed() {
        let mut pack = pack_with_title("测试世界");
        let mut c = char_with_opening("c1", "林九");
        c.opening_lines = String::new();
        pack.characters = vec![c];
        let s = session_with_focus(Some("c1"), &["c1"]);
        let (body, _opts) = build_mainline_opening(&pack, &s);
        assert!(body.contains("雨夜的小巷"), "仅场景画布也应进入开场: {body}");
        assert!(!body.contains("在这里等你很久了"), "空 opening_lines 不应出现: {body}");
    }

    /// P1/C1: entry 占位（"本章开始"）→ 回退 node.summary 前 280 字（含真实场景），
    /// 不让 LLM 靠标题脑补（度蜜月开幕跳酒店根因链第③环）。
    #[test]
    fn mainline_opening_placeholder_entry_falls_back_to_summary() {
        let mut pack = pack_with_title("测试世界");
        pack.nodes = vec![StoryNode {
            id: "n1".into(),
            chapter_id: "ch01".into(),
            title: "第一章 相遇".into(),
            entry: "本章开始".into(),
            exit: vec![],
            locked_beats: vec![],
            allowed_divergence: "branch".into(),
            present_characters: vec![],
            location_id: None,
            summary: "午后的阳光穿过教学楼旁那排老樟树的枝叶。".into(),
        }];
        let mut s = session_with_focus(Some("c1"), &["c1"]);
        s.node_id = Some("n1".into());
        s.chapter_cursor = Some("ch01".into());
        let (body, _opts) = build_mainline_opening(&pack, &s);
        assert!(!body.contains("本章开始"), "占位 entry 不应进开场: {body}");
        assert!(body.contains("老樟树"), "应回退 summary 真实场景: {body}");
    }

    /// P1/C1: 有实质内容的 entry（非占位）保持原样不回归。
    #[test]
    fn mainline_opening_real_entry_kept() {
        let mut pack = pack_with_title("测试世界");
        pack.nodes = vec![StoryNode {
            id: "n1".into(),
            chapter_id: "ch01".into(),
            title: "第一章 相遇".into(),
            entry: "午后，学校后门，你收到父亲调课的消息。".into(),
            exit: vec![],
            locked_beats: vec![],
            allowed_divergence: "branch".into(),
            present_characters: vec![],
            location_id: None,
            summary: "无关摘要".into(),
        }];
        let mut s = session_with_focus(Some("c1"), &["c1"]);
        s.node_id = Some("n1".into());
        s.chapter_cursor = Some("ch01".into());
        let (body, _opts) = build_mainline_opening(&pack, &s);
        assert!(body.contains("调课的消息"), "实质 entry 应保留: {body}");
        assert!(!body.contains("无关摘要"), "不应误回退 summary: {body}");
    }
}

/// Seed opening monologue if session has no messages yet. Returns true if seeded.
pub fn seed_opening_if_needed(session: &mut TavernSession, pack: &StoryPack) -> bool {
    if session.opening_seeded {
        return false;
    }
    if !session.messages.is_empty() {
        // Already has history (imported / repaired) — mark seeded so we don't inject mid-chat.
        session.opening_seeded = true;
        return false;
    }
    let (content, options) = if session.play_mode == PlayMode::Side {
        if let Some(sid) = session.side_branch_node_id.clone() {
            build_side_opening(pack, session, &sid)
        } else if let Some(nid) = session.node_id.clone() {
            build_side_opening(pack, session, &nid)
        } else {
            build_mainline_opening(pack, session)
        }
    } else {
        build_mainline_opening(pack, session)
    };
    session.messages.push(TavernMessage {
        id: format!("msg-opening-{}", Uuid::new_v4()),
        role: "assistant".into(),
        content,
        created_at: Utc::now().to_rfc3339(),
        options,
        engine_tag: None,
        program: None,
        reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
    });
    session.opening_seeded = true;
    // Opening is not a player turn; keep turn at 0.
    true
}

/// Enter a side branch at node_id: stash resume, switch mode, set cursor, seed side opening.
pub fn enter_side_branch(
    session: &mut TavernSession,
    pack: &StoryPack,
    node_id: &str,
) -> CoreResult<()> {
    let node = pack
        .nodes
        .iter()
        .find(|n| n.id == node_id)
        .ok_or_else(|| CoreError::NotFound(format!("node not found: {node_id}")))?;
    if session.play_mode != PlayMode::Side {
        if session.resume_node_id.is_none() {
            session.resume_node_id = session.node_id.clone();
        }
        session.play_mode = PlayMode::Side;
    } else if session.resume_node_id.is_none() {
        session.resume_node_id = session.node_id.clone();
    }
    session.side_branch_node_id = Some(node.id.clone());
    session.side_branch_label = Some(node.title.clone());
    session.node_id = Some(node.id.clone());
    session.chapter_cursor = Some(node.chapter_id.clone());
    if !node.present_characters.is_empty() {
        session.present_character_ids = node.present_characters.clone();
        session.focus_character_id = node.present_characters.first().cloned();
    }
    let (content, options) = build_side_opening(pack, session, &node.id);
    session.messages.push(TavernMessage {
        id: format!("msg-side-opening-{}", Uuid::new_v4()),
        role: "assistant".into(),
        content,
        created_at: Utc::now().to_rfc3339(),
        options,
        engine_tag: None,
        program: None,
        reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
    });
    session.opening_seeded = true;
    Ok(())
}

impl TavernSessionStore {
    pub fn new(data: DataRoot) -> Self {
        let _ = data.ensure_layout();
        Self {
            data,
            lock: std::sync::Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn dir(&self) -> PathBuf {
        self.data.tavern_sessions_dir()
    }

    pub(crate) fn path_for(&self, session_id: &str) -> CoreResult<PathBuf> {
        let id = safe_id(session_id)?;
        if !id.starts_with("tavern-session-") {
            return Err(CoreError::BadRequest(
                "session id must start with tavern-session-".into(),
            ));
        }
        Ok(self.dir().join(format!("{id}.json")))
    }

    pub fn list(&self) -> CoreResult<Vec<Value>> {
        let _g = self.lock.lock();
        fs::create_dir_all(self.dir())?;
        let mut out = Vec::new();
        for ent in fs::read_dir(self.dir())? {
            let ent = ent?;
            let name = ent.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") {
                continue;
            }
            match fs::read_to_string(ent.path()) {
                Ok(raw) => {
                    if let Ok(s) = serde_json::from_str::<TavernSession>(&raw) {
                        out.push(json!({
                            "sessionId": s.session_id,
                            "packId": s.pack_id,
                            "packMissing": s.pack_missing,
                            "playable": s.playable,
                            "playMode": s.play_mode,
                            "contentTier": s.content_tier,
                            "title": s.title,
                            "turn": s.turn,
                            "updatedAt": s.updated_at,
                            "createdAt": s.created_at,
                        }));
                    }
                }
                Err(_) => continue,
            }
        }
        out.sort_by(|a, b| {
            let au = a.get("updatedAt").and_then(|v| v.as_str()).unwrap_or("");
            let bu = b.get("updatedAt").and_then(|v| v.as_str()).unwrap_or("");
            bu.cmp(au)
        });
        Ok(out)
    }

    /// F1: list only sessions owned by `user_id`. Legacy sessions with
    /// `owner == None` are excluded (not attributed to any current user).
    pub fn list_owned(&self, user_id: &str) -> CoreResult<Vec<Value>> {
        let _g = self.lock.lock();
        fs::create_dir_all(self.dir())?;
        let mut out = Vec::new();
        for ent in fs::read_dir(self.dir())? {
            let ent = ent?;
            let name = ent.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") {
                continue;
            }
            match fs::read_to_string(ent.path()) {
                Ok(raw) => {
                    if let Ok(s) = serde_json::from_str::<TavernSession>(&raw) {
                        if s.owner.as_deref() != Some(user_id) {
                            continue;
                        }
                        out.push(json!({
                            "sessionId": s.session_id,
                            "packId": s.pack_id,
                            "packMissing": s.pack_missing,
                            "playable": s.playable,
                            "playMode": s.play_mode,
                            "contentTier": s.content_tier,
                            "title": s.title,
                            "turn": s.turn,
                            "updatedAt": s.updated_at,
                            "createdAt": s.created_at,
                        }));
                    }
                }
                Err(_) => continue,
            }
        }
        out.sort_by(|a, b| {
            let au = a.get("updatedAt").and_then(|v| v.as_str()).unwrap_or("");
            let bu = b.get("updatedAt").and_then(|v| v.as_str()).unwrap_or("");
            bu.cmp(au)
        });
        Ok(out)
    }

    /// F1: check that `user_id` owns the session. Returns the session if
    /// authorized, or `Forbidden` / `NotFound` otherwise. Legacy sessions
    /// (`owner == None`) are denied to all users except the one who saves
    /// (which stamps the owner). This is a conservative default: old data
    /// with no owner is not silently exposed to arbitrary users.
    pub fn get_for_owner(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> CoreResult<TavernSession> {
        let sess = self.get(session_id)?;
        match &sess.owner {
            Some(uid) if uid == user_id => Ok(sess),
            _ => Err(CoreError::Forbidden(format!(
                "session not owned by user: {session_id}"
            ))),
        }
    }

    pub fn get(&self, session_id: &str) -> CoreResult<TavernSession> {
        let _g = self.lock.lock();
        let path = self.path_for(session_id)?;
        if !path.exists() {
            return Err(CoreError::NotFound(format!(
                "session not found: {session_id}"
            )));
        }
        let raw = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, session: TavernSession) -> CoreResult<TavernSession> {
        self.save_inner(session, None)
    }

    /// F2: Atomic turn acquisition — check active_run_id is empty AND set it
    /// to `run_id` in a single locked transaction. If the session already has
    /// an active_run_id, returns `Conflict`. This prevents the race where two
    /// concurrent requests both see `active_run_id == None` and proceed to
    /// spawn duplicate LLM workers.
    ///
    /// Returns the updated session on success.
    pub fn acquire_turn(
        &self,
        session_id: &str,
        run_id: &str,
    ) -> CoreResult<TavernSession> {
        let _g = self.lock.lock();
        let path = self.path_for(session_id)?;
        if !path.exists() {
            return Err(CoreError::NotFound(format!(
                "session not found: {session_id}"
            )));
        }
        let raw = fs::read_to_string(&path)?;
        let mut sess: TavernSession = serde_json::from_str(&raw)?;

        // Reject if already has an active run.
        if sess.active_run_id.is_some() {
            return Err(CoreError::Conflict(format!(
                "turn already in progress: {:?}",
                sess.active_run_id
            )));
        }

        sess.active_run_id = Some(run_id.to_string());
        sess.updated_at = now_rfc3339();
        write_atomic(&path, &serde_json::to_string(&sess)?)?;
        Ok(sess)
    }

    /// F2: Atomic turn release — clear active_run_id if it matches `run_id`.
    /// Uses a single locked transaction to prevent races with acquire_turn.
    /// If `run_id` is None, no-op. If active_run_id doesn't match, no-op.
    pub fn release_turn(
        &self,
        session_id: &str,
        run_id: Option<&str>,
    ) {
        let run_id = match run_id {
            Some(r) => r,
            None => return,
        };
        let _g = self.lock.lock();
        let path = match self.path_for(session_id) {
            Ok(p) => p,
            Err(_) => return,
        };
        if !path.exists() {
            return;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            return;
        };
        let Ok(mut sess) = serde_json::from_str::<TavernSession>(&raw) else {
            return;
        };
        let should = match &sess.active_run_id {
            Some(cur) => cur == run_id || cur.starts_with("pending-"),
            None => false,
        };
        if should {
            sess.active_run_id = None;
            sess.updated_at = now_rfc3339();
            let _ = write_atomic(&path, &serde_json::to_string(&sess).unwrap_or_default());
        }
    }

    // ─── T2 世界认知（U2 实体图谱 / U7 真相账本）─────────────────────────────

    fn entity_kind_str(kind: &WsEntityKind) -> String {
        match kind {
            WsEntityKind::Character => "character".into(),
            WsEntityKind::Location => "location".into(),
            WsEntityKind::Item => "item".into(),
            WsEntityKind::Faction => "faction".into(),
            WsEntityKind::Concept => "concept".into(),
            WsEntityKind::Custom(s) => s.to_ascii_lowercase(),
        }
    }

    /// 从 pack 播种初始实体（角色 → Character 实体）。会话级 world 的种子来源。
    fn seed_world_from_pack(pack: &StoryPack) -> WorldState {
        let mut ws = WorldState::new();
        for ia in &pack.actor_state_config.initial_actors {
            let name = pack
                .characters
                .iter()
                .find(|c| c.id == ia.character_id)
                .map(|c| c.name.clone())
                .unwrap_or_else(|| ia.character_id.clone());
            let description = pack
                .actor_state_config
                .templates
                .get(&ia.template_id)
                .map(|t| format!("actor 模板 {}（{} 个字段）", ia.template_id, t.fields.len()))
                .unwrap_or_else(|| "（无模板描述）".to_string());
            let entity = WorldEntity {
                id: ia.character_id.clone(),
                kind: WsEntityKind::Character,
                name,
                description,
                properties: Default::default(),
                relationships: Vec::new(),
                state_flags: Default::default(),
                counters: Default::default(),
            };
            ws.entities.insert(entity.id.clone(), entity);
        }
        ws
    }

    /// U2: 实体列表查询（kind 筛选 + q 名字/ID 模糊）。
    pub fn world_entities(
        &self,
        session_id: &str,
        kind: Option<String>,
        q: Option<String>,
    ) -> CoreResult<Value> {
        let sess = self.get(session_id)?;
        let q = q.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let mut items: Vec<Value> = Vec::new();
        for e in sess.world.entities.values() {
            let kname = Self::entity_kind_str(&e.kind);
            if let Some(k) = &kind {
                if !k.eq_ignore_ascii_case(&kname) {
                    continue;
                }
            }
            if let Some(qq) = &q {
                let name = e.name.to_lowercase();
                let id = e.id.to_lowercase();
                let ql = qq.to_lowercase();
                if !name.contains(&ql) && !id.contains(&ql) {
                    continue;
                }
            }
            items.push(json!({
                "id": e.id,
                "kind": kname,
                "name": e.name,
                "description": e.description,
                "stateFlags": e.state_flags,
                "counters": e.counters,
                "properties": e.properties,
                "relationCount": e.relationships.len(),
            }));
        }
        items.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });
        Ok(json!({ "entities": items, "count": items.len() }))
    }

    /// U2: 实体详情 + 关系树（出边/入边展开目标实体名与类型）。
    pub fn world_entity_detail(&self, session_id: &str, entity_id: &str) -> CoreResult<Value> {
        let sess = self.get(session_id)?;
        let entity = sess
            .world
            .get_entity(entity_id)
            .ok_or_else(|| CoreError::NotFound(format!("entity not found: {entity_id}")))?;
        let kname = Self::entity_kind_str(&entity.kind);
        let out_edges: Vec<Value> = entity
            .relationships
            .iter()
            .map(|r| {
                let target = sess.world.get_entity(&r.target_id);
                json!({
                    "targetId": r.target_id,
                    "targetName": target.map(|t| t.name.clone()).unwrap_or_default(),
                    "targetKind": target.map(|t| Self::entity_kind_str(&t.kind)).unwrap_or_default(),
                    "relationType": r.relation_type,
                    "strength": r.strength,
                    "direction": "out",
                })
            })
            .collect();
        let mut in_edges: Vec<Value> = Vec::new();
        for other in sess.world.entities.values() {
            for r in &other.relationships {
                if r.target_id == entity_id {
                    in_edges.push(json!({
                        "sourceId": other.id,
                        "sourceName": other.name,
                        "sourceKind": Self::entity_kind_str(&other.kind),
                        "relationType": r.relation_type,
                        "strength": r.strength,
                        "direction": "in",
                    }));
                }
            }
        }
        Ok(json!({
            "entity": {
                "id": entity.id,
                "kind": kname,
                "name": entity.name,
                "description": entity.description,
                "properties": entity.properties,
                "stateFlags": entity.state_flags,
                "counters": entity.counters,
            },
            "relationships": { "out": out_edges, "in": in_edges },
            "relationshipCount": out_edges.len() + in_edges.len(),
        }))
    }

    /// U2: 应用世界事件（EntityCreated/Updated、flag/counter/relationship/meta），落盘并返回快照。
    pub fn world_apply_events(&self, session_id: &str, events: Vec<WorldEvent>) -> CoreResult<Value> {
        let mut sess = self.get(session_id)?;
        let before = sess.world.event_log.len();
        for ev in &events {
            sess.world.apply(ev.clone());
        }
        let saved = self.save(sess)?;
        Ok(json!({
            "applied": events.len(),
            "eventLogLen": saved.world.event_log.len(),
            "delta": saved.world.event_log.len() - before,
            "snapshot": saved.world.snapshot(),
        }))
    }

    fn truth_keys(ev: &WorldEvent) -> Vec<(String, String)> {
        match ev {
            WorldEvent::EntityCreated(e) => {
                let mut v = vec![(e.id.clone(), "exists".to_string())];
                for k in e.properties.keys() {
                    v.push((e.id.clone(), format!("prop.{k}")));
                }
                for f in e.state_flags.iter() {
                    v.push((e.id.clone(), format!("flag.{f}")));
                }
                for k in e.counters.keys() {
                    v.push((e.id.clone(), format!("counter.{k}")));
                }
                v
            }
            WorldEvent::EntityUpdated { id, changes } => changes
                .keys()
                .map(|k| (id.clone(), format!("prop.{k}")))
                .collect(),
            WorldEvent::EntityRemoved(id) => vec![(id.clone(), "exists".to_string())],
            WorldEvent::FlagSet { entity_id, flag } => {
                Self::truth_scope_keys(entity_id, &format!("flag.{flag}"))
            }
            WorldEvent::FlagCleared { entity_id, flag } => {
                Self::truth_scope_keys(entity_id, &format!("flag.{flag}"))
            }
            WorldEvent::CounterChanged { entity_id, counter, .. } => {
                Self::truth_scope_keys(entity_id, &format!("counter.{counter}"))
            }
            WorldEvent::CounterSet { entity_id, counter, .. } => {
                Self::truth_scope_keys(entity_id, &format!("counter.{counter}"))
            }
            WorldEvent::RelationshipSet {
                source_id,
                target_id,
                relation_type,
                ..
            } => vec![(
                source_id.clone(),
                format!("rel.{relation_type}.{target_id}"),
            )],
            WorldEvent::RelationshipRemoved {
                source_id,
                target_id,
                relation_type,
            } => vec![(
                source_id.clone(),
                format!("rel.{relation_type}.{target_id}"),
            )],
            WorldEvent::MetaSet { key, .. } => vec![("global".to_string(), format!("meta.{key}"))],
            WorldEvent::NarrativeEvent { .. } => Vec::new(),
        }
    }

    fn truth_scope_keys(entity_id: &Option<String>, key: &str) -> Vec<(String, String)> {
        match entity_id {
            Some(id) => vec![(id.clone(), key.to_string())],
            None => vec![("global".to_string(), key.to_string())],
        }
    }

    fn truth_value(sess: &TavernSession, scope: &str, key: &str) -> Value {
        if scope == "global" {
            if let Some(mk) = key.strip_prefix("meta.") {
                return sess.world.meta.get(mk).cloned().unwrap_or(Value::Null);
            }
            if let Some(fk) = key.strip_prefix("flag.") {
                return json!(sess.world.global_flags.contains(fk));
            }
            if let Some(ck) = key.strip_prefix("counter.") {
                return json!(sess.world.global_counters.get(ck).copied().unwrap_or(0));
            }
            return Value::Null;
        }
        let entity = match sess.world.entities.get(scope) {
            Some(e) => e,
            None => return Value::Null,
        };
        if key == "exists" {
            return json!(true);
        }
        if let Some(fk) = key.strip_prefix("flag.") {
            return json!(entity.state_flags.contains(fk));
        }
        if let Some(ck) = key.strip_prefix("counter.") {
            return json!(entity.counters.get(ck).copied().unwrap_or(0));
        }
        if let Some(pk) = key.strip_prefix("prop.") {
            return entity.properties.get(pk).cloned().unwrap_or(Value::Null);
        }
        if let Some(rk) = key.strip_prefix("rel.") {
            if let Some((rtype, tid)) = rk.split_once('.') {
                if let Some(r) = entity
                    .relationships
                    .iter()
                    .find(|r| r.relation_type == rtype && r.target_id == tid)
                {
                    return json!(r.strength);
                }
            }
            return json!(0.0);
        }
        Value::Null
    }

    fn truth_scope_kind(sess: &TavernSession, scope: &str) -> String {
        if scope == "global" {
            return "global".to_string();
        }
        sess.world
            .entities
            .get(scope)
            .map(|e| Self::entity_kind_str(&e.kind))
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// U7: 从 event_log 派生的真相账本（每 key 当前值 + 变更版本/最后事件）。
    pub fn world_truth(&self, session_id: &str) -> CoreResult<Value> {
        let sess = self.get(session_id)?;
        let mut versions: std::collections::HashMap<(String, String), usize> =
            std::collections::HashMap::new();
        let mut last: std::collections::HashMap<(String, String), usize> =
            std::collections::HashMap::new();
        for (idx, ev) in sess.world.event_log.iter().enumerate() {
            for (scope, key) in Self::truth_keys(ev) {
                *versions.entry((scope.clone(), key.clone())).or_insert(0) += 1;
                last.insert((scope, key), idx);
            }
        }
        let mut entries: Vec<Value> = Vec::new();
        for ((scope, key), version) in &versions {
            let last_event = last
                .get(&(scope.clone(), key.clone()))
                .copied()
                .unwrap_or(0);
            let scope_name = if scope == "global" {
                "世界".to_string()
            } else {
                sess.world
                    .entities
                    .get(scope)
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| scope.clone())
            };
            entries.push(json!({
                "scope": scope,
                "scopeName": scope_name,
                "scopeKind": Self::truth_scope_kind(&sess, scope),
                "key": key,
                "value": Self::truth_value(&sess, scope, key),
                "status": "current",
                "version": version,
                "lastEvent": last_event,
            }));
        }
        entries.sort_by(|a, b| {
            let ak = (a["scope"].as_str().unwrap_or(""), a["key"].as_str().unwrap_or(""));
            let bk = (b["scope"].as_str().unwrap_or(""), b["key"].as_str().unwrap_or(""));
            ak.cmp(&bk)
        });
        Ok(json!({
            "entries": entries,
            "count": entries.len(),
            "eventLogLen": sess.world.event_log.len(),
        }))
    }

    /// U7: 一致性断言——期望值与账本当前值比较（供编排/质检做世界真相校验）。
    pub fn world_truth_check(
        &self,
        session_id: &str,
        entity_id: String,
        key: String,
        expected: Value,
    ) -> CoreResult<Value> {
        let tv = self.world_truth(session_id)?;
        let entries = tv["entries"].as_array().cloned().unwrap_or_default();
        let scope = if entity_id.is_empty() { "global".to_string() } else { entity_id.clone() };
        let hit = entries.iter().find(|e| {
            e["scope"].as_str().unwrap_or("") == scope && e["key"].as_str().unwrap_or("") == key
        });
        let current = hit.map(|e| e["value"].clone()).unwrap_or(Value::Null);
        let ok = current == expected;
        Ok(json!({
            "entityId": entity_id,
            "key": key,
            "expected": expected,
            "current": current,
            "ok": ok,
            "status": if hit.is_none() { "missing" } else if ok { "consistent" } else { "conflict" },
        }))
    }

    /// CAS write: only persists if the on-disk `updated_at` still equals `base_revision`.
    pub fn save_with_revision(
        &self,
        session: TavernSession,
        base_revision: &str,
    ) -> CoreResult<TavernSession> {
        self.save_inner(session, Some(base_revision))
    }

    /// M-2 (CAS): Atomic read-modify-write inside a single locked transaction.
    ///
    /// Loads the session by `session_id`, hands a mutable reference to the
    /// closure `f`, then persists the result. Because the load, mutation, and
    /// write all happen while holding the store mutex, no concurrent save can
    /// interleave — the read-modify-write is race-free without requiring the
    /// caller to thread a base_revision string around.
    ///
    /// The closure may return `Err` to abort the write (e.g. a validation
    /// failure or "turn in progress" guard); in that case the on-disk session
    /// is left untouched.
    ///
    /// Returns the persisted session (post-closure mutation) on success.
    pub fn update_session<F>(
        &self,
        session_id: &str,
        f: F,
    ) -> CoreResult<TavernSession>
    where
        F: FnOnce(&mut TavernSession) -> CoreResult<()>,
    {
        let _g = self.lock.lock();
        let path = self.path_for(session_id)?;
        if !path.exists() {
            return Err(CoreError::NotFound(format!(
                "session not found: {session_id}"
            )));
        }
        let raw = fs::read_to_string(&path)?;
        let mut sess: TavernSession = serde_json::from_str(&raw)?;
        f(&mut sess)?;
        let now = now_rfc3339();
        if sess.created_at.is_empty() {
            sess.created_at = now.clone();
        }
        sess.updated_at = now;
        write_atomic(&path, &serde_json::to_string(&sess)?)?;
        Ok(sess)
    }

    fn save_inner(
        &self,
        mut session: TavernSession,
        base_revision: Option<&str>,
    ) -> CoreResult<TavernSession> {
        let _g = self.lock.lock();
        let is_new = session.session_id.trim().is_empty();
        if is_new {
            session.session_id = format!("tavern-session-{}", Uuid::new_v4());
        }
        let _ = safe_id(&session.session_id)?;
        let now = now_rfc3339();
        if session.created_at.is_empty() {
            session.created_at = now.clone();
        }
        session.updated_at = now;

        // Revision CAS: existing sessions must be saved on top of the revision the caller read.
        if !is_new {
            if let Some(base) = base_revision {
                let path = self.path_for(&session.session_id)?;
                if !path.exists() {
                    return Err(CoreError::Conflict(
                        "session 已被其他操作删除，请重新加载".into(),
                    ));
                }
                let raw = fs::read_to_string(&path)?;
                let current: TavernSession = serde_json::from_str(&raw)?;
                if current.updated_at != base {
                    return Err(CoreError::Conflict(format!(
                        "session 已被其他操作更新，请重新加载后再保存 (期望 revision {base}，当前 {})",
                        current.updated_at
                    )));
                }
            }
        }

        fs::create_dir_all(self.dir())?;
        let path = self.path_for(&session.session_id)?;
        // P0-1(审计): 紧凑序列化替代 to_string_pretty——MB 级会话文件省 30-50% 字节与 CPU
        // （长会话含 30 checkpoint + messages + embedding，写放大 O(n²) 主因）。
        write_atomic(&path, &serde_json::to_string(&session)?)?;
        Ok(session)
    }

    pub fn delete(&self, session_id: &str) -> CoreResult<()> {
        let _g = self.lock.lock();
        let path = self.path_for(session_id)?;
        if !path.exists() {
            return Err(CoreError::NotFound(format!(
                "session not found: {session_id}"
            )));
        }
        fs::remove_file(path)?;
        Ok(())
    }

    pub fn create_from_pack(
        &self,
        packs: &PackStore,
        req: CreateSessionRequest,
    ) -> CoreResult<TavernSession> {
        let pack = match packs.get(&req.pack_id) {
            Ok(p) => p,
            Err(CoreError::NotFound(_)) => {
                return Err(CoreError::NotFound(format!(
                    "pack not found: {}",
                    req.pack_id
                )));
            }
            Err(e) => return Err(e),
        };

        // Card max: take min across pack characters' tiers, then pack.max_tier
        let mut card_max = pack.max_tier;
        for c in &pack.characters {
            if let Some(t) = c.content_tier {
                if t.rank() < card_max.rank() {
                    card_max = t;
                }
            }
        }
        let global = req.global_tier.unwrap_or(ContentTier::Open);
        let final_tier = ContentTier::min3(req.user_tier, card_max, global);

        let entry = req.entry.unwrap_or_default();
        // P3 should have entry role; still allow create (wizard half-done) but keep fields
        let control = entry
            .vessel_character_id
            .clone()
            .or_else(|| pack.characters.first().map(|c| c.id.clone()));

        let present: Vec<String> = pack
            .first_node_id()
            .and_then(|nid| pack.nodes.iter().find(|n| n.id == nid).cloned())
            .map(|n| n.present_characters)
            .unwrap_or_else(|| pack.characters.iter().map(|c| c.id.clone()).collect());

        let title = req.title.unwrap_or_else(|| {
            format!(
                "{} · {}",
                pack.title,
                match req.playable {
                    Playable::P1 => "P1",
                    Playable::P2 => "P2",
                    Playable::P3 => "P3",
                    Playable::P4 => "P4",
                }
            )
        });

        let author_project_id = req
            .author_project_id
            .as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // U13: auto-mount the target work's saved creation compass (if any) so
        // brand-new sessions inherit it without a re-PUT after compass save.
        let compass_auto = req.work_id.as_ref().and_then(|wid| {
            let s = CompassStore::new(self.data.clone());
            match s.load(wid) {
                Ok(c) if !c.is_empty() => Some(c),
                _ => None,
            }
        });
        let mut session = TavernSession {
            session_id: format!("tavern-session-{}", Uuid::new_v4()),
            pack_id: pack.id.clone(),
            pack_missing: false,
            owner: req.owner.clone(),
            quality: req.quality,
            playable: req.playable,
            play_mode: req.play_mode,
            content_tier: final_tier,
            user_tier_request: req.user_tier,
            entry,
            chapter_cursor: pack.first_chapter_id(),
            node_id: pack.first_node_id(),
            resume_node_id: None,
            opening_seeded: false,
            side_branch_node_id: None,
            side_branch_label: None,
            current_worldline_id: None,
            last_restored_save_id: None,
            panels: vec![],
            mcp_tool_results: vec![],
            skill_load: None,
            timeline_id: "main".into(),
            turn: 0,
            present_character_ids: present.clone(),
            focus_character_id: present.first().cloned().or_else(|| pack.characters.first().map(|c| c.id.clone())),
            speaker_rotation: matches!(req.playable, Playable::P2 | Playable::P3 | Playable::P4),
            player: PlayerState {
                display_name: req
                    .player_display_name
                    .unwrap_or_else(|| "旅人".into()),
                control_character_id: control,
                persona: String::new(),
                inventory: vec![],
                flags: json!({}),
            },
            memory_l1: MemoryL1::default(),
            memory_l2: MemoryL2::default(),
            memory_l3: MemoryL3::default(),
            memory_l4: MemoryL4::default(),
            guard_events: vec![],
            messages: vec![],
            active_run_id: None,
            adult_confirmed: req.adult_confirmed,
            title,
            created_at: String::new(),
            updated_at: String::new(),
            author_project_id: author_project_id.clone(),
            author_live_path: None,
            author_live_enabled: true,
            author_live_every_n: 1,
            author_live_write_turns: false,
            actor_states: {
                let mut sys = pack.actor_state_config.to_system();
                if let Some(c) = compass_auto {
                    sys.mount_compass(c);
                } else {
                    // [自动罗盘] 无手写罗盘 → 首章目标派生 current_focus（作者可 PUT 覆盖）
                    let focus = crate::tavern_engine::derive_focus_from_pack(&pack, pack.first_chapter_id().as_deref());
                    if !focus.is_empty() {
                        sys.mount_compass(crate::st_compass::Compass::new("", focus));
                    }
                }
                sys
            },
            world: Self::seed_world_from_pack(&pack),
            // [1B 2026-08-18] 初始时间天气从 pack 开场信号推导（角色 openingScene + 首节点摘要），
            // 避免默认「清晨/晴/春」与原著设定冲突（宿醉「夏末雨夜」被压死事故，见
            // docs/宿醉时间天气原著冲突-20260817.md）。
            game_clock: {
                let mut clock_signals = String::new();
                for c in &pack.characters {
                    if !c.opening_scene.is_empty() {
                        clock_signals.push_str(&c.opening_scene);
                        clock_signals.push(' ');
                    }
                }
                if let Some(nid) = pack.first_node_id() {
                    if let Some(n) = pack.nodes.iter().find(|n| n.id == nid) {
                        if !n.summary.is_empty() {
                            clock_signals.push_str(&n.summary);
                        }
                    }
                }
                crate::time_clock::GameClock::derive_from_text(&clock_signals)
            },
            director_plan: None,
            director_pending: false,
            director_task: None,
            last_event: None,
            last_check_results: vec![],
        check_history: vec![],
            checkpoints: vec![],
            fold_through_turn: 0,
            // U11: 新会话默认 epoch=0、空账本（serde default 兼容旧会话文件）。
            epoch: 0,
            epoch_last_turn: None,
            epoch_last_chars: None,
            turn_cost_ledger: TurnCostLedger::default(),
            last_turn_diagnostic: None,
            scribe_whispers: Default::default(),
            skim_issues: Vec::new(),
            skim_sample: String::new(),
            chapter_diaries: Vec::new(),
            turn_progress: None,
            diary_config: None,
            pockets: {
                let mut m = std::collections::HashMap::new();
                for c in &pack.characters {
                    if !c.starting_wardrobe.is_empty() {
                        m.insert(c.id.clone(), c.starting_wardrobe.clone());
                    }
                }
                m
            },
            pockets_enabled: true,
            needs: Default::default(),
            growth: Default::default(),
            world_climate: Default::default(),
            chaos: Default::default(),
            milestones: Default::default(),
            objectives: Default::default(),
            ambitions: Default::default(),
            dream: Default::default(),
            episodes: Default::default(),
            journal: Default::default(),
            relationships: Default::default(),
            pending_swipes: Default::default(),
            promises: Default::default(),
            preferences: Default::default(),
            presence: Default::default(),
            timed_world_info: Default::default(),
            event_extract: true,
        };
        if let Some(pid) = author_project_id {
            session.author_live_path = Some(format!(
                "projects/{pid}/sessions/{}/live.md",
                session.session_id
            ));
        }
        // First-open / new session: seed opening monologue (主线开场白).
        let _ = seed_opening_if_needed(&mut session, &pack);
        self.save(session)
    }

    /// When pack deleted: mark sessions pack_missing (read-only play later).
    pub fn mark_pack_missing(&self, pack_id: &str) -> CoreResult<usize> {
        let _g = self.lock.lock();
        fs::create_dir_all(self.dir())?;
        let mut n = 0usize;
        for ent in fs::read_dir(self.dir())? {
            let ent = ent?;
            let path = ent.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut s) = serde_json::from_str::<TavernSession>(&raw) else {
                continue;
            };
            if s.pack_id == pack_id && !s.pack_missing {
                s.pack_missing = true;
                s.updated_at = now_rfc3339();
                let _ = write_atomic(&path, &serde_json::to_string_pretty(&s)?);
                n += 1;
            }
        }
        Ok(n)
    }
}
