//! p009 tests · protocol_split（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    #[test]
    pub(crate) fn test_split_program_from_narrative_extracts() {
        let text = "正文前\n【程序】<html><body><button onclick=\"alert(1)\">点我</button></body></html>\n【/程序】\n正文后";
        let (clean, program) = split_program_from_narrative(text);
        assert!(!clean.contains("【程序】"));
        assert!(!clean.contains("【/程序】"));
        assert!(clean.contains("正文前"));
        assert!(clean.contains("正文后"));
        let html = program.expect("program card should be extracted");
        assert!(html.starts_with("<html>"));
        assert!(html.ends_with("</html>"));
        assert!(html.contains("alert(1)"));
    }

    #[test]
    pub(crate) fn test_split_program_from_narrative_none() {
        let text = "普通文本";
        let (clean, program) = split_program_from_narrative(text);
        assert!(program.is_none());
        assert_eq!(clean, "普通文本");
    }

    #[test]
    pub(crate) fn test_split_state_updates_from_narrative_extracts() {
        let text = "旁白。\n【状态更新】{\"characterId\":\"char-1\",\"fields\":{\"hp\":95,\"hunger\":30},\"addTraits\":[{\"poolId\":\"p\",\"traitId\":\"t1\",\"name\":\"饱腹\",\"summary\":\"刚进食\"}]}\n继续。";
        let (clean, updates) = split_state_updates_from_narrative(text);
        assert_eq!(updates.len(), 1);
        let u = &updates[0];
        assert_eq!(u.character_id, "char-1");
        assert_eq!(u.fields.get("hp"), Some(&serde_json::json!(95)));
        assert_eq!(u.fields.get("hunger"), Some(&serde_json::json!(30)));
        assert_eq!(u.add_traits.len(), 1);
        assert!(!clean.contains("【状态更新】"));
        assert!(!clean.contains("characterId"));
        assert!(clean.contains("旁白。"));
        assert!(clean.contains("继续。"));
    }

    #[test]
    pub(crate) fn test_split_state_updates_from_narrative_keeps_invalid() {
        // [fix §9 2026-08-16] 解析失败也剥离（正文零残留优先）：带【状态更新】标记的
        // 畸形块不再保留原文——避免「英文字符串」污染正文；warn 日志记录原文。
        let text = "旁白。\n【状态更新】{not json}\n继续。";
        let (clean, updates) = split_state_updates_from_narrative(text);
        assert!(updates.is_empty());
        assert!(!clean.contains("【状态更新】"), "畸形块应剥离: {clean}");
        assert!(!clean.contains("{not json}"), "畸形块应剥离: {clean}");
        assert!(clean.contains("旁白。"));
        assert!(clean.contains("继续。"));
        // 空 character_id：剥离（无有效更新，正文零残留）
        let text2 = "旁白。\n【状态更新】{\"characterId\":\"\",\"fields\":{}}\n继续。";
        let (clean2, updates2) = split_state_updates_from_narrative(text2);
        assert!(updates2.is_empty());
        assert!(!clean2.contains("【状态更新】"), "空 character_id 块应剥离: {clean2}");
    }

    #[test]
    pub(crate) fn test_split_state_updates_from_narrative_none() {
        let text = "普通文本";
        let (clean, updates) = split_state_updates_from_narrative(text);
        assert!(updates.is_empty());
        assert_eq!(clean, "普通文本");
    }

    #[test]
    pub(crate) fn test_split_state_updates_bare_json_without_marker() {
        // 无【状态更新】标记的裸 JSON 状态块（模型偶发追加在正文末尾）必须被剥离
        let text = "正文内容\n\n{\"characterId\":\"c-distil-4\",\"fields\":{\"情绪\":\"心动\",\"体力\":75},\"addTraits\":[],\"removeTraits\":[]}";
        let (clean, updates) = split_state_updates_from_narrative(text);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].character_id, "c-distil-4");
        assert_eq!(updates[0].fields.get("体力"), Some(&serde_json::json!(75)));
        assert!(!clean.contains("characterId"));
        assert!(clean.contains("正文内容"));
    }

    #[test]
    pub(crate) fn test_split_state_updates_duplicate_keys_multi_char() {
        // LLM 偶发把多角色状态合并进单对象（重复 characterId/fields 键）→ 拆分并剥离整块
        let text = "旁白。\n【状态更新】{\"characterId\":\"c-distil-0\",\"fields\":{\"emotion\":\"心虚但坚定\",\"心情状态\":\"紧张又踏实\"},\"characterId\":\"c-distil-1\",\"fields\":{\"emotion\":\"平静\",\"心情状态\":\"平和\"}}\n继续。";
        let (clean, updates) = split_state_updates_from_narrative(text);
        assert_eq!(updates.len(), 2, "应拆出两个角色状态");
        assert_eq!(updates[0].character_id, "c-distil-0");
        assert_eq!(updates[0].fields.get("emotion"), Some(&serde_json::json!("心虚但坚定")));
        assert_eq!(updates[1].character_id, "c-distil-1");
        assert_eq!(updates[1].fields.get("心情状态"), Some(&serde_json::json!("平和")));
        assert!(!clean.contains("【状态更新】"));
        assert!(!clean.contains("characterId"));
        assert!(clean.contains("旁白。"));
        assert!(clean.contains("继续。"));
    }

    #[test]
    pub(crate) fn test_split_state_updates_duplicate_keys_bare() {
        // 无标记裸 JSON 的重复键变体同样要拆分剥离
        let text = "正文\n{\"characterId\":\"c-distil-0\",\"fields\":{\"emotion\":\"心虚\"},\"characterId\":\"c-distil-1\",\"fields\":{\"emotion\":\"平静\"}}";
        let (clean, updates) = split_state_updates_from_narrative(text);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].character_id, "c-distil-0");
        assert_eq!(updates[1].character_id, "c-distil-1");
        assert!(!clean.contains("characterId"));
        assert!(clean.contains("正文"));
    }

    #[test]
    pub(crate) fn test_split_state_updates_duplicate_keys_partial_still_keeps() {
        // [fix §9 2026-08-16] 拆分后子段含非法 JSON → 无法提取有效更新，但块仍剥离
        // （正文零残留优先；结构特征【状态更新】+JSON 极特定，不误删正文）。
        let text = "旁白。\n【状态更新】{\"characterId\":\"c-distil-0\",\"fields\":{\"emotion\":\"心虚\"},\"characterId\":\"c-distil-1\",\"fields\":{bad json}}\n继续。";
        let (clean, updates) = split_state_updates_from_narrative(text);
        assert!(updates.is_empty());
        assert!(!clean.contains("【状态更新】"), "畸形块应剥离: {clean}");
        assert!(!clean.contains("bad json"), "畸形块应剥离: {clean}");
        assert!(clean.contains("旁白。"));
        assert!(clean.contains("继续。"));
    }

    #[test]
    pub(crate) fn test_split_state_updates_bare_json_invalid_kept() {
        // 裸 JSON 无法解析为状态块 → 保留原文不误删
        let text = r#"正文里有一个 {"普通":"对象"} 不是状态块"#;
        let (clean, updates) = split_state_updates_from_narrative(text);
        assert!(updates.is_empty());
        assert!(clean.contains("普通"));
    }

    #[test]
    pub(crate) fn test_split_check_from_narrative_extracts() {
        let text = "旁白。\n【检定】{\"action\":\"潜行\",\"intent\":\"溜进仓库\",\"challenge\":\"守卫巡逻\",\"cost\":\"被发现\",\"difficulty\":\"normal\",\"bonuses\":[{\"reason\":\"地形掩护\",\"value\":2}],\"outcomes\":{\"success\":{\"result\":\"你成功溜了进去\"}}}\n继续。";
        let (clean, checks) = split_check_from_narrative(text);
        assert_eq!(checks.len(), 1);
        let c = &checks[0];
        assert_eq!(c.action, "潜行");
        assert_eq!(c.difficulty, "normal");
        assert_eq!(c.bonuses.len(), 1);
        assert_eq!(c.bonuses[0].value, 2.0);
        assert_eq!(c.outcomes.success.result, "你成功溜了进去");
        assert!(!clean.contains("【检定】"));
        assert!(!clean.contains("outcomes"));
        assert!(clean.contains("旁白。"));
        assert!(clean.contains("继续。"));
    }

    #[test]
    pub(crate) fn test_split_check_from_narrative_keeps_invalid() {
        // 非法 JSON：跳过该块但保留原文
        let text = "旁白。\n【检定】{not json}\n继续。";
        let (clean, checks) = split_check_from_narrative(text);
        assert!(checks.is_empty());
        assert!(clean.contains("【检定】"));
        assert!(clean.contains("{not json}"));
        // 空 action：同样跳过但保留原文
        let text2 = "旁白。\n【检定】{\"action\":\"\",\"intent\":\"x\",\"challenge\":\"y\"}\n继续。";
        let (clean2, checks2) = split_check_from_narrative(text2);
        assert!(checks2.is_empty());
        assert!(clean2.contains("【检定】"));
    }

    #[test]
    pub(crate) fn test_split_check_from_narrative_none() {
        let text = "普通文本";
        let (clean, checks) = split_check_from_narrative(text);
        assert!(checks.is_empty());
        assert_eq!(clean, "普通文本");
    }

    #[test]
    pub(crate) fn test_split_director_plan_from_narrative_extracts() {
        let text = "旁白。\n【导演计划】{\"goal\":\"引出铜铃来信\",\"pressure\":\"门外脚步已停\",\"cost\":\"若拆信则再无回头路\",\"hits_beats\":[\"信的存在不可抹除\"]}\n继续。";
        let (clean, update) = split_director_plan_from_narrative(text);
        match update {
            Some(DirectorPlanUpdate::Set(out)) => {
                assert_eq!(out.goal, "引出铜铃来信");
                assert_eq!(out.pressure.as_deref(), Some("门外脚步已停"));
                assert_eq!(out.cost.as_deref(), Some("若拆信则再无回头路"));
                assert_eq!(out.hits_beats, vec!["信的存在不可抹除".to_string()]);
            }
            other => panic!("expected Set, got {other:?}"),
        }
        assert!(!clean.contains("【导演计划】"));
        assert!(!clean.contains("goal"));
        assert!(clean.contains("旁白。"));
        assert!(clean.contains("继续。"));
    }

    #[test]
    pub(crate) fn test_split_director_plan_from_narrative_none_marker() {
        let text = "旁白。\n【导演计划】none\n继续。";
        let (clean, update) = split_director_plan_from_narrative(text);
        assert!(matches!(update, Some(DirectorPlanUpdate::Skip)));
        assert!(!clean.contains("【导演计划】"));
        assert!(!clean.contains("none"));
        assert!(clean.contains("旁白。"));
        assert!(clean.contains("继续。"));
    }

    #[test]
    pub(crate) fn test_split_director_plan_from_narrative_keeps_invalid() {
        // 非法 JSON：保留原文不剥离
        let text = "旁白。\n【导演计划】{not json}\n继续。";
        let (clean, update) = split_director_plan_from_narrative(text);
        assert!(update.is_none());
        assert!(clean.contains("【导演计划】"));
        assert!(clean.contains("{not json}"));
        // goal 为空：不算有效计划，保留原文
        let text2 = "旁白。\n【导演计划】{\"goal\":\"\",\"pressure\":\"x\"}\n继续。";
        let (clean2, update2) = split_director_plan_from_narrative(text2);
        assert!(update2.is_none());
        assert!(clean2.contains("【导演计划】"));
    }

    #[test]
    pub(crate) fn test_split_director_plan_from_narrative_none() {
        let text = "普通文本";
        let (clean, update) = split_director_plan_from_narrative(text);
        assert!(update.is_none());
        assert_eq!(clean, "普通文本");
    }
