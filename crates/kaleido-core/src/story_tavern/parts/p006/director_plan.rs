//! p006 测试集 · director_plan（R6 自 p006.rs 机械迁入，仅 use 路径改绝对）
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
mod director_plan_tests {
    use crate::story_tavern::parts::p005::tests::{minimal_pack, minimal_session};
    use crate::story_tavern::parts::*;

    #[test]
    fn director_run_policy_g8_defaults() {
        // G8: 缺字段反序列化回退默认值（fail_forward / balanced / audit_only / 5）
        let legacy = r#"{"mode":"interval","intervalTurns":2}"#;
        let rp: DirectorRunPolicy = serde_json::from_str(legacy).unwrap();
        assert_eq!(rp.mode, "interval");
        assert_eq!(rp.interval_turns, 2);
        assert_eq!(rp.failure_policy, "fail_forward");
        assert_eq!(rp.pacing_curve, "");
        assert_eq!(rp.event_frequency, "balanced");
        assert_eq!(rp.rule_visibility_mode, "audit_only");
        assert_eq!(rp.branch_planning_turns, 5);
        // 显式值可覆盖默认
        let full = r#"{"mode":"manual","failurePolicy":"hard_failure","eventFrequency":"frequent","ruleVisibilityMode":"public_roll","branchPlanningTurns":7}"#;
        let rp2: DirectorRunPolicy = serde_json::from_str(full).unwrap();
        assert_eq!(rp2.failure_policy, "hard_failure");
        assert_eq!(rp2.event_frequency, "frequent");
        assert_eq!(rp2.rule_visibility_mode, "public_roll");
        assert_eq!(rp2.branch_planning_turns, 7);
    }

    #[test]
    fn director_module_refs_g11_disabled_default_false() {
        // G11: 旧数据无 *_disabled 字段 → 默认 false（模块启用，零回归）
        let legacy = r#"{"narrativeStyleId":"ns1","eventPackageIds":["pkg-a"]}"#;
        let mr: DirectorModuleRefs = serde_json::from_str(legacy).unwrap();
        assert_eq!(mr.narrative_style_id.as_deref(), Some("ns1"));
        assert!(!mr.narrative_style_disabled);
        assert!(!mr.event_packages_disabled);
        assert!(!mr.rule_system_disabled);
        assert!(!mr.actor_state_disabled);
        assert!(!mr.image_preset_disabled);
        // 显式 disabled=true 且保留原 ID（denova 语义）
        let full = r#"{"narrativeStyleId":"ns1","narrativeStyleDisabled":true,"actorStateDisabled":true}"#;
        let mr2: DirectorModuleRefs = serde_json::from_str(full).unwrap();
        assert_eq!(mr2.narrative_style_id.as_deref(), Some("ns1"));
        assert!(mr2.narrative_style_disabled);
        assert!(mr2.actor_state_disabled);
        assert!(!mr2.event_packages_disabled);
    }

