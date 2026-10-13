//! Kaleido core: DataRoot, users, sessions, password hashing.
// [机械拆分产物 2026-10-05] 头部 use 块经 `use super::*` glob 桥接供部件文件使用，
// 本文件不直接引用属预期；cargo fix 会误判未使用并删除（导致整片编译失败），
// 工程纪律见 docs/REFACTOR_20260927.md「遗留」节末条。
#![allow(unused_imports)]

//! Schema reserves user_id / workspace_id for multi-user later.
//! Single-user runtime is the product default.

use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;

mod libparts;
pub use libparts::*;
mod st_import;
mod st_png;
mod st_card_webp;
mod st_card_illustrations;
mod st_card_skill;
mod st_compass;
mod st_review;
pub mod json_repair;
pub mod retry;
mod st_world_info;
mod st_regex;
mod st_regex_library;
mod st_timed_store;
mod st_automation_log;
mod st_token_estimate;
mod st_vector_index;
mod story_tavern;
mod tavern_engine;
pub mod st_simulation;
pub mod st_skimming;
pub mod st_emotional_hooks;
pub mod st_outline;
pub mod st_writing_style;
pub mod st_memory_contract;
pub mod dialogue_fingerprint;
pub mod dice_audit;
pub mod dice;
pub mod grounding;
pub mod harness;
pub mod context_budget;
pub mod ui_components;
pub mod memory_weaver;
pub mod ledger;
pub mod emotion_curve;
pub mod alias_merge;
pub mod entity_resolve; // [ENT] 实体解析层：关系边端点→角色卡 id + 幽灵/稀疏对账
pub mod rel_category; // [L0] rel 自由词→graph_store 五类映射（family/social/emotional/conflict/uncertain）
pub mod style_stats;
pub mod character_arc;
pub mod embed_hash;
pub mod db;
pub mod bakemono_query_parse;
pub mod bakemono_retrieval;
pub mod chapter_diary;
pub mod hybrid_search;
pub mod graph_store;
pub mod analysis_store;
pub mod ai_admin_store;
pub mod character_archive;
pub mod foreshadow_store;
pub mod scene_card_store;
pub mod plugin;
pub mod world_state;
pub mod time_clock;
pub mod pockets;
pub mod needs;
pub mod journal_physics;
pub mod journal_store;
pub mod world_climate;
pub mod chaos;
pub mod relationship_tiers;
pub mod objectives;
pub mod dreams;
pub mod relationship;
pub mod promise;
pub mod mood_presence;
pub mod humanize;
pub mod novel_workflow;
pub mod moa_comparison;
pub mod import_security;
pub mod text_chunker;
pub mod progressive_compress;
pub mod st_mvu; // [MVU 2026-10-11, 吞噬自 AIcove] 酒馆 MVU 变量"只解析不执行"兼容层（纯函数，零新依赖）
pub mod proactive_trigger; // [主动关怀 P1 2026-10-11, 吞噬自 AIcove] 触发器模型+判定+JSON store（P2 调度器在 server）
pub use import_security::{
    decode_utf8_imported_text, inspect_imported_plain_text, ImportThreat,
    MAX_IMPORT_TEXT_BYTES,
};
pub mod image_metadata;
pub use image_metadata::{read_raster_image_metadata, RasterImage, MAX_IMAGE_PIXELS};
pub mod docx_security;
pub mod extraction; // [文档抽取 PR5 2026-10-13] 内置 docx/xlsx/csv/pdf/txt 抽取，DEEIX 纪律直译
pub use docx_security::validate_docx;
pub mod prompt_safety;
pub use st_import::{
    build_st_import_bundle, character_book_entry_count, character_book_to_world_book,
    extract_embedded_images, import_st_character_card_bundle, import_st_character_card_json,
    parse_st_character_card_json, parse_st_character_card_value, st_card_to_partner_fields,
    st_card_to_partner_item, AssetRef, StCardData, StImportBundle, StImportError,
};
pub use st_png::{
    base64_to_png, embed_st_card_in_png, extract_st_card_from_png, png_to_base64,
};
pub use st_card_webp::{extract_st_card_from_jpeg, extract_st_card_from_webp};
pub use st_card_illustrations::{
    extract_catbox_illustrations, CatboxIllustration,
};
pub use st_card_skill::{render_card_skill, sanitize_skill_name};
pub use st_world_info::{
    check_world_info, check_world_info_timed, chat_to_scan_buffer, content_from_wi_entries,
    entries_from_world_book, entry_values_from_world_book, format_wi_for_system,
    import_card_world_book, merge_wi_entry_value, parse_decorators, parse_mes_examples,
    parse_regex_from_string, parse_wi_entry, st_book_raw_from_entries, substitute_params,
    wi_entry_to_st_json, ActivatedEntry, CharacterFilter, SelectiveLogic, TimedWorldInfo,
    WiChatInjection, WiDepthEntry, WiEntry, WiExampleMessage, WiOutletEntry, WiPosition,
    WiPromptSlots, WiScanContext, WiScanResult, WiSettings, WiTimedEffect,
};
pub use st_timed_store::TimedWorldInfoStore;
pub use st_automation_log::{AutomationTriggerEvent, AutomationTriggerLog, AutomationTriggerStore};
pub use st_compass::{Compass, CompassStore, COMPASS_FILE_NAME, COMPASS_MAX_LEN, COMPASS_SCHEMA_VERSION};
pub use dialogue_fingerprint::{build_all, build_fingerprint, check_turn_voices, drift_check, CharacterFingerprint, DriftReport};
pub use st_review::{
    run_post_check, run_dramatica_checks, PostIssue, ReviewHistory, ReviewIssue, ReviewRun, ReviewStore,
    REVIEW_DIMENSIONS, REVIEW_FILE_NAME, REVIEW_MAX_RUNS, REVIEW_SCHEMA_VERSION,
    REVIEW_STATUS_ACCEPTED, REVIEW_STATUS_FIXED, REVIEW_STATUS_OPEN,
    POST_RULE_FORBIDDEN_WORD, POST_RULE_AI_TRACE, POST_RULE_LONG_SENTENCE,
    POST_RULE_REPEAT_WORD, POST_RULE_PUNCT_ABUSE,
    POST_RULE_CONSECUTIVE_LE, POST_RULE_LONG_PARAGRAPH, POST_RULE_WORD_COUNT_DEVIATION,
    POST_RULE_AI_MARKER_DENSITY,
};
pub use st_token_estimate::{estimate_many, estimate_tokens, estimate_tokens_detailed, TokenEstimate, TokenEstimateMode};
pub use st_vector_index::{
    entry_embed_text, hits_to_map, merge_hit_lists, rank_hits, text_hash, vector_cosine_similarity,
    vector_model_mismatch, VectorActivationSettings, VectorHit, VectorIndexEntry, VectorIndexFile,
    VectorIndexStore,
};
pub use st_regex::{
    get_regexed_string, parse_regex_script, run_regex_script, scripts_from_card_fields,
    scripts_from_value, RegexPlacement, RegexScript,
};
pub use st_regex_library::{
    merge_regex_scripts, resolve_runtime_scripts, scripts_from_import_body, RegexLibraryFile,
    RegexLibraryStore,
};

