//! p009 tests · markers（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    // ─── scan_bold_refs `**Name**` 引用追踪单测（waidrin ①） ───────────────
    #[test]
    pub(crate) fn bold_refs_basic() {
        let r = scan_bold_refs("**沈棠**站在雨中，看着**林晚的**衣袖。");
        assert_eq!(r, vec!["沈棠".to_string(), "林晚".to_string()]);
        // 去重 + 短名过滤
        let r2 = scan_bold_refs("**沈棠**又见**沈棠**，**阿**跑了。");
        assert_eq!(r2, vec!["沈棠".to_string()]);
        // 无包裹不命中
        assert!(scan_bold_refs("沈棠站在雨中").is_empty());
    }

    // ─── lore_trigger_ok Hook 条件触发单测（loreweaver ⑤） ──────────────────
    #[test]
    pub(crate) fn lore_trigger_ok_item_present_bond() {
        use serde_json::json;
        // 构造最小 session：内联模板（不读 data 文件，公开库可跑）
        let mut sess_json: serde_json::Value = serde_json::json!({
            "sessionId": "t", "packId": "p", "playable": "P1", "playMode": "mainline",
            "contentTier": "standard", "turn": 1, "messages": [],
            "presentCharacterIds": [] });
        sess_json["pockets"] = serde_json::json!({"ayuan": {"carrying": [{"name": "银锁"}], "worn": [], "setAside": []}});
        sess_json["relationships"] = serde_json::json!({});
        let mut sess: kaleido_core::TavernSession = serde_json::from_value(sess_json).expect("sess");
        // triggerItem 命中
        assert!(lore_trigger_ok(&json!({"triggerItem":["银锁"]}), &sess));
        assert!(!lore_trigger_ok(&json!({"triggerItem":["金锁"]}), &sess));
        // triggerPresent：present 为空 → 跳过
        assert!(!lore_trigger_ok(&json!({"triggerPresent":["cc-shentang"]}), &sess));
        sess.present_character_ids.push("cc-shentang".into());
        assert!(lore_trigger_ok(&json!({"triggerPresent":["cc-shentang"]}), &sess));
        // triggerBondMin：0 分不达 50
        assert!(!lore_trigger_ok(&json!({"triggerBondMin":{"cc-linwan":50}}), &sess));
        // 无 trigger → 通过
        assert!(lore_trigger_ok(&json!({"title":"x"}), &sess));
    }

    // ─── parse_option_list JSON 污染防护单测（P11） ─────────────────────────
    #[test]
    pub(crate) fn parse_option_list_rejects_json_object_leak() {
        // P11 实踩：检定 JSON 对象裸写在正文 → 键值对被捞成 52 个「选项」
        let leak = "{\"action\":\"告诉母亲第一次还为她留着\",\"intent\":\"彻底坦白\",\"challenge\":\"母亲拒绝\",\"cost\":\"崩溃\",\"difficulty\":\"normal\",\"templateId\":\"亲密试探检定\",\"bonuses\":[{\"reason\":\"醉酒坦诚\",\"value\":1}],\"outcomes\":{\"criticalSuccess\":{\"result\":\"成功\"},\"success\":{\"result\":\"成功\",\"stateChanges\":[{\"actorId\":\"c-distil-0\",\"fieldId\":\"emotion\",\"change\":1}]}}}";
        let parsed = parse_option_list(leak);
        assert!(parsed.is_empty(), "JSON 泄漏不应产生选项: {parsed:?}");
    }

    #[test]
    pub(crate) fn parse_option_list_keeps_normal_choices() {
        // 正常选项（短中文串）不受影响
        let normal = "[\"靠在她膝盖上，什么都不说\", \"握住她的手\", \"把她拉进怀里\"]";
        let parsed = parse_option_list(normal);
        assert_eq!(parsed.len(), 3, "正常选项应保留: {parsed:?}");
    }

    #[test]
    pub(crate) fn parse_option_list_keeps_quoted_choices_without_json_hints() {
        // 无 JSON 键名的引号选项（旧分支行为）保留
        let quoted = "\"把头靠在她膝盖上\" \"握住她搭在头顶的那只手\" \"站起来坐在她身旁\"";
        let parsed = parse_option_list(quoted);
        assert_eq!(parsed.len(), 3, "引号选项应保留: {parsed:?}");
    }

    // ─── extract_advance_marker 节点推进 marker 兼容性单测（ST-15 fix） ─────
    #[test]
    pub(crate) fn extract_advance_marker_inner_colon_format() {
        // prompt 教的格式：【节点推进:n2】（冒号+节点ID在括号内）
        let text = "正文内容……她垂下眼。\n【节点推进:n2】";
        let (pos, id) = extract_advance_marker(text).expect("应提取到 n2");
        assert_eq!(id, "n2");
        assert!(text[..pos].contains("正文内容"), "marker 起始位置应指向正文之后");
    }

    #[test]
    pub(crate) fn extract_advance_marker_outer_format() {
        // 旧格式：【节点推进】 n2（节点ID在括号外）
        let text = "正文……\n【节点推进】 n3";
        let (_pos, id) = extract_advance_marker(text).expect("应提取到 n3");
        assert_eq!(id, "n3");
    }

    #[test]
    pub(crate) fn extract_advance_marker_absent_returns_none() {
        assert!(extract_advance_marker("纯正文，没有任何标记").is_none());
        assert!(extract_advance_marker("").is_none());
    }

    #[test]
    pub(crate) fn extract_advance_marker_takes_last_marker() {
        // 多个 marker 时取最后一个（rfind 语义）
        let text = "【节点推进:n1】 中间正文 【节点推进:n2】";
        let (_pos, id) = extract_advance_marker(text).expect("应取最后一个");
        assert_eq!(id, "n2");
    }