    #[test]
    fn mainline_strength_soft_guidance_compat() {
        // G8: mainline_strength 兼容——soft 旧值与 soft_guidance 都应被前端/注入识别
        let soft: StageDirectorConfig = serde_json::from_str(r#"{"mainlineStrength":"soft"}"#).unwrap();
        assert_eq!(soft.mainline_strength, "soft");
        let sg: StageDirectorConfig = serde_json::from_str(r#"{"mainlineStrength":"soft_guidance"}"#).unwrap();
        assert_eq!(sg.mainline_strength, "soft_guidance");
        // 缺省 → balanced
        let d: StageDirectorConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(d.mainline_strength, "balanced");
    }

    #[test]
    fn director_plan_g1_three_docs_defaults() {
        // G1: 三文档缺省空串（旧数据零回归）
        let legacy = r#"{"goal":"主线推进","pressure":"高","cost":"牺牲","hitsBeats":["beat-1"],"createdTurn":1,"updatedTurn":1}"#;
        let plan: DirectorPlan = serde_json::from_str(legacy).unwrap();
        assert_eq!(plan.goal, "主线推进");
        assert_eq!(plan.plan, "");
        assert_eq!(plan.agent_brief, "");
        assert_eq!(plan.lore_context, "");
        assert_eq!(plan.last_run, None);
        // 显式值可设置
        let full = r#"{"goal":"g","plan":"计划正文","agentBrief":"本回合指令","loreContext":"世界观铺垫"}"#;
        let p2: DirectorPlan = serde_json::from_str(full).unwrap();
        assert_eq!(p2.plan, "计划正文");
        assert_eq!(p2.agent_brief, "本回合指令");
        assert_eq!(p2.lore_context, "世界观铺垫");
    }

    #[test]
    fn director_plan_g2_run_status_serialization() {
        // G2: ready/running/conflict 状态构造 + 序列化
        let r = DirectorPlanRunStatus::ready("推进主线");
        assert_eq!(r.status, "ready");
        assert_eq!(r.summary.as_deref(), Some("推进主线"));
        let json_r = serde_json::to_value(&r).unwrap();
        assert_eq!(json_r["status"], "ready");
        assert!(json_r["updatedAt"].is_string());
        let c = DirectorPlanRunStatus::conflict("LLM 失败");
        assert_eq!(c.status, "conflict");
        assert_eq!(c.error.as_deref(), Some("LLM 失败"));
        // 反序列化旧数据（无 last_run）→ None
        let legacy = r#"{"goal":"g"}"#;
        let plan: DirectorPlan = serde_json::from_str(legacy).unwrap();
        assert_eq!(plan.last_run, None);
        // 完整 plan + last_run round-trip
        let full = r#"{"goal":"g","lastRun":{"status":"ready","summary":"s","updatedAt":"2026-08-12T00:00:00Z"}}"#;
        let p2: DirectorPlan = serde_json::from_str(full).unwrap();
        assert_eq!(p2.last_run.as_ref().map(|x| x.status.as_str()), Some("ready"));
        assert_eq!(p2.last_run.as_ref().and_then(|x| x.summary.as_deref()), Some("s"));
    }

    #[test]
    fn fit_text_to_token_budget_basic() {
        // G4: 预算内原样返回
        let s = "短文本";
        assert_eq!(fit_text_to_token_budget(s, 100, 0.5), s);
        // 超预算：头尾保留 + 中间省略标记
        let long = "甲乙丙丁戊己庚辛壬癸";
        let out = fit_text_to_token_budget(long, 6, 0.5);
        assert!(out.contains("省略 4 字符"), "got: {out}");
        assert!(out.starts_with("甲乙丙"), "头部保留: {out}");
        assert!(out.ends_with("壬癸"), "尾部保留: {out}");
        // 空串 / 预算 0 → 空串
        assert_eq!(fit_text_to_token_budget("", 10, 0.5), "");
        assert_eq!(fit_text_to_token_budget("abc", 0, 0.5), "");
    }

    #[test]
    fn fit_text_to_token_budget_head_ratio_edge() {
        // head_ratio=1.0 → 全头部保留、无尾部
        let long = "abcdefgh";
        let out = fit_text_to_token_budget(long, 5, 1.0);
        assert!(out.starts_with("abcde"), "got: {out}");
        // head_ratio=0.0 → 全尾部保留
        let out2 = fit_text_to_token_budget(long, 5, 0.0);
        assert!(out2.ends_with("fgh"), "got: {out2}");
        // head_ratio 越界 clamp
        let out3 = fit_text_to_token_budget(long, 4, 2.0);
        assert!(out3.contains("省略"), "clamp 后仍裁剪: {out3}");
    }

    #[test]
    fn director_ledger_entry_serialization() {
        // G5: 账本条目 camelCase 序列化 + 缺省反序列化
        let e = DirectorLedgerEntry {
            source: "recent".into(),
            title: "最近剧情".into(),
            body_bytes: 2048,
            limit: 1024,
            included: false,
            note: "超预算，省略 800 字符".into(),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["source"], "recent");
        assert_eq!(v["bodyBytes"], 2048);
        assert_eq!(v["included"], false);
        let back: DirectorLedgerEntry = serde_json::from_value(v).unwrap();
        assert_eq!(back.source, "recent");
        assert_eq!(back.note, "超预算，省略 800 字符");
        // 缺省字段 → 默认值（bool 缺省 false = 未登记即未包含）
        let d: DirectorLedgerEntry = serde_json::from_str(r#"{"source":"s"}"#).unwrap();
        assert_eq!(d.body_bytes, 0);
        assert!(!d.included);
    }

    #[test]
    fn retain_guard_events_basic() {
        // G15: 未超窗 → 原样返回
        let ev = vec!["[high][人物] 甲".to_string(), "[med][节拍] 乙".to_string()];
        assert_eq!(retain_guard_events(&ev, 10), ev);
        // 超窗 → 裁剪到 max_recent，保留窗口尾巴
        let many: Vec<String> = (0..10).map(|i| format!("[med][d] 事件{i}")).collect();
        let kept = retain_guard_events(&many, 5);
        assert_eq!(kept.len(), 5);
        assert!(kept[0].contains("事件5"), "保留最近窗口: {kept:?}");
        // max_recent=0 → 空
        assert!(retain_guard_events(&many, 0).is_empty());
    }

    #[test]
    fn retain_guard_events_prefers_high() {
        // G15: 超窗时 med 先被淘汰，high 保底
        let events: Vec<String> = vec![
            "[med][d] 早".into(),
            "[high][人物] 关键".into(),
            "[med][d] 中".into(),
            "[med][d] 晚".into(),
        ];
        let kept = retain_guard_events(&events, 2);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().any(|e| e.starts_with("[high]")), "high 保底: {kept:?}");
        // 全部 high 时从最老淘汰
        let all_high: Vec<String> = (0..6).map(|i| format!("[high][d] 事件{i}")).collect();
        let kept2 = retain_guard_events(&all_high, 3);
        assert_eq!(kept2.len(), 3);
        assert!(kept2[0].contains("事件3"), "最老优先淘汰: {kept2:?}");
    }

    #[test]
    fn director_due_interval_hit() {
        // interval=3：turn 3 命中（turn%3==0 && last != Some(3)）
        assert!(director_due("interval", 3, 3, None));
        assert!(director_due("interval", 3, 6, Some(3)));
        // interval=1：每个已推进回合都命中
        assert!(director_due("interval", 1, 1, None));
        assert!(director_due("interval", 1, 5, Some(4)));
    }

    #[test]
    fn director_due_interval_miss() {
        assert!(!director_due("interval", 3, 1, None));
        assert!(!director_due("interval", 3, 2, None));
        assert!(!director_due("interval", 3, 4, Some(3)));
        // turn=0 不触发
        assert!(!director_due("interval", 3, 0, None));
    }

    #[test]
    fn director_due_interval_no_repeat() {
        // 同一回合刚生成过计划，不重复触发
        assert!(!director_due("interval", 3, 3, Some(3)));
        assert!(!director_due("interval", 1, 1, Some(1)));
    }

    #[test]
    fn director_due_interval_zero_defensive() {
        // interval_turns=0 防御：永不触发
        assert!(!director_due("interval", 0, 3, None));
        assert!(!director_due("interval", 0, 0, None));
    }

    #[test]
    fn opening_plan_due_first_turn_seeded() {
        // 首回合（turn=0）+ opening 已 seed + 无 plan → 触发开局导演规划
        assert!(opening_plan_due(0, false, true));
    }

    #[test]
    fn opening_plan_due_already_has_plan() {
        // 已有 plan → 幂等，不重复生成
        assert!(!opening_plan_due(0, true, true));
        assert!(!opening_plan_due(0, true, false));
    }

    #[test]
    fn opening_plan_due_not_first_turn_or_unseeded() {
        // 非首回合 → 不触发
        assert!(!opening_plan_due(1, false, true));
        assert!(!opening_plan_due(2, false, true));
        // opening 未 seed（空会话/无开场）→ 不触发
        assert!(!opening_plan_due(0, false, false));
    }

    #[test]
    fn director_due_manual_and_ondemand_false() {
        assert!(!director_due("manual", 3, 3, None));
        assert!(!director_due("manual", 1, 1, None));
        assert!(!director_due("on_demand", 3, 3, None));
        assert!(!director_due("on_demand", 1, 6, Some(3)));
    }

    #[test]
    fn director_due_invalid_mode_false() {
        assert!(!director_due("", 3, 3, None));
        assert!(!director_due("every_turn", 3, 3, None));
        assert!(!director_due("  ", 3, 3, None));
    }

    #[test]
    fn director_plan_serde_roundtrip() {
        let plan = DirectorPlan {
            goal: "引出铜铃来信".into(),
            pressure: Some("门外脚步已停".into()),
            cost: Some("若拆信则再无回头路".into()),
            hits_beats: vec!["信的存在不可抹除".into()],
            created_turn: 3,
            updated_turn: 3,
            plan: String::new(),
            agent_brief: String::new(),
            lore_context: String::new(),
            last_run: None,
        };
        let raw = serde_json::to_string(&plan).unwrap();
        let back: DirectorPlan = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.goal, "引出铜铃来信");
        assert_eq!(back.pressure.as_deref(), Some("门外脚步已停"));
        assert_eq!(back.cost.as_deref(), Some("若拆信则再无回头路"));
        assert_eq!(back.hits_beats, vec!["信的存在不可抹除".to_string()]);
        assert_eq!(back.created_turn, 3);
        assert_eq!(back.updated_turn, 3);
        // camelCase JSON 字段名
        assert!(raw.contains("\"hitsBeats\""));
        assert!(raw.contains("\"createdTurn\""));
        assert!(raw.contains("\"updatedTurn\""));
    }

    #[test]
    fn director_plan_missing_fields_default() {
        // 缺省字段（pressure/cost/hits_beats/createdTurn/updatedTurn）走 serde default
        let raw = r#"{"goal":"引出铜铃来信"}"#;
        let plan: DirectorPlan = serde_json::from_str(raw).unwrap();
        assert_eq!(plan.goal, "引出铜铃来信");
        assert!(plan.pressure.is_none());
        assert!(plan.cost.is_none());
        assert!(plan.hits_beats.is_empty());
        assert_eq!(plan.created_turn, 0);
        assert_eq!(plan.updated_turn, 0);
    }

    #[test]
    fn session_defaults_director_plan_none() {
        // 旧 session 无 director_plan/director_pending 字段 → 正常加载（serde default）
        let raw = r#"{
            "sessionId": "tavern-session-old",
            "packId": "pack-1",
            "playable": "P3",
            "playMode": "mainline",
            "contentTier": "standard",
            "turn": 5
        }"#;
        let sess: TavernSession = serde_json::from_str(raw).unwrap();
        assert_eq!(sess.session_id, "tavern-session-old");
        assert!(sess.director_plan.is_none());
        assert!(!sess.director_pending);
        assert!(sess.director_task.is_none());
    }