pub use story_tavern::{
    ContentTier, CreateSessionRequest, EntryConfig, EntryRole, EngineTag, MemoryL1, MemoryL2, MemoryL2Event, MemoryL3, MemoryL4,
    MetaKnowledge, NodeExit, PackCharacterRef, PackSource, PackStore, PackSummary, PlayMode, Playable, Quality, TavernSave, TavernSaveMeta,
    ActorStateSystem, ActorStateUpdate, PlayerState, RewriteIntensity, SideBranchCatalog, SideBranchNode, StoryChapter, StoryNode, StoryPack, TavernMessage,
    ActorStatePackConfig,
    TavernPersona, TavernPersonaStore, TavernSession, TavernSessionStore, TavernPanel, WorldlineLine, WorldlineView,
    ToolResultBrief,
    SkillLoadInfo,
    RuleSystem, RuleCheck, RuleStateBinding, TurnCheckRequest, TurnCheckBonus, TurnCheckOutcomes, TurnCheckOutcome, TurnStateChange, CheckResult, CheckHistoryEntry,
    parse_dice, difficulty_to_dc, roll_check,
    DirectorPlan, DirectorPlanRunStatus, DirectorLedgerEntry, DirectorTaskGroup, director_due,
    fit_text_to_token_budget, opening_plan_due,
    retain_guard_events, retain_l2_events, turn_submit_guard,
    TurnDiagnostic, TurnCostLedger,
    // [morphling Wave B3 2026-08-16] 章节剧情摘要账本
    ChapterDiaryEntry, ChapterDiaryConfig,
    StageDirectorConfig,
    EventPackage, EventLogEntry, TellerEventCard, pick_event_card,
    TurnCheckpoint,
    // [morphling ROMA P0 2026-08-19] 回合级进度检查点（崩溃恢复判读用）
    TurnPhase, TurnProgress,
    build_mainline_opening, build_pack_novel_summary, build_side_branch_catalog, build_side_opening,
    build_worldline, clip_chars, enter_side_branch, ensure_focus_character, is_clean_cast_name,
    rotate_focus_character, seed_opening_if_needed, select_side_branch_nodes,
};
pub use tavern_engine::{
    TurnExtraction, apply_extraction, apply_memory_compression, build_compression_prompt,
    build_cross_session_context, build_extraction_prompt,
    build_memory_context, classify_engine_tag,
    cosine_similarity, heuristic_l2_l3_from_turn, parse_extraction_response, persist_cross_session,
    post_turn_extraction, tokenize,
    try_advance_node, is_mainline_exhausted, derive_focus_from_pack, refresh_auto_focus,
    // [morphling Wave B3 2026-08-16] 章节剧情摘要账本
    build_chapter_diary_prompt, parse_chapter_diary_response,
};

