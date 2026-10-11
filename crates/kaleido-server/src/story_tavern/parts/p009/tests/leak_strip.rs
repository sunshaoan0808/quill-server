//! p009 tests · leak_strip（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    // ─── strip_director_preamble 导演自白剥离 单测 ─────────────────────────

    #[test]
    pub(crate) fn strip_preamble_removes_director_confession_before_scene_tag() {
        // R4 泄漏模式：「好的，我是林逸。…」直到 <场景 标签前
        let mut body = "好的，我是林逸。我看着妈妈，她还在撒谎。我决定跟她摊牌，让她知道我能扛。<场景：林逸家客厅｜傍晚｜林逸、陆清韵>\n\n陆清韵捧着水杯的手猛地一颤。".to_string();
        let stripped = strip_director_preamble(&mut body);
        assert!(stripped.contains("好的，我是林逸"));
        assert!(stripped.contains("摊牌"));
        assert!(!stripped.contains("<场景"));
        assert!(body.starts_with("<场景：林逸家客厅"));
        assert!(body.contains("陆清韵捧着水杯"));
    }

    #[test]
    pub(crate) fn strip_preamble_keeps_normal_narrative_untouched() {
        // 正常正文不以「好的，我是」开头 → 原样保留
        let mut body = "<场景：林逸家客厅｜傍晚｜林逸、陆清韵>\n\n陆清韵闻声回头，眼角还带着泪痕。".to_string();
        let stripped = strip_director_preamble(&mut body);
        assert!(stripped.is_empty());
        assert!(body.starts_with("<场景"));
    }

    #[test]
    pub(crate) fn strip_preamble_no_scene_tag_keeps_whole_body() {
        // R7 泄漏模式（无 <场景 边界）→ 保守不剥，宁可保留
        let mut body = "好的，我是林逸。敲门声又响了，我从平板监控里看到了那个光头男。他是我雇的人，来执行我的计划。妈听到敲门声肯定会紧张。".to_string();
        let stripped = strip_director_preamble(&mut body);
        assert!(stripped.is_empty());
        assert!(body.starts_with("好的，我是林逸"));
    }

    // ─── strip_fix_thinking_blocks 结构剥离（思维自动折叠）单测 ─────────────
    #[test]
    pub(crate) fn strip_trailing_metadiscourse_removes_self_check_tail() {
        // P2 实踩模式：正文后接「让我检查…」自检段
        let mut body = "她把那团布料叠好放在茶几上。\n\n让我检查一下“胸口起伏”是否与上文重复……\n\n再检查是否需要【状态更新】：不需要。\n\n好，让我输出最终版本。".to_string();
        let _ = strip_fix_thinking_blocks(&mut body);
        assert!(body.contains("布料叠好"), "正文应保留: {body}");
        assert!(!body.contains("让我检查"), "自检段应被剥离: {body}");
        assert!(!body.contains("让我输出"), "收尾段应被剥离: {body}");
    }

    #[test]
    pub(crate) fn strip_trailing_metadiscourse_multi_dash_takes_final_version() {
        // P2 多段 --- 拼接：取最后一段正文（若末尾是元话语则继续前溯）
        let mut body = "草稿一。\n\n---\n\n让我重新组织：检查问题1…\n\n---\n\n最终正文，干净收尾。".to_string();
        let _ = strip_fix_thinking_blocks(&mut body);
        assert!(body.contains("最终正文"), "应保留最终版: {body}");
        assert!(!body.contains("让我重新组织"), "中间元话语段应剥离: {body}");
    }

    #[test]
    pub(crate) fn strip_trailing_metadiscourse_strips_flashfree_self_check_tail() {
        // [fix 2026-08-16] 真实样本（msg35）：deepseek-v4-flash-free 在正文+【角色清单】后
        // 输出「嗯，这个版本不错。让我检查一下…✓ 检查清单 + 状态更新思考」，推理混入正文。
        let raw = "<场景：宿舍｜深夜>\n\n向明初把手机扣在胸口，闭上眼睛。那个寒假，值得等。\n\n【角色清单】向明初、山楂\n\n嗯，这个版本不错。让我检查一下：\n1. 时间：深夜，初冬 ✓\n2. 角色：向明初、山楂 ✓\n\n状态更新：向明初的心情从“紧张”变为“期待”。\n\n让我写状态更新块：\n\n等等，c-distil-0的字段有“心情状态”和“金钱”吗？让我看看。\n\n我应该更新哪个？我不确定哪个是权威的。让我都更新吧。\n\n好，让我写最终版本。\n\n另外，我需要注意正文末尾的【角色清单】和状态更新块。让我安排好顺序：\n1. 正文\n2. 【角色清单】向明初、山楂\n3."
            .to_string();
        let out = strip_trailing_metadiscourse(raw);
        assert!(!out.contains("版本不错"), "自检段应剥离: {out}");
        assert!(!out.contains("让我检查一下"), "自检段应剥离: {out}");
        assert!(!out.contains("状态更新"), "状态思考应剥离: {out}");
        assert!(out.contains("那个寒假，值得等"), "正文应保留: {out}");
        assert!(out.contains("【角色清单】"), "角色清单应保留: {out}");
        // 正文以自然句结尾（不再以「3.」等推理序号收尾）
        assert!(!out.trim_end().ends_with("3."), "不应以推理序号收尾: {out}");
    }

    #[test]
    pub(crate) fn strip_trailing_metadiscourse_keeps_normal_ending() {
        // 正常正文结尾不被误伤
        let mut body = "她没回头。窗外的滴水声又响了两下，才听见她轻轻“嗯”了一声。\n\n【角色清单】母亲、我".to_string();
        let _ = strip_fix_thinking_blocks(&mut body);
        assert!(body.contains("滴水声"), "正文应完整保留: {body}");
        assert!(body.contains("角色清单"), "选项块应保留: {body}");
    }

    #[test]
    pub(crate) fn strip_fix_thinking_blocks_extracts_thinking_and_story() {
        let mut body = "<thinking>好的，我需要根据审稿意见逐条分析：1. 人物声线不稳 2. 节奏拖沓</thinking>\n\n<story>这是修订后的完整正文，人物声线统一。</story>".to_string();
        let thinking = strip_fix_thinking_blocks(&mut body);
        assert!(thinking.contains("审稿意见"), "思维段应剥离返回");
        assert_eq!(body, "这是修订后的完整正文，人物声线统一。");
    }

    #[test]
    pub(crate) fn strip_fix_thinking_blocks_without_tags_keeps_body() {
        let mut body = "普通叙事正文，没有标签。".to_string();
        let thinking = strip_fix_thinking_blocks(&mut body);
        assert!(thinking.is_empty(), "无标签必须原样返回");
        assert_eq!(body, "普通叙事正文，没有标签。");
    }

    #[test]
    pub(crate) fn strip_fix_thinking_blocks_story_without_thinking() {
        let mut body = "<story>只有正文块</story>".to_string();
        let thinking = strip_fix_thinking_blocks(&mut body);
        assert!(thinking.is_empty());
        assert_eq!(body, "只有正文块");
    }

    #[test]
    pub(crate) fn strip_fix_thinking_blocks_unclosed_story_strips_open_tag() {
        // [fix 2026-08-15] <story> 开标签无 </story> 闭合（正文被 max_tokens
        // 截断实踩：宿醉 msg 残留「<story> 烛火在茶几上跳着…」）：剥开标签，
        // 保留其后正文，不泄漏标签残片。
        let mut body = "<story>烛火在茶几上跳着。我指尖还搭在她肩头。".to_string();
        let thinking = strip_fix_thinking_blocks(&mut body);
        assert!(thinking.is_empty());
        assert!(!body.contains("<story>"), "开标签残片必须剥离");
        assert!(body.starts_with("烛火在茶几上跳着"), "正文内容保留");
    }

    #[test]
    pub(crate) fn strip_fix_thinking_blocks_unclosed_story_with_prefix_keeps_prefix() {
        // <story> 前有普通正文（模型把部分正文写在标签外）→ 前缀+块内都保留
        let mut body = "开头几句正文。<story>中间正文被截断".to_string();
        let thinking = strip_fix_thinking_blocks(&mut body);
        assert!(thinking.is_empty());
        assert!(!body.contains("<story>"));
        assert!(body.contains("开头几句正文。"), "前缀正文保留");
        assert!(body.contains("中间正文被截断"), "块内正文保留");
    }

    #[test]
    pub(crate) fn strip_fix_thinking_blocks_unclosed_tag_truncates() {
        let mut body = "正文开头\n<thinking>只有开头没有闭合".to_string();
        let thinking = strip_fix_thinking_blocks(&mut body);
        assert!(thinking.is_empty());
        assert_eq!(body, "正文开头", "开标签残片应截断，保留标签前内容");
    }

    #[test]
    pub(crate) fn strip_fix_thinking_blocks_multiple_blocks() {
        let mut body = "前文\n<thinking>第一段思维</thinking>\n中间\n<thinking>第二段思维</thinking>\n<story>最终正文</story>".to_string();
        let thinking = strip_fix_thinking_blocks(&mut body);
        assert!(thinking.contains("第一段思维") && thinking.contains("第二段思维"));
        assert_eq!(body, "前文\n中间\n最终正文");
    }

    // ─── strip_heavy_fix_preamble Heavy fix 思维前缀剥离 单测 ───────────────

    #[test]
    pub(crate) fn strip_heavy_fix_preamble_removes_review_thought() {
        // 实踩泄漏模式：「好的，我需要根据审稿意见修订这段正文。让我仔细分析每条审稿意见…」
        let mut body = "好的，我需要根据审稿意见修订这段正文。让我仔细分析每条审稿意见，然后重写正文。\n\n审稿意见：\n1. **major continuity**：删除或改写“我伸手”这一动作。\n\n烛火重新亮起来的时候，她已经把背心穿回去了。我伸手，碰了碰她盘起的发髻。".to_string();
        let stripped = strip_heavy_fix_preamble(&mut body);
        assert!(stripped.contains("审稿意见"), "思维段应被剥离: {stripped}");
        assert!(stripped.contains("让我仔细分析"), "思维段应含分析前缀");
        assert!(body.starts_with("烛火重新亮起来"), "正文应从叙事段开始: {body}");
    }

    #[test]
    pub(crate) fn strip_heavy_fix_preamble_keeps_normal_narrative() {
        // 正常「好的，」开头但无审稿特征 → 不剥
        let mut body = "好的，她端着酒走过来，杯沿抵在我唇边。\n\n酒是温的，带着一点甜。".to_string();
        let stripped = strip_heavy_fix_preamble(&mut body);
        assert!(stripped.is_empty());
        assert!(body.starts_with("好的，她端着酒"));
    }

    #[test]
    pub(crate) fn strip_heavy_fix_preamble_removes_let_me_rewrite() {
        // [fix 2026-08-15] 部署后实踩 msg7「让我想想。前文是…」/ msg5「让我重写正文： --- 」：
        // 无标签思维变体（模型未按 <thinking> 输出）必须剥离。
        let mut body = "让我重写正文：\n\n---\n\n左手覆上左边的丰盈时，能感到她整个人震了一下。掌心被那分量填满。\n\n我低下头，嘴唇贴上右边。".to_string();
        let stripped = strip_heavy_fix_preamble(&mut body);
        assert!(stripped.contains("让我重写"), "思维段应被剥离: {stripped}");
        assert!(body.starts_with("左手覆上"), "正文应从叙事段开始: {body}");
    }

    #[test]
    pub(crate) fn strip_heavy_fix_preamble_removes_let_me_think() {
        let mut body = "让我想想。前文是「左手覆上左边的丰盈时……我低下头，嘴唇贴上右边。」所以现在应该…\n\n她整个人震了一下，我松开嘴，看着她的两个乳尖。".to_string();
        let stripped = strip_heavy_fix_preamble(&mut body);
        assert!(stripped.contains("让我想想"), "思维段应被剥离: {stripped}");
        assert!(body.starts_with("她整个人震了一下"), "正文应从叙事段开始: {body}");
    }

    #[test]
    pub(crate) fn strip_heavy_fix_tail_removes_self_check() {
        // [fix 2026-08-15] 部署后实踩 msg5 尾部「--- 好，这个版本可以输出。
        // 让我再检查一下角色清单：母亲、我。」：尾部自检段必须剥离。
        let mut body = "烛火在她身上晃了晃。肩头那只手始终没有移开。\n\n---\n\n好，这个版本可以输出。\n让我再检查一下角色清单：母亲、我。".to_string();
        let stripped = strip_heavy_fix_tail(&mut body);
        assert!(stripped.contains("版本可以输出"), "尾部自检应被剥离: {stripped}");
        assert!(body.contains("肩头那只手始终没有移开"), "正文保留");
        assert!(!body.contains("角色清单"), "自检段不残留: {body}");
    }

    #[test]
    pub(crate) fn strip_heavy_fix_tail_removes_check_needed() {
        let mut body = "她没接话，嘴唇动了动，又抿紧。\n\n再检查是否需要【检定】。玩家在说「撸管子」这种直白的话，这是言语交锋，结果比较确定，不需要掷骰。".to_string();
        let stripped = strip_heavy_fix_tail(&mut body);
        assert!(stripped.contains("再检查是否需要"), "尾部自检应被剥离: {stripped}");
        assert!(body.contains("嘴唇动了动"), "正文保留");
        assert!(!body.contains("【检定】"), "自检段不残留: {body}");
    }

    #[test]
    pub(crate) fn strip_heavy_fix_tail_keeps_normal_narrative() {
        // 正常叙事里含「再检查」字样但位于开头/中段 → 不剥
        let mut body = "她让我再检查一下伤口，指尖按在绷带上。\n\n烛火在她眼里晃了晃。".to_string();
        let stripped = strip_heavy_fix_tail(&mut body);
        assert!(stripped.is_empty());
        assert_eq!(body, "她让我再检查一下伤口，指尖按在绷带上。\n\n烛火在她眼里晃了晃。");
    }

    #[test]
    pub(crate) fn strip_heavy_fix_preamble_no_double_newline_keeps_body() {
        // 无 \n\n 边界 → 保守不剥
        let mut body = "好的，我需要根据审稿意见修订这段正文，但全部挤在一段里没有双换行。".to_string();
        let stripped = strip_heavy_fix_preamble(&mut body);
        assert!(stripped.is_empty());
        assert!(body.starts_with("好的，我需要"));
    }

    #[test]
    pub(crate) fn strip_heavy_fix_preamble_user_asks_variant() {
        // 实踩变体：「用户要求我作为内容负责人，根据审稿意见修订正文。让我仔细分析…」
        let mut body = "用户要求我作为内容负责人，根据审稿意见修订正文。让我仔细分析每条审稿意见，然后重写正文。\n\n审稿意见：\n1. **major continuity**：玩家输入。\n\n烛火重新亮起来的时候，她已经把背心穿回去了。".to_string();
        let stripped = strip_heavy_fix_preamble(&mut body);
        assert!(stripped.contains("用户要求我"), "思维段应被剥离: {stripped}");
        assert!(body.starts_with("烛火重新亮起来"), "正文应从叙事段开始: {body}");
    }

    #[test]
    pub(crate) fn strip_heavy_fix_preamble_numbered_continuation_variant() {
        // 实踩变体：多轮 fix 的续篇，从「6. **minor style**」编号开头
        let mut body = "6. **minor style**：『窗外虫鸣一声接一声』在正文中出现了两次。\n\n7. **minor pacing**：从划拳结束到母亲回房，节奏偏快。\n\n让我重新设计这段正文：\n\n烛火重新亮起来的时候，她正站在窗边。".to_string();
        let stripped = strip_heavy_fix_preamble(&mut body);
        assert!(!stripped.is_empty(), "编号续篇思维应被剥离");
        assert!(body.starts_with("烛火重新亮起来"), "正文应从叙事段开始: {body}");
    }

    #[test]
    pub(crate) fn strip_preamble_wo_shi_variant() {
        // 「好的，我正坐在妈妈身边…」模式（无「我是」字面，但仍是导演自白）
        let mut body = "好的，我正坐在妈妈身边，她还在假装没事。我心里却在冷笑——那个混蛋爸爸，这次我一定要让他付出代价。<场景：林逸家客厅｜傍晚｜林逸、陆清韵>\n\n你坐到妈妈身边。".to_string();
        let stripped = strip_director_preamble(&mut body);
        assert!(stripped.contains("好的，我正坐在妈妈身边"));
        assert!(stripped.contains("付出代价"));
        assert!(body.starts_with("<场景：林逸家客厅"));
        assert!(body.contains("你坐到妈妈身边"));
    }

    #[test]
    pub(crate) fn strip_preamble_director_plan_variant() {
        // 「好的，玩家扮演林逸…好，开始写。<场景…>」导演计划模式
        let mut body = "好的，玩家扮演林逸，十四章初中生，刚刚回到家。这是第一章，当前节点n1。我需要呈现陆清韵的犹豫。最后给玩家选项，三个出口：n2/n3/n4。好，开始写。<场景：林逸家客厅｜傍晚｜林逸、陆清韵>\n\n陆清韵听到你这句话，身子明显僵了一下。".to_string();
        let stripped = strip_director_preamble(&mut body);
        assert!(stripped.contains("好的，玩家扮演林逸"));
        assert!(stripped.contains("开始写"));
        assert!(stripped.contains("n1"));
        assert!(body.starts_with("<场景：林逸家客厅"));
        assert!(body.contains("陆清韵听到你这句话"));
    }

    #[test]
    pub(crate) fn strip_preamble_plain_wo_prefix_keeps_body() {
        // 以「我是」开头但无 <场景 边界 → 不剥
        let mut body = "我是林逸，天才黑客。今天放学回家，发现妈妈又哭了。".to_string();
        let stripped = strip_director_preamble(&mut body);
        assert!(stripped.is_empty());
        assert_eq!(body, "我是林逸，天才黑客。今天放学回家，发现妈妈又哭了。");
    }
