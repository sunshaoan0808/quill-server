//! p009 tests · domain_edges（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    // ---- S7 改进单测 (2026-08-18) ----

    #[test]
    pub(crate) fn s7_attach_plot_scope_with_beats() {
        let q = "玩家最近消息".to_string();
        let beats: Vec<String> = vec!["答应陪妈去三亚".into(), "约定下周二出发".into()];
        let out = s7_attach_plot_scope(q.clone(), Some(("第一章 蜜月的起始", &beats)));
        assert!(out.contains("第一章 蜜月的起始"));
        assert!(out.contains("【剧情要点】"));
        assert!(out.contains("答应陪妈去三亚"));
        assert!(out.starts_with(&q)); // 原 query 在前，叠加在后
    }

    #[test]
    pub(crate) fn s7_attach_plot_scope_no_beats() {
        let q = "仅最近消息".to_string();
        let out = s7_attach_plot_scope(q, Some(("节点标题", &[])));
        // 无 beats → 只加标题
        assert!(out.contains("节点标题"));
        assert!(!out.contains("【剧情要点】"));
    }

    #[test]
    pub(crate) fn s7_attach_plot_scope_none_node() {
        let q = "无节点".to_string();
        let out = s7_attach_plot_scope(q.clone(), None);
        assert_eq!(out, q); // 叠加为空，原样返回
    }

    #[test]
    pub(crate) fn s7_recall_title_key_signal_hard() {
        // 命中文本含"承诺/约定" → 硬约束语气
        let h = s7_recall_title("你答应过陪她一起去。约定不变。");
        assert!(h.contains("关键约定/承诺"), "got: {h}");
        assert!(h.contains("必须延续"));
    }

    #[test]
    pub(crate) fn s7_recall_title_soft_when_normal() {
        // 平凡历史回忆 → 软参考（按需沿用）
        let h = s7_recall_title("那天傍晚在公园散步，风很舒服。");
        assert!(h.contains("按需沿用"), "got: {h}");
        assert!(!h.contains("必须延续"));
    }

    // ---- §14.6 角色注入三层 纯函数单测 (2026-08-18) ----

    pub(crate) fn char_ref(id: &str, name: &str) -> kaleido_core::PackCharacterRef {
        kaleido_core::PackCharacterRef {
            id: id.into(),
            name: name.into(),
            role: String::new(),
            gender: String::new(),
            appearance: String::new(),
            opening_scene: String::new(),
            opening_lines: String::new(),
            nsfw_profile: "non".into(),
            importance: "high".into(),
            content_tier: None,
            example_dialogs: vec![],
            boundaries: vec![],
            personality: format!("{name}的性格描述，这一整段性格描述文字被刻意写得非常长，目的就是要稳稳超过六十个字符那么多，从而验证概要行的截断逻辑确实生效并截到六十字边界附近"),
            speech_style: "短句".into(),
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

    #[test]
    pub(crate) fn character_summary_line_marks_absent_and_truncates() {
        let c = char_ref("c1", "林婉清");
        let s = character_summary_line(&c);
        assert!(s.contains("林婉清"));
        assert!(s.contains("未在场"), "got: {s}");
        // personality 截断 60 字（不喂全量）
        let pers_60 = c.personality.chars().take(60).collect::<String>();
        assert!(s.contains(&pers_60), "应含截断后的 personality: {s}");
        assert!(!s.contains(&c.personality), "不应含完整 personality（需截断）");
    }

    #[test]
    pub(crate) fn character_compact_card_bounded() {
        let c = char_ref("c2", "蒋闵柔");
        let out = character_compact_card(&c);
        assert!(out.contains("蒋闵柔"));
        assert!(out.chars().count() <= 300, "压缩卡应 ≤300 字, got {}", out.chars().count());
        assert!(out.chars().count() > 0);
    }

    // ---- §13.4① 玩家动作强制包装 (2026-08-18) ----

    #[test]
    pub(crate) fn wrap_player_action_short_action_triggered() {
        // 短动作指令 → 强制包装
        let w = wrap_player_action("与在场的人打招呼");
        assert!(w.is_some());
        let w = w.unwrap();
        assert!(w.contains("【玩家动作指令】"), "got: {w}");
        assert!(w.contains("必须让该动作真实发生"), "got: {w}");
    }

    #[test]
    pub(crate) fn wrap_player_action_browse_triggered() {
        let w = wrap_player_action("环顾四周，感受这个世界");
        assert!(w.is_some(), "环顾应触发");
    }

    #[test]
    pub(crate) fn wrap_player_action_long_narrative_untouched() {
        // 长叙述不包装（避免过度干预自由输入）
        let long = "我走进厨房，看见妈在炖排骨，蒸汽把她的刘海濡湿了，我站在门口想了一会儿该怎么开口提三亚的事";
        assert!(wrap_player_action(long).is_none(), "长叙述不应包装");
    }

    #[test]
    pub(crate) fn wrap_player_action_no_signal_word_untouched() {
        assert!(wrap_player_action("顺着刚才的节奏继续").is_none(), "无动作信号词不包装");
    }

    // ---- §13.5 Scene Gate 场景错位检测 (2026-08-18) ----

    #[test]
    pub(crate) fn scene_mismatch_hotel_detected() {
        // 首回合写成酒店/三亚 → 错位（Scene Gate 应触发纠偏）
        let t = "<场景：三亚·月见屋蜜月套房｜清晨｜小雨>\n我睁开眼……";
        assert!(is_scene_mismatch_location(t), "酒店应判错位");
    }

    #[test]
    pub(crate) fn scene_mismatch_airport_detected() {
        let t = "<场景：三亚凤凰国际机场｜清晨>\n飞机落地……";
        assert!(is_scene_mismatch_location(t), "机场应判错位");
    }

    #[test]
    pub(crate) fn scene_ok_school_allowed() {
        // 学校/家里 → 放行
        let t = "<场景：学校走廊｜午后>\n学生涌出教室……";
        assert!(!is_scene_mismatch_location(t), "学校不应判错位");
    }

    #[test]
    pub(crate) fn scene_ok_no_tag_allowed() {
        // 无 <场景 标签 → 不误拦
        assert!(!is_scene_mismatch_location("午后的阳光穿过教学楼旁那排老樟树……"));
    }

    // ---- §14.7.2B affinity_for_edge (2026-08-18) ----

    #[test]
    pub(crate) fn affinity_for_edge_exact_pair() {
        let aff = serde_json::json!({"c1:c2": 85});
        assert_eq!(affinity_for_edge(&aff, "c1", "c2"), Some(85));
    }

    #[test]
    pub(crate) fn affinity_for_edge_reversed_key() {
        // affinity 存反（to:from）也能命中
        let aff = serde_json::json!({"c2:c1": 60});
        assert_eq!(affinity_for_edge(&aff, "c1", "c2"), Some(60));
    }

    #[test]
    pub(crate) fn affinity_for_edge_player_single() {
        // 玩家(me)→角色 c2 好感存单键 c2（无 target 指向玩家自身反查）
        let aff = serde_json::json!({"c2": 92, "c3": 30});
        assert_eq!(affinity_for_edge(&aff, "me", "c2"), Some(92));
        // 反向键优先于单键（存在明确 c2:c1 时用它）
        let aff2 = serde_json::json!({"c2:c1": 60, "c1": 92});
        assert_eq!(affinity_for_edge(&aff2, "c1", "c2"), Some(60));
    }

    #[test]
    pub(crate) fn affinity_for_edge_miss() {
        let aff = serde_json::json!({"c1": 10});
        assert_eq!(affinity_for_edge(&aff, "c3", "c4"), None);
    }