#[cfg(test)]
mod brand_dir_tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp_root() -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "kaleido_brand_dir_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn brand_dir_prefers_new_and_falls_back_to_legacy() {
        let root = tmp_root();
        // neither exists → new path
        assert_eq!(brand_dir(&root, "config"), root.join("Kaleido").join("config"));
        // legacy exists, new doesn't → legacy
        fs::create_dir_all(root.join("MuseAI").join("config")).unwrap();
        assert_eq!(brand_dir(&root, "config"), root.join("MuseAI").join("config"));
        // new exists → new wins even if legacy also present
        fs::create_dir_all(root.join("Kaleido").join("config")).unwrap();
        assert_eq!(brand_dir(&root, "config"), root.join("Kaleido").join("config"));
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod works_fs_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_data() -> (tempfile_shim::Tmp, DataRoot, WorksFs, String) {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).expect("data root");
        let fs = WorksFs::new(data.clone());
        let ws = "ws-test-001";
        (tmp, data, fs, ws.into())
    }

    /// Minimal temp dir without adding a dependency.
    mod tempfile_shim {
        use super::*;
        pub struct Tmp {
            path: PathBuf,
        }
        impl Tmp {
            pub fn new() -> Self {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let path = std::env::temp_dir().join(format!("kaleido-works-test-{nanos}"));
                fs::create_dir_all(&path).unwrap();
                Self { path }
            }
            pub fn path(&self) -> &Path {
                &self.path
            }
        }
        impl Drop for Tmp {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn happy_path_crud() {
        let (_tmp, _data, wfs, ws) = tmp_data();
        wfs.mkdir(&ws, "notes").unwrap();
        wfs.write_text(&ws, "notes/hello.md", "# hi\n").unwrap();
        let body = wfs.read_text(&ws, "notes/hello.md").unwrap();
        assert_eq!(body.content, "# hi\n");
        let list = wfs.list(&ws, "", 2).unwrap();
        assert_eq!(list.kind, "dir");
        assert!(list.children.iter().any(|c| c.name == "notes"));
        wfs.rename(&ws, "notes/hello.md", "notes/renamed.md")
            .unwrap();
        assert!(wfs.read_text(&ws, "notes/renamed.md").is_ok());
        assert!(wfs.read_text(&ws, "notes/hello.md").is_err());
        wfs.delete(&ws, "notes", true).unwrap();
        assert!(wfs.stat(&ws, "notes").is_err());
    }

    #[test]
    fn rejects_dotdot_traversal() {
        let (_tmp, _data, wfs, ws) = tmp_data();
        let err = wfs.read_text(&ws, "../../etc/passwd").unwrap_err();
        match err {
            CoreError::Forbidden(_) | CoreError::BadRequest(_) | CoreError::Coded { .. } => {}
            other => panic!("unexpected: {other}"),
        }
        let err = wfs.write_text(&ws, "../escape.txt", "x").unwrap_err();
        match err {
            CoreError::Forbidden(_) | CoreError::BadRequest(_) | CoreError::Coded { .. } => {}
            other => panic!("unexpected: {other}"),
        }
        let err = wfs.list(&ws, "foo/../../..", 1).unwrap_err();
        match err {
            CoreError::Forbidden(_) | CoreError::BadRequest(_) | CoreError::Coded { .. } => {}
            other => panic!("unexpected: {other}"),
        }
    }

    #[test]
    fn rejects_absolute_paths() {
        let (_tmp, _data, wfs, ws) = tmp_data();
        for p in ["/etc/passwd", "\\Windows\\System32", "C:\\secrets"] {
            let err = wfs.stat(&ws, p).unwrap_err();
            match err {
                CoreError::Forbidden(_) | CoreError::BadRequest(_) | CoreError::Coded { .. } => {}
                other => panic!("path {p}: unexpected {other}"),
            }
        }
    }

    #[test]
    fn rejects_binary_and_huge() {
        let (_tmp, _data, wfs, ws) = tmp_data();
        let huge = "x".repeat((WORKS_MAX_FILE_BYTES as usize) + 1);
        let err = wfs.write_text(&ws, "big.txt", &huge).unwrap_err();
        assert!(matches!(
            err,
            CoreError::BadRequest(_) | CoreError::Coded { .. }
        ));
        let err = wfs
            .write_text(&ws, "bin.txt", "a\0b")
            .unwrap_err();
        assert!(matches!(
            err,
            CoreError::BadRequest(_) | CoreError::Coded { .. }
        ));
    }

    #[test]
    fn symlink_escape_rejected_on_read() {
        let (_tmp, data, wfs, ws) = tmp_data();
        let jail = wfs.workspace_root(&ws).unwrap();
        // Create a file outside jail
        let outside = data.root().join("outside-secret.txt");
        fs::write(&outside, "secret").unwrap();
        // Symlink inside jail pointing out
        // （unix 侧引用 _link；windows 侧无 symlink、变量看似未用——勿改名）
        let _link = jail.join("leak");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, &_link).unwrap();
            let err = wfs.read_text(&ws, "leak").unwrap_err();
            match err {
                CoreError::Forbidden(_)
                | CoreError::BadRequest(_)
                | CoreError::NotFound(_)
                | CoreError::Coded { .. } => {}
                other => panic!("unexpected: {other}"),
            }
            let err = wfs.stat(&ws, "leak").unwrap_err();
            match err {
                CoreError::Forbidden(_)
                | CoreError::BadRequest(_)
                | CoreError::NotFound(_)
                | CoreError::Coded { .. } => {}
                other => panic!("unexpected: {other}"),
            }
        }
    }

    #[test]
    fn normalize_rel_path_unit() {
        assert_eq!(normalize_rel_path("").unwrap(), "");
        assert_eq!(normalize_rel_path("./a/b").unwrap(), "a/b");
        assert_eq!(normalize_rel_path("a//b/./c").unwrap(), "a/b/c");
        assert!(normalize_rel_path("/etc/passwd").is_err());
        assert!(normalize_rel_path("../x").is_err());
        assert!(normalize_rel_path("a/../../b").is_err());
    }
}

