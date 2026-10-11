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

// ─── G10: 回合提交幂等守卫测试 ───────────────────────────────────────────────

#[cfg(test)]
mod turn_submit_guard_tests {
    use super::super::*;

    fn msg(role: &str, content: &str) -> TavernMessage {
        TavernMessage {
            id: format!("m-{role}-{content}"),
            role: role.into(),
            content: content.into(),
            created_at: String::new(),
            options: vec![],
            engine_tag: None,
            program: None,
            reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
        }
    }

    fn session_with(msgs: &[(&str, &str)]) -> TavernSession {
        TavernSession {
            session_id: "t".into(),
            pack_id: "p".into(),
            pack_missing: false,
            owner: None,
            quality: Quality::Lite,
            playable: Playable::P1,
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
            present_character_ids: vec![],
            focus_character_id: None,
            speaker_rotation: false,
            player: PlayerState::default(),
            memory_l1: MemoryL1::default(),
            memory_l2: MemoryL2::default(),
            memory_l3: MemoryL3::default(),
            memory_l4: MemoryL4::default(),
            guard_events: vec![],
            messages: msgs.iter().map(|(r, c)| msg(r, c)).collect(),
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
            director_task: None,
            world: WorldState::default(),
            game_clock: Default::default(),
        }
    }

    /// 三态之一：同回合同内容已提交（末条 user 消息 hash 相同且已有 assistant 回应）→ true。
    #[test]
    fn same_content_duplicate_rejected() {
        let s = session_with(&[("user", "你好"), ("assistant", "你好呀")]);
        assert!(turn_submit_guard(&s, &text_hash("你好")));
    }

    /// 三态之二：不同内容 → false。
    #[test]
    fn different_content_allowed() {
        let s = session_with(&[("user", "你好"), ("assistant", "你好呀")]);
        assert!(!turn_submit_guard(&s, &text_hash("再见")));
        // 空会话首条提交也不判重复。
        let empty = session_with(&[]);
        assert!(!turn_submit_guard(&empty, &text_hash("开始")));
    }

    /// 三态之三：新回合 → false。
    /// 上一回合已完成（末条为 assistant）→ 新回合可提交；
    /// 新回合首个提交（末条 user 消息尚无 assistant 回应，可复用重试路径）→ 不判重复。
    #[test]
    fn new_turn_allowed() {
        let completed = session_with(&[("user", "你好"), ("assistant", "你好呀")]);
        assert!(!turn_submit_guard(&completed, &text_hash("继续")));

        let pending = session_with(&[("user", "你好"), ("assistant", "你好呀"), ("user", "继续")]);
        assert!(!turn_submit_guard(&pending, &text_hash("继续")));

        // LLM 空响应后重试同一输入：末条 user 消息无 assistant 回应 → 放行复用重试。
        let failed = session_with(&[("user", "你好")]);
        assert!(!turn_submit_guard(&failed, &text_hash("你好")));
    }
}