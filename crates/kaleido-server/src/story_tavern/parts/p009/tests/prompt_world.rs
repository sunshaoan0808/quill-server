//! p009 tests · prompt_world（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    #[test]
    pub(crate) fn test_build_tavern_system_prompt_skill_load_injected() {
        let pack = test_pack_minimal();
        let tools: Vec<crate::tavern_mcp::McpToolEntry> = vec![];
        let tmp = std::env::temp_dir();
        let base =
            build_tavern_system_prompt(&pack, &test_session_minimal(), "", &tmp, None, &tools, None);
        assert!(
            !base.contains("你请求加载的完整写作 Skill"),
            "未请求时不得注入 skill_load：{base}"
        );
        let mut session = test_session_minimal();
        session.skill_load = Some(kaleido_core::SkillLoadInfo {
            tier: "heavy".into(),
            markdown: "## 完整写作 Skill（heavy）\n特征行 ST-SKILL-FEATURE\n## 模板\n### plan\n大纲".into(),
        });
        let out = build_tavern_system_prompt(&pack, &session, "", &tmp, None, &tools, None);
        assert!(
            out.contains("你请求加载的完整写作 Skill"),
            "回填标记应注入：{out}"
        );
        assert!(out.contains("ST-SKILL-FEATURE"), "SKILL.md 特征串注入");
        assert!(out.contains("## 模板"), "模板段随全文注入");
    }

    // T 层 (2026-08-19): 原著时间线注入——按当前章过滤，首回合只见已推进事件，后期事件零泄漏。
    #[test]
    pub(crate) fn worldline_block_filters_by_current_chapter() {
        let mut pack = test_pack_minimal();
        pack.worldline = serde_json::from_str::<Vec<serde_json::Value>>(r#"[
            {"chapter":"ch01","event":"主角放学接到父亲电话","time_point":"放学后","importance":"low"},
            {"chapter":"ch02","event":"与母亲抵达度假村","time_point":"抵达","importance":"high"},
            {"chapter":"ch06","event":"蜜月第一晚相处","time_point":"夜晚","importance":"medium"},
            {"chapter":"ch10","event":"相亲对象登场","time_point":"周末","importance":"high"}
        ]"#).unwrap();

        // 首回合（ch01）：只注入 ch01，ch02/ch06/ch10 全被过滤
        let b1 = build_worldline_block(&pack, "ch01").expect("ch01 应有时间线");
        assert!(b1.contains("放学接到父亲电话"), "ch01 事件应注入: {b1}");
        assert!(!b1.contains("抵达度假村"), "ch02 不得在首回合泄漏");
        assert!(!b1.contains("蜜月第一晚"), "ch06 不得在首回合泄漏");
        assert!(!b1.contains("相亲对象"), "ch10 不得在首回合泄漏");

        // ch02：注入 ch01+ch02，仍不含 ch06/ch10
        let b2 = build_worldline_block(&pack, "ch02").expect("ch02 应有时间线");
        assert!(b2.contains("抵达度假村"), "ch02 事件应注入");
        assert!(!b2.contains("蜜月第一晚"), "ch06 不得泄漏");
        assert!(!b2.contains("相亲对象"), "ch10 不得泄漏");

        // 无章节标注的事件（解析失败）保守不注入
        let mut bad = test_pack_minimal();
        bad.worldline = serde_json::from_str::<Vec<serde_json::Value>>(r#"[{"event":"无章号事件"}]"#).unwrap();
        assert!(build_worldline_block(&bad, "ch01").is_none(), "无章号事件不注入: {bad:?}");

        // 空 worldline（旧 pack）→ None 零注入
        assert!(build_worldline_block(&test_pack_minimal(), "ch01").is_none());
    }