#[cfg(test)]
mod job_store_tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    mod tempfile_shim {
        use super::*;
        pub struct Tmp {
            path: PathBuf,
        }
        impl Tmp {
            pub fn new() -> Self {
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                let path = std::env::temp_dir().join(format!("kaleido-jobs-test-{nanos}"));
                fs::create_dir_all(&path).unwrap();
                Self { path }
            }
            pub fn path(&self) -> &Path {
                &self.path
            }
        }
        impl Drop for Tmp {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    fn store_with_max(max: usize) -> (tempfile_shim::Tmp, JobStore) {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).expect("data root");
        // Explicit max avoids races on process-wide KALEIDO_MAX_CONCURRENT_JOBS.
        let store = JobStore::with_max_concurrent(data, max);
        (tmp, store)
    }

    #[test]
    fn normalize_status_legacy_map() {
        assert_eq!(normalize_job_status("done"), "succeeded");
        assert_eq!(normalize_job_status("error"), "failed");
        assert_eq!(normalize_job_status("stopped"), "cancelled");
        assert_eq!(normalize_job_status("running"), "running");
        assert_eq!(normalize_job_status("queued"), "queued");
    }

    #[test]
    fn try_start_rate_limits_at_max() {
        let (_tmp, store) = store_with_max(1);
        let a = store
            .try_start("chat", "u1", "ws1", None, json!({}))
            .unwrap();
        assert_eq!(a.status, "running");
        let err = store
            .try_start("chat", "u1", "ws1", None, json!({}))
            .unwrap_err();
        assert!(matches!(err, CoreError::RateLimited(_)));
        store.finish(&a.run_id, "done");
        let b = store
            .try_start("chat", "u1", "ws1", None, json!({}))
            .unwrap();
        assert_eq!(normalize_job_status(&store.get(&b.run_id).unwrap().status), "running");
        // finished job maps done → succeeded on disk
        let finished = store.get(&a.run_id).unwrap();
        assert_eq!(normalize_job_status(&finished.status), "succeeded");
    }

