//! p006 测试集 · focus（R6 自 p006.rs 机械迁入，仅 use 路径改绝对）
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
mod focus_tests {
    use crate::story_tavern::parts::*;
    #[test]
    fn rotate_focus_cycles() {
        let mut s = TavernSession {
            session_id: "tavern-session-test".into(),
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
            present_character_ids: vec!["a".into(), "b".into(), "c".into()],
            focus_character_id: Some("a".into()),
            speaker_rotation: true,
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
        };
        assert_eq!(rotate_focus_character(&mut s).as_deref(), Some("b"));
        assert_eq!(rotate_focus_character(&mut s).as_deref(), Some("c"));
        assert_eq!(rotate_focus_character(&mut s).as_deref(), Some("a"));
    }
}

// ─── 世界线测例（吸收自 Liyuan worldline.ts） ──────────────────────────────

