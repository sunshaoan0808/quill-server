//! p009 tests · guards（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    #[test]
    pub(crate) fn guard_person_unknown_high() {
        // 2026-08-16 统一来源根治: 切词启发式退役——无 roster 时「王麻子说道」不再靠切词瞎判。
        // 真外对象只信统一来源（roster/场景标签/LLM 兜底），宁漏勿瞎。
        let known: std::collections::HashSet<String> =
            ["林晚".into()].into_iter().collect();
        let vs = guard_narrative(
            "林晚说道：师兄且慢。王麻子说道：这玉佩是我的！",
            "",
            &known,
            &[],
            &[],
            &[],
            None,
        );
        assert!(
            !vs.iter().any(|v| v.dim == "人物"),
            "无 roster 时切词退役，不靠启发式瞎判外对象: {vs:?}"
        );
        // 同一文本若 LLM 自报清单（统一来源）含王麻子 → 精确比对检出
        let vs2 = guard_narrative(
            "林晚说道：师兄且慢。王麻子说道：这玉佩是我的！",
            "",
            &known,
            &[],
            &[],
            &[],
            Some(&["王麻子".to_string()]),
        );
        let hit = vs2.iter().find(|v| v.dim == "人物" && v.msg.contains("王麻子"));
        assert!(hit.is_some(), "roster 统一来源应检出王麻子: {vs2:?}");
        assert_eq!(hit.unwrap().severity, GuardSeverity::High);
    }

    #[test]
    pub(crate) fn guard_no_fragment_fp_on_common_phrases() {
        // 2026-08-16 瞎报根治回归: 「道」作名词尾（胡说八道/谁知道/城市主干道/坡道）
        // 不再是说话标记；4字碎片（东电话里/段石阶坡）与称呼（老板娘）不误报。
        let known: std::collections::HashSet<String> =
            ["林晚".into(), "庄眉".into()].into_iter().collect();
        let text = "林晚说：那些胡说八道的东西你也信？庄眉在电话里说：谁知道呢，\
            反正城市主干道上的霓虹还亮着。老板娘说那边有条新街刚招商，租金便宜。\
            那段石阶坡道又陡又窄，走起来费劲。";
        let vs = guard_narrative(text, "", &known, &[], &[], &[], None);
        assert!(
            !vs.iter().any(|v| v.dim == "人物"),
            "常见短语碎片不应误报外对象: {vs:?}"
        );
    }

    #[test]
    pub(crate) fn guard_still_catches_real_unknown_after_fp_fix() {
        // 统一来源根治: 真外对象由 LLM 自报清单（roster）检出，不依赖切词。
        let known: std::collections::HashSet<String> =
            ["林晚".into()].into_iter().collect();
        let vs = guard_narrative(
            "林晚说：师兄且慢。李铁柱说道：这玉佩是我的！",
            "",
            &known,
            &[],
            &[],
            &[],
            Some(&["李铁柱".to_string()]),
        );
        let hit = vs.iter().find(|v| v.dim == "人物");
        assert!(hit.is_some(), "roster 统一来源应检出真外对象: {vs:?}");
        assert!(hit.unwrap().msg.contains("李铁柱"), "{vs:?}");
    }

    #[test]
    pub(crate) fn guard_roster_narrative_unknown_high() {
        // ST-30 根治: 叙述/动作形态点名（无说话标记，切词启发式漏检）——
        // LLM 自报角色清单含外角色 → 精确比对检出（替代切词，根治漏报）。
        let known: std::collections::HashSet<String> =
            ["沈棠".into(), "林晚".into()].into_iter().collect();
        let roster = vec!["沈棠".to_string(), "李铁柱".to_string()];
        let vs = guard_narrative(
            "李铁柱的声音比雨声重，压过檐下滴水。沈棠没有立刻答话。",
            "",
            &known,
            &[],
            &[],
            &[],
            Some(&roster),
        );
        let hit = vs.iter().find(|v| v.dim == "人物" && v.msg.contains("李铁柱"));
        assert!(hit.is_some(), "清单外角色应检出: {vs:?}");
        assert_eq!(hit.unwrap().severity, GuardSeverity::High);
        // 全 known 清单 → 人物维零违规（无切词误报）
        let roster2 = vec!["沈棠".to_string(), "林晚".to_string()];
        let vs2 = guard_narrative(
            "沈棠的手指在门框上一僵。林晚在柜台后面搁下瓷杯。",
            "",
            &known,
            &[],
            &[],
            &[],
            Some(&roster2),
        );
        assert!(
            !vs2.iter().any(|v| v.dim == "人物"),
            "known 清单不应报人物违规: {vs2:?}"
        );
    }

    #[test]
    pub(crate) fn guard_roster_skips_heuristic_no_fragment_fp() {
        // ST-30 根治: 清单存在时不再跑切词启发式 → 「明日柜上/却带着点」类
        // 切碎短语误报从根上消失（不再可能被切出来）。
        let known: std::collections::HashSet<String> =
            ["沈棠".into(), "林晚".into()].into_iter().collect();
        let roster = vec!["沈棠".to_string()];
        let vs = guard_narrative(
            "明日柜上，却带着点雨气的灯笼被风推着转了小半圈，沈棠站在檐下。",
            "",
            &known,
            &[],
            &[],
            &[],
            Some(&roster),
        );
        assert!(
            !vs.iter().any(|v| v.dim == "人物"),
            "切词启发式误报应在清单模式下消失: {vs:?}"
        );
    }

    #[test]
    pub(crate) fn guard_roster_missing_falls_back_heuristic() {
        // ST-30 兜底: 清单缺失（LLM 未遵守格式）→ 降级现有启发式（场景标签检测仍工作）。
        let known: std::collections::HashSet<String> =
            ["沈棠".into(), "林晚".into()].into_iter().collect();
        let vs = guard_narrative(
            "<场景：茶馆｜雨夜｜沈棠、孙二娘>茶香袅袅。",
            "",
            &known,
            &[],
            &[],
            &[],
            None,
        );
        let hit = vs.iter().find(|v| v.dim == "人物" && v.msg.contains("孙二娘"));
        assert!(hit.is_some(), "无清单时场景标签启发式应兜底检出: {vs:?}");
    }

    #[test]
    pub(crate) fn guard_beat_missing_high() {
        let known: std::collections::HashSet<String> =
            ["林晚".into()].into_iter().collect();
        let vs = guard_narrative(
            "林晚接过药瓶，沉默不语。",
            "",
            &known,
            &[],
            &["必须向师父坦白".into()],
            &[],
        None,
        );
        assert_eq!(vs.len(), 1);
        assert_eq!(vs[0].severity, GuardSeverity::High);
        assert_eq!(vs[0].dim, "节拍");
    }

    #[test]
    pub(crate) fn guard_present_missing_medium() {
        let known: std::collections::HashSet<String> =
            ["林晚".into(), "陈默".into()].into_iter().collect();
        let vs = guard_narrative(
            "林晚站在山门口，望着远方。",
            "",
            &known,
            &["陈默".to_string()],
            &[],
            &[],
        None,
        );
        assert!(vs.iter().any(|v| v.severity == GuardSeverity::Medium && v.dim == "出场"));
    }

    #[test]
    pub(crate) fn guard_goal_missing_medium() {
        let known: std::collections::HashSet<String> =
            ["林晚".into()].into_iter().collect();
        let vs = guard_narrative(
            "林晚沿着山路走着。",
            "",
            &known,
            &[],
            &[],
            &["拜师学艺".into()],
        None,
        );
        assert!(vs.iter().any(|v| v.severity == GuardSeverity::Medium && v.dim == "大纲"));
    }

    #[test]
    pub(crate) fn guard_scene_tag_unknown_high() {
        // 2026-08-14: 场景标签里的原著外角色（王麻子案例——引语式检测抓不到）
        let known: std::collections::HashSet<String> =
            ["林晚".into(), "沈棠".into()].into_iter().collect();
        let vs = guard_narrative(
            "<场景：旧茶馆内·窗边｜雨夜｜沈棠、林晚、王麻子>\n\n门被推开，一个满脸横肉的男人闯了进来。",
            "",
            &known,
            &["林晚".into()],
            &[],
            &[],
        None,
        );
        let scene_hit = vs.iter().find(|v| v.msg.contains("王麻子"));
        assert!(scene_hit.is_some(), "场景标签外角色应被检出: {vs:?}");
        assert_eq!(scene_hit.unwrap().severity, GuardSeverity::High);
    }

    #[test]
    pub(crate) fn guard_scene_tag_adjective_no_false_positive() {
        // 2026-08-14 误报修复: 「温热的」「白的蓝」等形容词片段 + known 角色延伸 不应误报
        let known: std::collections::HashSet<String> =
            ["沈棠".into(), "林晚".into()].into_iter().collect();
        let vs = guard_narrative(
            "<场景：茶馆｜雨夜｜沈棠、温热的、白的蓝、林晚的伞>茶香袅袅，白瓷杯冒着热气。",
            "",
            &known,
            &[],
            &[],
            &[],
        None,
        );
        assert!(
            !vs.iter().any(|v| v.dim == "人物"),
            "形容词/延伸片段不应误报为角色: {vs:?}"
        );
        // 真外角色（孙二娘）→ 仍应检出
        let vs2 = guard_narrative(
            "<场景：茶馆｜雨夜｜沈棠、孙二娘>茶香袅袅。",
            "",
            &known,
            &[],
            &[],
            &[],
        None,
        );
        assert!(
            vs2.iter().any(|v| v.dim == "人物" && v.msg.contains("孙二娘")),
            "场景标签外角色仍应检出: {vs2:?}"
        );
    }

    #[test]
    pub(crate) fn guard_idiom_fragment_no_false_positive() {
        // 2026-08-14: 「她亲恼羞成怒」被切出「亲恼羞成」当角色名（实测误报）——
        // 情绪字（笑/哭/怒/叹）不再作触发标记，仅保留去尾角色（林晚笑道 → 林晚）
        let known: std::collections::HashSet<String> =
            ["沈棠".into(), "林晚".into()].into_iter().collect();
        let vs = guard_narrative(
            "林晚亲恼羞成怒，把茶杯重重搁下。沈棠说道：师姐消消气。",
            "",
            &known,
            &[],
            &[],
            &[],
        None,
        );
        assert!(
            !vs.iter().any(|v| v.dim == "人物"),
            "成语/情绪片段不应误报为角色: {vs:?}"
        );
        // 「林晚笑道」→ 情绪字去尾后命中 known → 不报
        let vs3 = guard_narrative(
            "林晚笑道：师妹多虑了。",
            "",
            &known,
            &[],
            &[],
            &[],
        None,
        );
        assert!(
            !vs3.iter().any(|v| v.dim == "人物"),
            "known 角色+情绪字不应误报: {vs3:?}"
        );
        // 真外角色由 LLM 自报清单（统一来源）检出
        let vs2 = guard_narrative(
            "王麻子说道：这玉佩是我的！",
            "",
            &known,
            &[],
            &[],
            &[],
            Some(&["王麻子".to_string()]),
        );
        assert!(
            vs2.iter().any(|v| v.dim == "人物" && v.msg.contains("王麻子")),
            "roster 统一来源应检出王麻子: {vs2:?}"
        );
    }

    #[test]
    pub(crate) fn guard_clean_pass() {
        let known: std::collections::HashSet<String> =
            ["林晚".into()].into_iter().collect();
        let vs = guard_narrative(
            "林晚说道：这玉佩我收下了。",
            "",
            &known,
            &["林晚".to_string()],
            &["收下玉佩".into()],
            &["收下玉佩".into()],
        None,
        );
        assert!(vs.is_empty(), "expected clean pass, got {:?}", vs);
    }
