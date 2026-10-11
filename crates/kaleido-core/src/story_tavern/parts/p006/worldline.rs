//! p006 测试集 · worldline（R6 自 p006.rs 机械迁入，仅 use 路径改绝对）
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
mod worldline_tests {
    use crate::story_tavern::parts::*;

    fn meta(save_id: &str, label: &str, turn: u32, fork_from: Option<&str>, wl: &str) -> TavernSaveMeta {
        TavernSaveMeta {
            save_id: save_id.into(),
            session_id: "s1".into(),
            label: label.into(),
            turn,
            node_id: None,
            chapter_cursor: None,
            play_mode: PlayMode::Mainline,
            created_at: format!("2026-08-01T00:00:{:02}Z", turn),
            fork_from_save_id: fork_from.map(|s| s.to_string()),
            worldline_id: wl.into(),
        }
    }

    fn session(wl: Option<&str>, restored: Option<&str>) -> TavernSession {
        TavernSession {
            session_id: "s1".into(),
            pack_id: "p1".into(),
            pack_missing: false,
            owner: None,
            quality: Quality::Lite,
            playable: Playable::P2,
            play_mode: PlayMode::Mainline,
            content_tier: ContentTier::Standard,
            panels: vec![],
            mcp_tool_results: vec![],
            skill_load: None,
            user_tier_request: ContentTier::Standard,
            entry: EntryConfig::default(),
            chapter_cursor: None,
            node_id: None,
            resume_node_id: None,
            opening_seeded: false,
            side_branch_node_id: None,
            side_branch_label: None,
            current_worldline_id: wl.map(|x| x.to_string()),
            last_restored_save_id: restored.map(|x| x.to_string()),
            timeline_id: "main".into(),
            turn: 10,
            present_character_ids: vec![],
            focus_character_id: None,
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
            // U11: 测试构造显式初始化新字段。
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

    #[test]
    fn test_build_worldline_main_only() {
        let saves = vec![
            meta("save-a", "第2回合", 2, None, "main"),
            meta("save-b", "第5回合", 5, None, "main"),
        ];
        let view = build_worldline(&session(Some("main"), None), &saves);
        assert_eq!(view.lines.len(), 1);
        assert_eq!(view.lines[0].id, "main");
        assert_eq!(view.lines[0].saves.len(), 2);
        assert_eq!(view.current_worldline_id, "main");
        assert_eq!(view.current_save_id.as_deref(), Some("save-b"));
    }

    #[test]
    fn test_build_worldline_fork() {
        let saves = vec![
            meta("save-a", "第2回合", 2, None, "main"),
            meta("save-b", "第5回合", 5, Some("save-a"), "wl-1"),
        ];
        let view = build_worldline(&session(Some("wl-1"), None), &saves);
        assert_eq!(view.lines.len(), 2);
        let fork = view.lines.iter().find(|l| l.id == "wl-1").expect("fork line");
        assert_eq!(fork.fork_from_save_id.as_deref(), Some("save-a"));
        assert_eq!(fork.saves.len(), 1);
        assert_eq!(view.current_worldline_id, "wl-1");
        assert_eq!(view.current_save_id.as_deref(), Some("save-b"));
    }

    #[test]
    fn test_fork_detection_turn_advanced() {
        // 回档(save-a, turn=2) 后走出不同路到 turn=5 再存 → 分叉
        let mut sess = session(None, Some("save-a"));
        sess.turn = 5;
        let _saves = vec![meta("save-a", "第2回合", 2, None, "main")];
        // create_save 逻辑的纯函数复现：restored_turn(2) < sess.turn(5) → fork
        let restored_turn = 2;
        assert!(restored_turn < sess.turn);
        // build_worldline 视角：新线从 save-a fork
        let view = build_worldline(
            &session(Some("wl-new"), None),
            &[meta("save-a", "第2回合", 2, None, "main"), meta("save-c", "第5回合", 5, Some("save-a"), "wl-new")],
        );
        assert_eq!(view.lines.len(), 2);
        assert_eq!(view.current_worldline_id, "wl-new");
    }
}