    #[test]
    fn create_queues_when_at_capacity() {
        let (_tmp, store) = store_with_max(1);
        let a = store
            .create("background", "u1", "ws1", json!({"n": 1}), None, None)
            .unwrap();
        assert_eq!(a.status, "running");
        let b = store
            .create("book_travel", "u1", "ws1", json!({"n": 2}), None, None)
            .unwrap();
        assert_eq!(b.status, "queued");
        assert_eq!(store.queued_count(), 1);
        assert_eq!(store.running_count(), 1);

        // finish a → b promoted
        store.finish(&a.run_id, "succeeded");
        let b2 = store.get(&b.run_id).unwrap();
        assert_eq!(normalize_job_status(&b2.status), "running");
        assert_eq!(store.queued_count(), 0);
        assert_eq!(store.running_count(), 1);
    }

    #[test]
    fn list_filter_and_cancel() {
        let (_tmp, store) = store_with_max(2);
        let a = store
            .create("outline", "u1", "ws1", json!({}), None, None)
            .unwrap();
        let b = store
            .create("agent", "u1", "ws1", json!({}), None, None)
            .unwrap();
        let _c = store
            .create("other", "u2", "ws2", json!({}), None, None)
            .unwrap();

        let all = store
            .list(JobListFilter {
                limit: 50,
                ..Default::default()
            })
            .unwrap();
        assert!(all.len() >= 3);

        let outlines = store
            .list(JobListFilter {
                kind: Some("outline".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(outlines.len(), 1);
        assert_eq!(outlines[0].run_id, a.run_id);

        let cancelled = store.cancel(&b.run_id).unwrap();
        assert_eq!(normalize_job_status(&cancelled.status), "cancelled");
        // idempotent
        let again = store.cancel(&b.run_id).unwrap();
        assert_eq!(normalize_job_status(&again.status), "cancelled");

        let by_status = store
            .list(JobListFilter {
                status: Some("cancelled".into()),
                limit: 10,
                ..Default::default()
            })
            .unwrap();
        assert!(by_status.iter().any(|j| j.run_id == b.run_id));
    }

    #[test]
    fn push_event_and_complete() {
        let (_tmp, store) = store_with_max(2);
        let j = store
            .create("noop", "u1", "ws1", json!({"mode": "test"}), None, None)
            .unwrap();
        store
            .push_event(
                &j.run_id,
                JobEvent::progress("halfway", 0.5),
                Some(0.5),
                Some("cursor-1".into()),
            )
            .unwrap();
        let mid = store.get(&j.run_id).unwrap();
        assert_eq!(mid.progress, Some(0.5));
        assert_eq!(mid.cursor.as_deref(), Some("cursor-1"));
        assert!(mid.events.iter().any(|e| e.event_type == "progress"));

        let done = store
            .complete(&j.run_id, "succeeded", Some(json!({"ok": true})), None)
            .unwrap();
        assert_eq!(normalize_job_status(&done.status), "succeeded");
        assert_eq!(done.progress, Some(1.0));
        assert_eq!(done.result, Some(json!({"ok": true})));
        assert!(is_terminal_job_status(&done.status));
    }

    #[test]
    fn finish_legacy_done_error_stopped() {
        let (_tmp, store) = store_with_max(2);
        let a = store
            .try_start("chat", "u1", "ws1", None, json!({}))
            .unwrap();
        store.finish(&a.run_id, "done");
        assert_eq!(
            normalize_job_status(&store.get(&a.run_id).unwrap().status),
            "succeeded"
        );

        let b = store
            .try_start("chat", "u1", "ws1", None, json!({}))
            .unwrap();
        store.finish(&b.run_id, "error");
        assert_eq!(
            normalize_job_status(&store.get(&b.run_id).unwrap().status),
            "failed"
        );

        let c = store
            .try_start("chat", "u1", "ws1", None, json!({}))
            .unwrap();
        store.finish(&c.run_id, "stopped");
        assert_eq!(
            normalize_job_status(&store.get(&c.run_id).unwrap().status),
            "cancelled"
        );
    }

    #[test]
    fn persist_survives_reload() {
        let tmp = tempfile_shim::Tmp::new();
        // P10: 不再 set_var（进程级全局态与并行测试竞态）；容量改显式构造，语义不变。
        let data = DataRoot::new(tmp.path()).unwrap();
        let store = JobStore::with_max_concurrent(data.clone(), 2);
        let j = store
            .create("background", "u1", "ws1", json!({"x": 1}), None, None)
            .unwrap();
        let id = j.run_id.clone();
        drop(store);

        let store2 = JobStore::with_max_concurrent(data, 2);
        let reloaded = store2.get(&id).unwrap();
        assert_eq!(reloaded.kind, "background");
        assert_eq!(normalize_job_status(&reloaded.status), "running");
        assert_eq!(reloaded.payload, Some(json!({"x": 1})));
    }

    #[test]
    fn max_concurrent_clamped_to_4() {
        // P10 根治 flake：原实现 set_var("99") + JobStore::new()，进程级 env 与并行测试
        // （persist_survives_reload 曾 set_var("2")）竞态导致偶发红。现直测解析纯函数，
        // 语义等价（new() = parse_max_concurrent(env) → with_max_concurrent 内同款 clamp）。
        assert_eq!(parse_max_concurrent(Some("99")), 4);
        assert_eq!(parse_max_concurrent(Some("3")), 3);
        assert_eq!(parse_max_concurrent(Some("2")), 2);
        // [并行 agent 2026-08-17] JobStore 并发上限 2→4（semantics 合入）
        assert_eq!(parse_max_concurrent(Some("4")), 4);
        // 边界：下限钳到 1；解析失败/未设置回落默认 2
        assert_eq!(parse_max_concurrent(Some("0")), 1);
        assert_eq!(parse_max_concurrent(Some("-5")), 2);
        assert_eq!(parse_max_concurrent(Some("abc")), 2);
        assert_eq!(parse_max_concurrent(None), 2);
    }

    /// P8: 并发水位 metrics —— create 抬升峰值、finish/cancel 计入终态计数。
    #[test]
    fn jobs_metrics_track_peak_and_terminals() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let store = JobStore::with_max_concurrent(data, 2);
        let m = |s: &JobStore| s.metrics.clone();

        let a = store.create("t", "u", "w", json!({}), None, None).unwrap();
        let b = store.create("t", "u", "w", json!({}), None, None).unwrap();
        let c = store.create("t", "u", "w", json!({}), None, None).unwrap(); // queued（并发=2 满）
        assert_eq!(m(&store).peak_running.load(Ordering::Relaxed), 2);
        assert_eq!(store.queued_count(), 1);
        assert_eq!(m(&store).total_created.load(Ordering::Relaxed), 3);

        store.finish(&a.run_id, "succeeded");
        assert_eq!(m(&store).total_succeeded.load(Ordering::Relaxed), 1);
        // 槽位释放 → c 被提升，峰值不变仍为 2
        assert_eq!(store.running_count(), 2);

        store.cancel(&b.run_id).unwrap();
        assert_eq!(m(&store).total_cancelled.load(Ordering::Relaxed), 1);

        store.finish(&c.run_id, "failed");
        assert_eq!(m(&store).total_failed.load(Ordering::Relaxed), 1);
        assert_eq!(store.running_count(), 0);

        // legacy 状态码映射也计入（done→succeeded, error→failed）
        let d = store.create("t", "u", "w", json!({}), None, None).unwrap();
        store.finish(&d.run_id, "done");
        assert_eq!(m(&store).total_succeeded.load(Ordering::Relaxed), 2);
        assert!(m(&store).boot_at_unix.load(Ordering::Relaxed) > 0);

        // P10: complete() 也是终态路径（director-plan 等后台任务专用），必须同样计数
        let e = store.create("t", "u", "w", json!({}), None, None).unwrap();
        store
            .complete(&e.run_id, "succeeded", Some(json!({"ok": true})), None)
            .unwrap();
        assert_eq!(m(&store).total_succeeded.load(Ordering::Relaxed), 3);
        // complete 对已取消 job 的迟到调用不复活、也不重复计数
        let f = store.create("t", "u", "w", json!({}), None, None).unwrap();
        store.cancel(&f.run_id).unwrap();
        store
            .complete(&f.run_id, "succeeded", Some(json!({})), None)
            .unwrap();
        assert_eq!(m(&store).total_cancelled.load(Ordering::Relaxed), 2);
        assert_eq!(m(&store).total_succeeded.load(Ordering::Relaxed), 3);
    }

    /// P10: totals 跨重启持久化 —— metrics.json 落盘 + boot 恢复；
    /// peak_running 保持进程作用域（不恢复，/health 语义为 since_boot）。
    #[test]
    fn jobs_metrics_persist_across_restart() {
        let tmp = tempfile_shim::Tmp::new();
        let data = DataRoot::new(tmp.path()).unwrap();
        let store = JobStore::with_max_concurrent(data.clone(), 2);

        let a = store.create("t", "u", "w", json!({}), None, None).unwrap();
        store.finish(&a.run_id, "succeeded");
        let b = store.create("t", "u", "w", json!({}), None, None).unwrap();
        store.cancel(&b.run_id).unwrap();

        // 快照文件已落盘
        let snap: JobMetricsSnapshot = serde_json::from_str(
            &fs::read_to_string(data.jobs_dir().join(JobMetrics::FILE_NAME)).unwrap(),
        )
        .unwrap();
        assert_eq!(snap.total_created, 2);
        assert_eq!(snap.total_succeeded, 1);
        assert_eq!(snap.total_cancelled, 1);

        drop(store);
        let store2 = JobStore::with_max_concurrent(data, 2);
        let m = |s: &JobStore| s.metrics.clone();
        assert_eq!(store2.metrics.total_created.load(Ordering::Relaxed), 2);
        assert_eq!(store2.metrics.total_succeeded.load(Ordering::Relaxed), 1);
        assert_eq!(store2.metrics.total_cancelled.load(Ordering::Relaxed), 1);
        assert_eq!(store2.metrics.total_failed.load(Ordering::Relaxed), 0);
        // peak 是本进程水位：新 boot 从 0 起算
        assert_eq!(store2.metrics.peak_running.load(Ordering::Relaxed), 0);
        // 累计基数上继续累加
        let c = store2.create("t", "u", "w", json!({}), None, None).unwrap();
        assert_eq!(store2.metrics.total_created.load(Ordering::Relaxed), 3);
        store2.finish(&c.run_id, "error");
        assert_eq!(m(&store2).total_failed.load(Ordering::Relaxed), 1);
    }
}

/// [审计修复 核心存储与认证-6] validate_llm_base_url 的 IP 规范化回归测试。
/// 直测纯函数内核（白名单显式注入），避免进程级 env 竞态（P10 惯例）。
#[cfg(test)]
mod llm_base_url_ssrf_tests {
    use super::*;

    fn rejected(raw: &str) -> CoreError {
        validate_llm_base_url_with_allow(raw, false, &[]).unwrap_err()
    }

    fn allowed(raw: &str) -> bool {
        validate_llm_base_url_with_allow(raw, false, &[]).is_ok()
    }

    #[test]
    fn integer_and_mixed_radix_ip_literals_blocked() {
        // 十进制整数：127.0.0.1 / 10.0.0.1 / 169.254.169.254（云元数据）
        assert!(matches!(rejected("http://2130706433/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://167772161/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://2852039166/"), CoreError::BadRequest(_)));
        // 十六进制
        assert!(matches!(rejected("http://0x7f000001/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://0x7f.0.0.1/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://0x7f.1/"), CoreError::BadRequest(_))); // → 127.0.0.1
        // 八进制
        assert!(matches!(rejected("http://0177.0.0.1/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://0177.0000.0000.0001/"), CoreError::BadRequest(_)));
        // 缺省段短形式
        assert!(matches!(rejected("http://127.1/"), CoreError::BadRequest(_))); // → 127.0.0.1
        assert!(matches!(rejected("http://10.1/"), CoreError::BadRequest(_))); // → 10.0.0.1
        assert!(matches!(rejected("http://192.168.1/"), CoreError::BadRequest(_))); // → 192.168.0.1
        assert!(matches!(rejected("http://172.20.1/"), CoreError::BadRequest(_))); // → 172.20.0.1
        // 0/8 与 unspecified：`http://0/`、`http://0x7f/` → 0.0.0.x（多数协议栈打回本机）
        assert!(matches!(rejected("http://0.0.0.0/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://0/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://0x7f/"), CoreError::BadRequest(_)));
        // 尾点 / 端口变体
        assert!(matches!(rejected("http://127.0.0.1./"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://2130706433:8080/"), CoreError::BadRequest(_)));
        // 常规字面量回归（原实现已拦截，规范化后仍拦截）
        assert!(matches!(rejected("http://127.0.0.1/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://172.16.0.1/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://10.255.255.255/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://169.254.169.254/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://192.168.1.100/"), CoreError::BadRequest(_)));
    }

    #[test]
    fn ipv6_and_embedded_ipv4_blocked() {
        assert!(matches!(rejected("http://[::1]/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://[0:0:0:0:0:0:0:1]/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://[::ffff:127.0.0.1]/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://[::ffff:10.0.0.1]:8080/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://[::127.0.0.1]/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://[fe80::1]/"), CoreError::BadRequest(_))); // link-local
        assert!(matches!(rejected("http://[fd00::1]/"), CoreError::BadRequest(_))); // unique local
    }

    #[test]
    fn userinfo_disguise_blocked() {
        // 实际连接的是 `@` 之后的主机
        assert!(matches!(rejected("http://user:pass@10.0.0.1:8080/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://user@127.0.0.1/"), CoreError::BadRequest(_)));
    }

    #[test]
    fn localhost_family_blocked() {
        assert!(matches!(rejected("http://localhost:11434/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://LOCALHOST/"), CoreError::BadRequest(_)));
        // RFC 6761：*.localhost 由解析器保证指向回环
        assert!(matches!(rejected("http://api.localhost/"), CoreError::BadRequest(_)));
    }

    #[test]
    fn ambiguous_numeric_hosts_fail_closed() {
        // 浏览器同样视为非法 URL 的数字形 host → 一律拒绝（fail-closed）
        assert!(matches!(rejected("http://300.1.1.1/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://0x/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://99999999999999/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http://1..2/"), CoreError::BadRequest(_)));
    }

    #[test]
    fn public_hosts_still_allowed() {
        assert!(allowed("https://api.openai.com/v1"));
        assert!(allowed("https://api.anthropic.com"));
        assert!(allowed("https://generativelanguage.googleapis.com"));
        assert!(allowed("http://8.8.8.8/"));
        assert!(allowed("http://1.1.1.1:8080/"));
        // 172.16/12 之外是公网
        assert!(allowed("https://172.32.0.1/"));
        // 缺省段短形式 → 公网地址放行（与浏览器实际连接语义一致）
        assert!(allowed("http://172.16/")); // → 172.0.0.16
        assert!(allowed("http://134744072/")); // → 8.8.8.8
        // 十六进制风格但末段非数字 → 普通域名（不触发 IPv4 解析，不误伤）
        assert!(allowed("http://dead.beef.be/"));
        // IP 形前缀 + 域名后缀 → 普通域名
        assert!(allowed("http://10.0.0.1.example.com/"));
        // 空 base_url 维持原语义：跳过校验
        assert!(allowed(""));
        assert!(allowed("   "));
        // 带 query/fragment
        assert!(allowed("http://example.com:443?x=1#frag"));
    }

    #[test]
    fn scheme_and_authority_edges() {
        assert!(matches!(rejected("ftp://127.0.0.1/"), CoreError::BadRequest(_)));
        assert!(matches!(rejected("http:///nohost"), CoreError::BadRequest(_)));
    }

    #[test]
    fn allow_list_matches_canonical_forms() {
        let allow = |hosts: &[&str]| -> Vec<String> { hosts.iter().map(|s| s.to_string()).collect() };
        // 常规白名单
        assert!(validate_llm_base_url_with_allow("http://127.0.0.1:11434/", false, &allow(&["127.0.0.1"])).is_ok());
        assert!(validate_llm_base_url_with_allow("http://localhost:11434/", false, &allow(&["localhost"])).is_ok());
        // 白名单条目写宽松形式 / URL 写整数形式，按规范化比对均可命中
        assert!(validate_llm_base_url_with_allow("http://127.0.0.1/", false, &allow(&["0x7f000001"])).is_ok());
        assert!(validate_llm_base_url_with_allow("http://2130706433:11434/", false, &allow(&["127.0.0.1"])).is_ok());
        assert!(validate_llm_base_url_with_allow("http://[::1]/", false, &allow(&["::1"])).is_ok());
        assert!(validate_llm_base_url_with_allow("http://10.0.0.5:8080/", false, &allow(&["10.0.0.5"])).is_ok());
        // 未列入白名单的私网仍拒绝
        assert!(validate_llm_base_url_with_allow("http://10.0.0.6/", false, &allow(&["10.0.0.5"])).is_err());
        // allow_all=1 兼容旧「全放行」语义
        assert!(validate_llm_base_url_with_allow("http://2130706433/", true, &[]).is_ok());
        assert!(validate_llm_base_url_with_allow("http://localhost/", true, &[]).is_ok());
    }
}