    // ─── G13/G14 DirectorTaskGroup 串行任务组 单测 ───────────────────────────

    #[test]
    fn task_group_same_key_second_start_returns_false() {
        let group = std::sync::Arc::new(DirectorTaskGroup::new());
        let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let t = {
            let group = group.clone();
            let gate = gate.clone();
            let release = release.clone();
            std::thread::spawn(move || {
                let started = group.start("session-a", "director_plan_update", || {
                    gate.wait(); // 信号：任务已进入运行表
                    while !release.load(std::sync::atomic::Ordering::SeqCst) {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                });
                assert!(started);
            })
        };
        gate.wait(); // 等任务 A 真正在跑

        // 同 key 第二个 start → false，且闭包不执行
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran2 = ran.clone();
        let started2 = group.start("session-a", "director_plan_update_2", || {
            ran2.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        assert!(!started2);
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(group.is_running("session-a"));
        assert_eq!(
            group.current_task("session-a"),
            Some("director_plan_update".into())
        );

        release.store(true, std::sync::atomic::Ordering::SeqCst);
        t.join().unwrap();

        // 结束后 key 释放，可再次启动
        assert!(!group.is_running("session-a"));
        let started3 = group.start("session-a", "director_plan_update_3", || {});
        assert!(started3);
        assert!(!group.is_running("session-a"));
    }

    #[test]
    fn task_group_panic_caught_does_not_crash_thread() {
        let group = DirectorTaskGroup::new();
        let started = group.start("panic-key", "task-panic", || {
            panic!("deliberate director task panic");
        });
        assert!(started);
        // panic 被捕获后运行表清理，key 可复用（不炸进程）
        assert!(!group.is_running("panic-key"));
        let ok = group.start("panic-key", "task-after", || {});
        assert!(ok);
        assert!(!group.is_running("panic-key"));
    }

    #[test]
    fn task_group_with_task_registers_current_task() {
        let group = DirectorTaskGroup::new();
        let mut observed: Option<String> = None;
        group.start("wk", "initial", || {
            group.with_task("updated");
            observed = group.current_task("wk");
        });
        assert_eq!(observed, Some("updated".into()));
        assert!(!group.is_running("wk"));
    }

    #[test]
    fn task_group_acquire_release_primitives() {
        let group = DirectorTaskGroup::new();
        assert!(group.acquire("a1", "t1"));
        assert!(!group.acquire("a1", "t2"));
        assert_eq!(group.current_task("a1"), Some("t1".into()));
        assert!(group.is_running("a1"));
        group.release("a1");
        assert!(!group.is_running("a1"));
        assert!(group.acquire("a1", "t3"));
        group.release("a1");
    }

    #[test]
    fn task_group_different_keys_do_not_block_each_other() {
        // denova GoKeyed 语义：按 key 串行，不同 key 互不阻塞
        let group = std::sync::Arc::new(DirectorTaskGroup::new());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for i in 0..2 {
            let group = group.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let key = format!("key-{i}");
                let started =
                    group.start(key, format!("task-{i}"), || {
                        barrier.wait(); // 两个任务同时进入 → 不同 key 并行执行
                    });
                assert!(started);
            }));
        }
        barrier.wait();
        for h in handles {
            h.join().unwrap();
        }
        assert!(!group.is_running("key-0"));
        assert!(!group.is_running("key-1"));
    }

