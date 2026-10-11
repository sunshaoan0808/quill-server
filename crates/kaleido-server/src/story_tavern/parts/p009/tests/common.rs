//! p009 tests · common（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    // ─── skill 工具按需加载（P4 后置②）─────────────────────────────────────

    pub(crate) fn test_pack_minimal() -> StoryPack {
        StoryPack {
            id: "p1".into(),
            title: "测试剧本".into(),
            source: Default::default(),
            characters: vec![],
            world_book_ids: vec![],
            chapters: vec![],
            nodes: vec![],
            lore_entries: vec![],
            default_mode: Default::default(),
            max_tier: Default::default(),
            language: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            stage_director: Default::default(),
            event_packages: vec![],
            actor_state_config: Default::default(),
            worldline: vec![], // T 层测试 fixture
        }
    }

    pub(crate) fn test_session_minimal() -> TavernSession {
        serde_json::from_value(json!({
            "sessionId": "s1",
            "packId": "p1",
            "playable": "P1",
            "playMode": "mainline",
            "contentTier": "standard",
        }))
        .expect("minimal session must parse")
    }
