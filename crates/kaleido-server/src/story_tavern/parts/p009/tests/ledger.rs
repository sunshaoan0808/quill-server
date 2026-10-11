//! p009 tests · ledger（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    #[test]
    pub(crate) fn ledger_extract_entries_heuristics() {
        // 场景标签 + 角色状态 + 伏笔 + 物品 + 好感 全覆盖
        let body = "<场景：海边小屋｜午后>\n陆川轻轻把肖静搂进怀里，目光温柔。\n「旧玉坠」从她衣领里滑出来，他指尖摩挲着，轻声问：\n\"你还没告诉我，这枚玉坠的来历。\"\n肖静抚着隆起的小腹，眼底有些不安。";
        let chars = ["陆川", "肖静"];
        let entries = ledger_extract_entries(body, &chars);

        let kinds: Vec<LedgerKind> = entries.iter().map(|(k, _, _)| k.clone()).collect();
        assert!(kinds.contains(&LedgerKind::Time), "应提取时间，实际 {kinds:?}");
        assert!(kinds.contains(&LedgerKind::Status), "应提取状态，实际 {kinds:?}");
        assert!(kinds.contains(&LedgerKind::Foreshadow), "应提取伏笔，实际 {kinds:?}");
        assert!(kinds.contains(&LedgerKind::Item), "应提取物品，实际 {kinds:?}");
        assert!(kinds.contains(&LedgerKind::Affinity), "应提取好感，实际 {kinds:?}");

        // 物品：旧玉坠（对话「你还没告诉我」排除——含"告诉"但无虚词，实际提取的应为「旧玉坠」）
        let items: Vec<&str> = entries
            .iter()
            .filter(|(k, _, _)| matches!(k, LedgerKind::Item))
            .map(|(_, key, _)| key.as_str())
            .collect();
        assert!(items.iter().any(|s| s.contains("旧玉坠")), "物品应含旧玉坠，实际 {items:?}");
        // 对话性「」不应入账本
        assert!(!items.iter().any(|s| s.contains("你还没")), "对话不应入账本，实际 {items:?}");

        // 状态 key 应为角色名（肖静）
        let statuses: Vec<&str> = entries
            .iter()
            .filter(|(k, _, _)| matches!(k, LedgerKind::Status))
            .map(|(_, key, _)| key.as_str())
            .collect();
        assert!(statuses.iter().any(|s| *s == "肖静"), "状态应绑定肖静，实际 {statuses:?}");
    }

    #[test]
    pub(crate) fn ledger_extract_entries_limits() {
        // 限流：每类最多 N 条
        let body = "「玉佩」「香囊」「荷包」「扇子」「书卷」……肖静怀孕了，陆川发烧了，两人都很不安，心里藏着秘密，还有谜。";
        let chars = ["陆川", "肖静"];
        let entries = ledger_extract_entries(body, &chars);

        let items: usize = entries
            .iter()
            .filter(|(k, _, _)| matches!(k, LedgerKind::Item))
            .count();
        assert!(items <= 3, "物品最多 3 条，实际 {items}");

        let statuses: usize = entries
            .iter()
            .filter(|(k, _, _)| matches!(k, LedgerKind::Status))
            .count();
        assert!(statuses <= 2, "状态最多 2 条，实际 {statuses}");

        let hooks: usize = entries
            .iter()
            .filter(|(k, _, _)| matches!(k, LedgerKind::Foreshadow))
            .count();
        assert!(hooks <= 2, "伏笔最多 2 条，实际 {hooks}");
    }

    #[test]
    pub(crate) fn ledger_extract_entries_empty_no_panic() {
        let entries = ledger_extract_entries("", &["陆川", "肖静"]);
        assert!(entries.is_empty(), "空正文不应产生条目，实际 {entries:?}");
    }