    #[test]
    fn session_loads_existing_director_plan() {
        // 落库后的 session 可回读 director_plan
        let raw = r#"{
            "sessionId": "tavern-session-x",
            "packId": "pack-1",
            "playable": "P3",
            "playMode": "mainline",
            "contentTier": "standard",
            "turn": 6,
            "directorPlan": {
                "goal": "推进到无名信",
                "pressure": "脚步逼近",
                "cost": "暴露行踪",
                "hitsBeats": ["信的存在不可抹除"],
                "createdTurn": 3,
                "updatedTurn": 6
            },
            "directorPending": true
        }"#;
        let sess: TavernSession = serde_json::from_str(raw).unwrap();
        let plan = sess.director_plan.expect("plan loaded");
        assert_eq!(plan.goal, "推进到无名信");
        assert_eq!(plan.hits_beats, vec!["信的存在不可抹除".to_string()]);
        assert_eq!(plan.updated_turn, 6);
        assert!(sess.director_pending);
    }

    // ─── S5: 事件卡包 ──────────────────────────────────────────────

    fn event_pack_fixture() -> StoryPack {
        let mut pack = minimal_pack();
        pack.stage_director.modules.event_package_ids = vec!["pkg-a".into(), "pkg-b".into()];
        pack.event_packages = vec![
            EventPackage {
                id: "pkg-a".into(),
                name: "A".into(),
                description: None,
                enabled: true,
                cards: vec![
                    TellerEventCard {
                        id: "a1".into(),
                        title: "A1".into(),
                        prompt: "p1".into(),
                        weight: 1,
                        enabled: true,
                        once_per_session: false,
                        used_in_session: false,
                        type_name: "外门考核".into(),
                        category: "打脸".into(),
                        tags: vec!["门派".into(), "考核".into()],
                        intensity: "medium".into(),
                        cooldown_turns: 2,
                        chapter_range: vec![],
                    },
                    TellerEventCard {
                        id: "a2".into(),
                        title: "A2".into(),
                        prompt: "p2".into(),
                        weight: 9,
                        enabled: true,
                        once_per_session: false,
                        used_in_session: false,
                        type_name: "误入香闺".into(),
                        category: "恋爱".into(),
                        tags: vec!["闺房".into(), "幽会".into()],
                        intensity: "low".into(),
                        cooldown_turns: 3,
                        chapter_range: vec![],
                    },
                ],
            },
            EventPackage {
                id: "pkg-b".into(),
                name: "B".into(),
                description: None,
                enabled: true,
                cards: vec![TellerEventCard {
                    id: "b1".into(),
                    title: "B1".into(),
                    prompt: "p3".into(),
                    weight: 1,
                    enabled: true,
                    once_per_session: false,
                    used_in_session: false,
                    type_name: "秘境争夺".into(),
                    category: "秘境".into(),
                    tags: vec!["秘境".into()],
                    intensity: "high".into(),
                    cooldown_turns: 0,
                    chapter_range: vec![],
                }],
            },
        ];
        pack
    }

    #[test]
    fn pick_event_card_empty_pack_returns_none() {
        let pack = minimal_pack(); // 无 eventPackages
        assert!(pick_event_card(&pack, 42, 1, None, None).is_none());
    }

    #[test]
    fn pick_event_card_disabled_package_excluded() {
        let mut pack = event_pack_fixture();
        pack.event_packages[0].enabled = false;
        let (pkg, card) = pick_event_card(&pack, 7, 1, None, None).expect("pkg-b card");
        assert_eq!(pkg.id, "pkg-b");
        assert_eq!(card.id, "b1");
    }

    #[test]
    fn pick_event_card_module_ids_filter() {
        let mut pack = event_pack_fixture();
        pack.stage_director.modules.event_package_ids = vec!["pkg-a".into()];
        let (pkg, _) = pick_event_card(&pack, 7, 1, None, None).expect("pkg-a card");
        assert_eq!(pkg.id, "pkg-a");
    }

    #[test]
    fn pick_event_card_weighted_prefers_high_weight() {
        let pack = event_pack_fixture();
        let mut a2_hits = 0;
        for seed in 0..200u64 {
            let (_, card) = pick_event_card(&pack, seed, 1, None, None).expect("card");
            if card.id == "a2" {
                a2_hits += 1;
            }
        }
        // 权重 9 vs 1(+1) → a2 应占绝大多数
        assert!(a2_hits > 150, "a2 hits {a2_hits}");
    }

    #[test]
    fn pick_event_card_once_per_session_excludes_used() {
        let mut pack = event_pack_fixture();
        pack.event_packages[0].cards[0].once_per_session = true;
        pack.event_packages[0].cards[0].used_in_session = true;
        let (_, card) = pick_event_card(&pack, 1, 1, None, None).expect("card");
        assert_ne!(card.id, "a1");
    }

    /// 冷却语义：cooldown_turns>0 且距上次抽同卡 < 冷却回合 → 排除该卡。
    /// 夹具 a2 卡 cooldown_turns=3 且 weight=9（超高权重）；last_event 记 a2 在 turn=10 抽出，
    /// next_turn=11 时差值 1 < 3 仍在冷却期 → 无论 seed 都抽不到 a2。
    #[test]
    fn pick_event_card_cooldown_excludes_recent_card() {
        let pack = event_pack_fixture();
        let last = EventLogEntry {
            turn: 10,
            package_id: "pkg-a".into(),
            card_id: "a2".into(),
            title: "A2".into(),
            prompt: "p2".into(),
            created_at: String::new(),
            type_name: "误入香闺".into(),
            category: "恋爱".into(),
            intensity: "low".into(),
        };
        let mut a2_hits = 0;
        for seed in 0..200u64 {
            let (_, card) = pick_event_card(&pack, seed, 11, Some(&last), None).expect("card");
            if card.id == "a2" {
                a2_hits += 1;
            }
        }
        // 冷却期（11-10=1 < 3）内 a2 绝不能出现
        assert_eq!(a2_hits, 0, "a2 should be in cooldown");
        // 兜底：明确断言抽到的是非冷却卡
        let (_, card) = pick_event_card(&pack, 7, 11, Some(&last), None).expect("card");
        assert_ne!(card.id, "a2");
    }

    /// 冷却结束后恢复：next_turn 距上次抽卡 >= cooldown_turns 后同卡可再次抽到。
    /// 夹具 a2 卡 cooldown_turns=3：last_event 记 a2 在 turn=10 抽出，next_turn=13 时
    /// 差值 3 >= 3 冷却已过 → a2 回归高权重候选（占绝大多数）。
    #[test]
    fn pick_event_card_cooldown_recovers_after_elapsed() {
        let pack = event_pack_fixture();
        let last = EventLogEntry {
            turn: 10,
            package_id: "pkg-a".into(),
            card_id: "a2".into(),
            title: "A2".into(),
            prompt: "p2".into(),
            created_at: String::new(),
            type_name: "误入香闺".into(),
            category: "恋爱".into(),
            intensity: "low".into(),
        };
        let mut a2_hits = 0;
        for seed in 0..200u64 {
            let (_, card) = pick_event_card(&pack, seed, 13, Some(&last), None).expect("card");
            if card.id == "a2" {
                a2_hits += 1;
            }
        }
        // 冷却已过（13-10=3 >= 3）→ a2 恢复为高权重候选
        assert!(a2_hits > 150, "a2 hits {a2_hits}");
    }

    /// A2 按章过滤：卡标注 chapter_range 且当前章命中 → 正常选出；
    /// 当前章不命中 → 该卡排除；全部排除 → None（本回合不抽事件卡）。
    #[test]
    fn pick_event_card_chapter_range_filters_by_current_chapter() {
        let mut pack = event_pack_fixture();
        // a1/a2 标注 ch01-ch03，b1 无标注（旧数据兼容）
        pack.event_packages[0].cards[0].chapter_range = vec!["ch01".into(), "ch02".into()];
        pack.event_packages[0].cards[1].chapter_range = vec!["ch01".into(), "ch02".into()];
        // 当前章 ch01 → a1/a2 可抽（b1 无标注也可抽）→ 不会返回 None
        assert!(pick_event_card(&pack, 7, 1, None, Some("ch01")).is_some());
        // 当前章 ch03 → 标注卡 a1/a2 排除，仅剩无标注的 b1 → 抽到 b1
        let (pkg, card) = pick_event_card(&pack, 7, 1, None, Some("ch03")).expect("b1 应存活");
        assert_eq!(pkg.id, "pkg-b");
        assert_eq!(card.id, "b1");
        // 全部卡都标注且都不覆盖当前章 → None（本回合不抽）
        pack.event_packages[1].cards[0].chapter_range = vec!["ch05".into()];
        // ch03: a1/a2 排除（ch01-ch02），b1 排除（ch05）→ 无候选
        assert!(pick_event_card(&pack, 7, 1, None, Some("ch03")).is_none());
        // 当前章未知（None）→ 按旧行为不拦截
        assert!(pick_event_card(&pack, 7, 1, None, None).is_some());
    }

    #[test]
    fn event_package_serde_camel_case_roundtrip() {
        let raw = r#"{"id":"p","name":"包","enabled":true,"cards":[{"id":"c","title":"T","prompt":"P","weight":3,"enabled":true,"oncePerSession":true,"usedInSession":false,"typeName":"外门考核打脸","category":"打脸","tags":["门派","考核"],"intensity":"medium","cooldownTurns":2,"chapterRange":["ch01","ch03"]}]}"#;
        let pkg: EventPackage = serde_json::from_str(raw).unwrap();
        assert_eq!(pkg.cards[0].weight, 3);
        assert!(pkg.cards[0].once_per_session);
        // 新字段 G7：camelCase 映射 + 值落盘
        assert_eq!(pkg.cards[0].type_name, "外门考核打脸");
        assert_eq!(pkg.cards[0].category, "打脸");
        assert_eq!(pkg.cards[0].tags, vec!["门派".to_string(), "考核".to_string()]);
        assert_eq!(pkg.cards[0].intensity, "medium");
        assert_eq!(pkg.cards[0].cooldown_turns, 2);
        // A1：chapterRange camelCase 反序列化
        assert_eq!(pkg.cards[0].chapter_range, vec!["ch01".to_string(), "ch03".to_string()]);
        let back = serde_json::to_string(&pkg).unwrap();
        assert!(back.contains("\"oncePerSession\""));
        assert!(back.contains("\"usedInSession\""));
        // 新字段序列化回 camelCase
        assert!(back.contains("\"typeName\":\"外门考核打脸\""));
        assert!(back.contains("\"chapterRange\":[\"ch01\",\"ch03\"]"));
        assert!(back.contains("\"cooldownTurns\":2"));
    }

    #[test]
    fn event_package_missing_fields_default() {
        let raw = r#"{"id":"p"}"#;
        let pkg: EventPackage = serde_json::from_str(raw).unwrap();
        assert!(!pkg.enabled);
        assert!(pkg.cards.is_empty());
    }

    #[test]
    fn session_last_event_missing_defaults_none() {
        let raw = r#"{"sessionId":"tavern-session-le","packId":"p","playable":"P3","playMode":"mainline","contentTier":"standard","turn":1}"#;
        let sess: TavernSession = serde_json::from_str(raw).unwrap();
        assert!(sess.last_event.is_none());
    }

    #[test]
    fn session_last_event_roundtrip() {
        let mut s = minimal_session();
        s.last_event = Some(EventLogEntry {
            turn: 2,
            package_id: "pkg-a".into(),
            card_id: "a2".into(),
            title: "A2".into(),
            prompt: "p2".into(),
            created_at: "2026-08-05T00:00:00Z".into(),
            type_name: "误入香闺".into(),
            category: "恋爱".into(),
            intensity: "low".into(),
        });
        let raw = serde_json::to_string(&s).unwrap();
        let back: TavernSession = serde_json::from_str(&raw).unwrap();
        let ev = back.last_event.expect("last_event");
        assert_eq!(ev.card_id, "a2");
        assert_eq!(ev.turn, 2);
    }

    // ─── S6: Actor 归档 ────────────────────────────────────────────

    fn actor_system_fixture() -> ActorStateSystem {
        let mut sys = ActorStateSystem::default();
        sys.actors.insert(
            "cc-linwan".into(),
            ActorStateEntry {
                template_id: "tpl-1".into(),
                fields: [(
                    "气力".to_string(),
                    ActorFieldValue {
                        value_type: "number".into(),
                        value: Some(json!(72)),
                        min: None,
                        max: None,
                        options: vec![],
                        display: None,
                        update_instruction: None,
                        updated_by: String::new(),
                        updated_turn: 0,
                        updated_reason: String::new(),
                    },
                )]
                .into_iter()
                .collect(),
                traits: vec![],
            },
        );
        sys
    }

    #[test]
    fn archive_actor_creates_snapshot_with_reason() {
        let mut sys = actor_system_fixture();
        let snap = sys
            .archive_actor("cc-linwan", "manual", "2026-08-05T00:00:00Z")
            .expect("snap");
        assert_eq!(snap.reason, "manual");
        assert_eq!(snap.character_id, "cc-linwan");
        assert_eq!(sys.archive.len(), 1);
    }

    #[test]
    fn archive_actor_missing_character_returns_none() {
        let mut sys = actor_system_fixture();
        assert!(
            sys.archive_actor("cc-none", "manual", "2026-08-05T00:00:00Z")
                .is_none()
        );
        assert!(sys.archive.is_empty());
    }

    #[test]
    fn restore_actor_restores_latest_snapshot() {
        let mut sys = actor_system_fixture();
        sys.archive_actor("cc-linwan", "story", "2026-08-05T00:00:00Z");
        // 修改状态后恢复
        sys.actors
            .get_mut("cc-linwan")
            .unwrap()
            .fields
            .get_mut("气力")
            .unwrap()
            .value = Some(json!(30));
        assert!(sys.restore_actor("cc-linwan"));
        let restored = sys.actors.get("cc-linwan").unwrap();
        let v = restored
            .fields
            .get("气力")
            .and_then(|f| f.value.as_ref())
            .unwrap();
        assert_eq!(v, &json!(72));
    }

    #[test]
    fn restore_actor_without_snapshot_returns_false() {
        let mut sys = actor_system_fixture();
        assert!(!sys.restore_actor("cc-shentang"));
    }

    #[test]
    fn restore_actor_picks_most_recent_snapshot() {
        let mut sys = actor_system_fixture();
        sys.archive_actor("cc-linwan", "auto", "2026-08-05T00:00:00Z");
        sys.actors
            .get_mut("cc-linwan")
            .unwrap()
            .fields
            .get_mut("气力")
            .unwrap()
            .value = Some(json!(50));
        sys.archive_actor("cc-linwan", "manual", "2026-08-05T01:00:00Z");
        sys.actors
            .get_mut("cc-linwan")
            .unwrap()
            .fields
            .get_mut("气力")
            .unwrap()
            .value = Some(json!(10));
        assert!(sys.restore_actor("cc-linwan"));
        let v = sys
            .actors
            .get("cc-linwan")
            .unwrap()
            .fields
            .get("气力")
            .and_then(|f| f.value.as_ref())
            .unwrap();
        assert_eq!(v, &json!(50));
    }

    #[test]
    fn actor_archive_reason_missing_defaults_empty() {
        let raw = r#"{"characterId":"cc-linwan","createdAt":"2026-08-05T00:00:00Z","state":{}}"#;
        let snap: ActorArchiveSnapshot = serde_json::from_str(raw).unwrap();
        assert!(snap.reason.is_empty());
        let back = serde_json::to_string(&snap).unwrap();
        assert!(back.contains("\"reason\""));
    }

    // 兜底写盘：write_atomic 必须剔除 U+FFFD 替换符（上游 LLM 偶发把单字损坏成 2-3 个 FFFD）。
    #[test]
    fn write_atomic_strips_fffd() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kaleido-clean-test-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.json");
        write_atomic(&path, "手段\u{FFFD}\u{FFFD}诚意 对弗\u{FFFD}\u{FFFD}\u{FFFD}德").unwrap();
        let out = fs::read_to_string(&path).unwrap();
        assert!(!out.contains('\u{FFFD}'), "落盘后不应残留 U+FFFD: {:?}", out);
        assert!(out.contains("手段诚意"), "fffd 应被剔除: {:?}", out);
        fs::remove_dir_all(&dir).unwrap();
    }
}
