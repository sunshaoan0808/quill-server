//! parts/turn/leak_strip — [2026-10-05 P1] 从 p006 逐字节迁出的思维泄漏剥离族：
//! strip_trailing_metadiscourse / strip_director_preamble / strip_lite_reasoning_leak /
//! strip_heavy_fix_preamble / strip_heavy_fix_tail。p009 的 26 个 strip 测试守门。
//! 以 `use super::super::*;` 直连 parts 作用域（符号经 parts/mod.rs 的 use 块与重导出供给）；
//! 本文件不自带 use 头，cargo fix 无桥可拆（工程纪律见 docs/REFACTOR_20260927.md）。
#![allow(unused_imports)]

use super::super::*;

/// [P2 2026-08-15] 剥离 fix 输出尾部的元话语段（自指动作 + 审稿清单特征）。
/// 从正文末尾向前找「让我检查/让我重新组织/让我起草/让我输出最终版本/再检查/让我分析审稿意见」等
/// 段起点，截断其后所有内容。多段 `---` 拼接时取最后一段正文（若最后一段是元话语则继续前溯）。
pub(crate) fn strip_trailing_metadiscourse(work: String) -> String {
    const SELF_REF_MARKERS: [&str; 35] = [
        "让我检查",
        "让我再检查",
        "让我重新组织",
        "让我重新设计",
        "让我起草",
        "让我看看",
        "让我输出最终版本",
        "让我分析审稿意见",
        "再检查是否需要",
        "再检查一下",
        "好，让我输出",
        "让我逐条",
        // [fix 2026-08-16 档位全覆盖] deepseek-v4-flash-free 正文后自检变体
        // （实踩 msg35：「嗯，这个版本不错。让我检查一下：1. 时间…✓」+
        // 状态更新思考「让我写状态更新块：…我应该更新哪个？…」）——
        // 段首不是 markers 而是「嗯，/等等/不过」等引导词，故改段内包含匹配。
        "这个版本不错",
        "让我检查一下",
        "让我写状态更新",
        "让我更新",
        "我应该更新哪个",
        "让我看看：",
        "让我看看,",
        "我不确定哪个是权威",
        "让我都更新",
        "让我写最终版本",
        "让我安排好顺序",
        // [fix 2026-08-16 实踩]「嗯，这样应该可以。现在让我把完整的叙事写出来。」
        // +「还要注意：- 正文末尾需要角色清单」——正文后自检的「让我把」变体。
        "让我把完整的叙事",
        "让我把最终",
        "让我把正文",
        "还要注意",
        "正文末尾需要",
        "现在让我把",
        "这样应该可以",
        "接下来还要",
        // [fix 2026-08-16 实踩 21:47] 时间推进规则复述：「在正文末尾标注，因为我
        // 在正文中间已经过夜了。嗯……规则说…」——LLM 解释为什么不在末尾标注
        // [时间推进]，属尾部自检思考，旧表漏判。
        "在正文末尾标注",
        "因为我", "嗯……", "规则说",
    ];
    // 分段过滤：按 \n\n 拆段，删除「含自指标记的段」与「审稿清单段」，
    // 重组剩余段。仅当正文含多段且至少一段是元话语时介入（单段正文不误伤）。
    let segments: Vec<&str> = work.split("\n\n").collect();
    if segments.len() < 2 {
        // 单段：整段含自指标记 → 纯自检无正文
        let t = work.trim_start();
        if SELF_REF_MARKERS.iter().any(|m| t.contains(m)) {
            return String::new();
        }
        return work;
    }
    let kept: Vec<&str> = segments
        .iter()
        .filter(|seg| {
            let line = seg.trim_start();
            let is_meta = SELF_REF_MARKERS.iter().any(|m| line.contains(m))
                || line.starts_with("问题")
                || line.starts_with('✅')
                || line.starts_with('❌')
                || line.starts_with("出现了")
                // [fix 2026-08-16] 状态更新草案段（正文后「状态更新：xxx」= 模型对状态块的
                // 思考草稿而非正文；正式【状态更新】块由 split_state_updates_from_narrative 处理）
                || line.starts_with("状态更新");
            !is_meta
        })
        .copied()
        .collect();
    // 若过滤后只剩 1 段或全部保留，说明无元话语 → 原样（防误伤）
    if kept.len() == segments.len() || kept.is_empty() {
        return work;
    }
    let mut out = kept.join("\n\n").trim().to_string();
    // 剥掉残留的分隔线段（--- / *** / ===）
    let segs2: Vec<&str> = out.split("\n\n").collect();
    let kept2: Vec<&str> = segs2
        .iter()
        .filter(|s| {
            let t = s.trim();
            t != "---" && t != "***" && t != "===" && !t.is_empty()
        })
        .copied()
        .collect();
    out = kept2.join("\n\n").trim().to_string();
    out
}

/// S4: 提取【导演计划】块（同【检定】括号深度匹配骨架）。
/// 无块 → None；「【导演计划】none」→ Some(Skip)（剥离块）；JSON 且 goal 非空 → Some(Set)（剥离块）。
/// 解析失败 / goal 为空 → 保留原文不剥离。
pub(crate) fn strip_director_preamble(body: &mut String) -> String {
    // 剥离正文开头的「导演自白」段（模型偶发泄漏的内部推理，TEN_ROUND_PLOT_VERIFY
    // R4/R8 模式：「好的，我是XX…我要怎么演…」直到第一个 `<场景` 标签前）。
    // 仅当 body 以明确自白前缀开头且存在 `<场景` 边界时才剥离，避免误伤正常叙事。
    // 返回剥离出的自白段；body 原地更新为剩余正文。
    let trimmed_head = body.trim_start();
    // 自白前缀：「好的，」「好的我」「我是」（导演式自白开头；实测模式含
    // 「好的，我是林逸」「好的，我正坐在妈妈身边」「好的，玩家扮演林逸」，
    // 统一以「好的，」覆盖所有变体）。
    let is_preamble = trimmed_head.starts_with("好的，")
        || trimmed_head.starts_with("好的,")
        || trimmed_head.starts_with("好的我")
        || trimmed_head.starts_with("我是");
    if !is_preamble {
        return String::new();
    }
    // 需要 `<场景` 标签作为正文起点边界（无标签时保守不剥，避免吞掉整个正文）
    let Some(scene_mark) = trimmed_head.find("<场景") else {
        return String::new();
    };
    if scene_mark == 0 {
        return String::new();
    }
    let preamble = trimmed_head[..scene_mark].trim().to_string();
    let rest = trimmed_head[scene_mark..].to_string();
    *body = rest;
    preamble
}

/// [fix 2026-08-16] Lite 档思维链泄漏剥离。Lite 直出不走 run_quality_refine 多轮
/// 管道，Standard/Heavy 的 fix 阶段剥离（strip_fix_thinking_blocks）对 lite 完全不
/// 生效（`TurnQuality::Lite => Ok((draft.to_string(), None))` 裸返回），历史修复
/// （360bf20 等）只补在 multi-round 管道函数上，导致默认档位 lite 的推理泄漏直落正文。
/// 实测变体（TURN4）：长剧情快速推进时模型输出无 `<场景` 标签的章节规划推理，
/// 以「根据章节大纲/当前节点/出口是…」开头，正文被推理段包裹且重复多次。
/// 策略：仅当输出无 `<场景` 标签且含明显推理前缀时介入，按 `\n\n` 分段，用
/// 负向前缀剔除推理段，取「最长连续叙事段组」作为正文（正文在推理中被重复多次，
/// 最长连续组即干净正文）。正常结构化回合（含 `<场景`）不误伤。
pub(crate) fn strip_lite_reasoning_leak(draft: String) -> String {
    let head = draft.trim_start();
    // 正常结构化回合以 <场景 标签开头 → 不介入（避免误伤）。
    if head.starts_with("<场景") {
        return draft;
    }
    // 推理/规划段负向前缀：真正的叙事段不以这些开头。
    const NEG_PREFIX: [&str; 66] = [
        "根据", "让我", "嗯，", "嗯,", "也许", "考虑到", "实际上", "回顾", "重新阅读",
        "所以", "因此", "那么", "但指令", "鉴于", "我应该", "我不应该", "然后提供",
        "角色清单", "然后选项", "这样应该", "等等", "好的，", "好的,", "当前节点",
        "出口是", "节点", "这些选项", "它处理", "或者", "故事继续", "看看", "让我们",
        "在亲密时刻之后", "待我",
        // [fix 2026-08-16] flash-free 思考混入 content 的新变体：reasoning_content
        // 只装第一段思考，后续「我需要/我要用/我的目标」等规划段混进 content，
        // 旧表缺这些前缀 → leak 判 false → 思考链原样保留为「正文」。
        "我需要", "现在我需要", "接下来我要", "接下来我", "我要用", "我要先", "我想先",
        "我的目标", "我的想法是", "好，我来", "好，我", "开始写正文", "正文如下", "下面开始",
        "让我来", "让我现在", "首先，", "首先,", "然后我", "接着我", "我先",
        // [fix 2026-08-16] 尾部自检变体：「最后，让我确认/最后让我确认一下」——
        // 以「最后，」开头不以「让我」开头，旧表漏判 → 尾部思考混入正文。
        "最后，让我", "最后让我", "最后，我", "最后我",
        // [fix 2026-08-16 实踩 21:23] 「好，让我写正文。」——以「好，让我」开头
        // 匹配不到「好，我」（第二个字是「让」非「我」）也匹配不到「让我」（段首
        // 是「好」），旧表漏判 → 思考残留正文首段。
        "好，让我", "好，我现在", "好，我需要",
        // [fix 2026-08-16 实踩 21:35] 「好，最终正文：」——交付前的宣告段，
        // 与「好，让我写正文」同族但以「好，最终」开头，补全兜底。
        "好，最终正文", "好，以下是最终", "好，下面是正文", "好，正文如下",
    ];
    // 判定是否真的泄漏：首段命中推理前缀（无标签 + 推理开头 → 高度疑似泄漏）。
    let first = head.split("\n\n").next().unwrap_or("").trim();
    let leak = NEG_PREFIX.iter().any(|p| first.starts_with(p));
    if !leak {
        return draft;
    }
    // 按段切分，标记叙事段，求最长连续叙事段组。
    let segments: Vec<&str> = head.split("\n\n").map(|s| s.trim()).collect();
    let mut best: Vec<&str> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for seg in segments {
        // [fix 2026-08-16 剥离误杀] 中文正文常含短句段（「走过去。」4字、
        // 「没有说话。」5字、对话单行「“妈。”」），旧阈值 len>=10 把这些
        // 正常叙事判为思考段 → 整篇正文被剥光（实踩 21:05 回合 content 只剩
        // 42 字思考尾）。阈值降至 2（仅过滤空段/单字语气词），NEG 前缀仍过滤
        // 真正的思考/规划段。
        let is_narr = seg.len() >= 2 && !NEG_PREFIX.iter().any(|p| seg.starts_with(p));
        if is_narr {
            cur.push(seg);
            if cur.len() > best.len() {
                best = cur.clone();
            }
        } else {
            cur.clear();
        }
    }
    // 没有找到足够叙事段 → 保守返回原文（避免吞掉整个正文）。
    if best.len() < 2 {
        return draft;
    }
    best.join("\n\n").trim().to_string()
}

/// [修复 2026-08-15] Heavy 管道 fix 阶段思维前缀剥离。
/// 实测泄漏模式：「好的，我需要根据审稿意见修订这段正文。让我逐条分析…」
/// 「好的，我需要根据审稿意见修订这段正文。让我仔细分析每条审稿意见…」
/// 这类 fix 思维没有 `<场景` 标签，strip_director_preamble 的保守规则不会剥。
/// 识别「好的，我需要…审稿/修订/让我…」前缀，剥离思维段 + 审稿意见列表，
/// 直到遇到真正的叙事段。仅在确有审稿/修订思维特征时剥离（避免误伤正常
/// 「好的，」开头叙事）。
pub(crate) fn strip_heavy_fix_preamble(body: &mut String) -> String {
    let trimmed = body.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    let is_fix_thought = lower.starts_with("好的，我需要根据审稿")
        || lower.starts_with("好的，我需要根据审稿意见")
        || lower.starts_with("好的，让我仔细")
        || lower.starts_with("好的，我来")
        || lower.starts_with("好的，先")
        || lower.starts_with("好的，根据审稿")
        || lower.starts_with("用户要求我")
        || lower.starts_with("根据审稿意见")
        // [fix 2026-08-15] 无标签思维新变体（部署后实踩 msg7「让我想想。前文是…」、
        // msg5「让我重写正文： --- 」）——模型未按 <thinking> 标签输出时泄漏。
        || lower.starts_with("让我想想")
        || lower.starts_with("让我重写")
        || lower.starts_with("让我重新")
        || lower.starts_with("让我梳理")
        || lower.starts_with("我需要根据")
        || lower.starts_with("我需要重新")
        || (lower.starts_with("好的，") && lower.contains("审稿"))
        || (lower.starts_with("好的，") && lower.contains("修订这段正文"))
        || (lower.contains("审稿意见") && (lower.contains("minor") || lower.contains("major") || lower.contains("修订")))
        || (lower.starts_with('6') && lower.contains("minor"))
        || (lower.starts_with('7') && lower.contains("minor"))
        || (lower.starts_with('8') && lower.contains("minor"))
        || (lower.starts_with('9') && lower.contains("minor"));
    if !is_fix_thought {
        return String::new();
    }
    // 逐段剥离：思维段、审稿意见列表（含编号/标题/引用）都算思维前缀，
    // 直到遇到普通叙事段（不以编号/破折号/引号行首开头的段落）。
    let mut rest = trimmed;
    let mut thought = String::new();
    loop {
        let Some(para_end) = rest.find("\n\n") else {
            // 无双换行边界：若已剥出思维段，剩余 rest 即正文；否则（首段即
            // 无换行）无法安全区分 → 保守不剥。
            if thought.is_empty() {
                return String::new();
            }
            *body = rest.to_string();
            return thought;
        };
        let para = rest[..para_end].trim();
        let after = rest[para_end + 2..].trim_start();
        let para_is_thought = para.starts_with("好的，")
            || para.starts_with("审稿意见")
            || para.starts_with("修改意见")
            || para.starts_with('1')
            || para.starts_with('2')
            || para.starts_with('3')
            || para.starts_with('4')
            || para.starts_with('5')
            || para.starts_with('6')
            || para.starts_with('7')
            || para.starts_with('8')
            || para.starts_with('9')
            || para.starts_with('-')
            || para.starts_with('*')
            || para.starts_with("**")
            || para.starts_with('“')
            || para.starts_with('「')
            || para.starts_with("然后")
            || para.starts_with("接下来")
            || para.starts_with("首先")
            || para.starts_with("最后")
            || para.starts_with("让我重新设计")
            || para.starts_with("让我重新")
            || para.starts_with("让我写")
            || para.starts_with("让我想想")
            || para.starts_with("让我重写")
            || para.starts_with("让我梳理")
            || para.starts_with("我需要在")
            || para.starts_with("情绪曲线")
            || para.starts_with("场景")
            || para.starts_with("正文")
            || para.starts_with("修改后")
            || para.starts_with("修订后")
            || para.contains("意见")
            || para.contains("修订")
            || para.contains("审稿");
        if !para_is_thought {
            // 遇到叙事段：思维剥离到此为止，正文从当前段起
            if thought.is_empty() {
                return String::new();
            }
            *body = rest.to_string();
            return thought;
        }
        if thought.is_empty() {
            thought = para.to_string();
        } else {
            thought = format!("{}\n\n{}", thought, para);
        }
        rest = after;
    }
}

/// [fix 2026-08-15] Heavy 管道 fix 尾部自检剥离（部署后实踩 msg5「--- 好，这个版本可以输出。
/// 让我再检查一下角色清单：母亲、我。」、msg7/msg11「再检查是否需要【检定】…」尾部泄漏）。
/// 模型在正文末尾追加自我检查/质量确认段（无 <thinking> 标签），剥离后归入 reasoning。
/// 返回剥离出的尾部思维段；body 原地改写为纯正文（尾部截断）。未匹配时返回空串不动正文。
pub(crate) fn strip_heavy_fix_tail(body: &mut String) -> String {
    let markers: [&str; 8] = [
        "让我再检查",
        "让我再确认",
        "再检查是否需要",
        "再确认是否需要",
        "好，这个版本可以输出",
        "好，这个版本已经可以输出",
        "让我输出最终版本",
        "让我最终输出",
    ];
    // 只在正文中后段匹配（防止误伤开头正常叙事里的罕见短语）；取最后一个匹配点。
    // [fix] 精确规则：marker 必须位于正文最后一段（最后一个 \n\n 之后）——自检段
    // 总是以独立段落出现在文末；单段正文则要求位于中后段。
    let trimmed = body.trim_end().to_string();
    let last_para_start = trimmed.rfind("\n\n").map(|p| p + 2).unwrap_or(0);
    let mut cut: Option<usize> = None;
    for m in markers.iter() {
        let mut search_from = last_para_start;
        while let Some(rel) = trimmed[search_from..].find(m) {
            let abs = search_from + rel;
            if last_para_start == 0 && abs < trimmed.len() / 2 {
                // 单段正文：只接受位于中后段的匹配
                search_from = abs + m.len();
                continue;
            }
            cut = Some(cut.map_or(abs, |c| c.max(abs)));
            break;
        }
    }
    let Some(cut_pos) = cut else {
        return String::new();
    };
    // 从匹配点前最近的分段边界（--- 或双换行）开始截，若匹配点紧跟在 --- 后
    // （msg5 实踩：「--- 好，这个版本可以输出。」）则连分隔线一起剥。
    let mut start = cut_pos;
    let before = &trimmed[..cut_pos];
    if let Some(dash_rel) = before.rfind("---") {
        let dash_abs = dash_rel;
        // --- 与匹配点之间无正文（只隔空白/换行）时，从 --- 起剥
        if before[dash_abs + 3..].trim().is_empty() || before[dash_abs + 3..].trim().chars().count() <= 20 {
            start = dash_abs;
        }
    }
    let tail = trimmed[start..].trim();
    let head = trimmed[..start].trim_end();
    if tail.is_empty() || head.is_empty() {
        return String::new();
    }
    *body = head.to_string();
    tail.to_string()
}

/// 合并思维段到 reasoning 缓冲（去空、双换行分隔）。
pub(crate) fn append_thinking(dst: &mut String, src: &str) {
    let src = src.trim();
    if src.is_empty() {
        return;
    }
    if !dst.is_empty() {
        dst.push_str("\n\n");
    }
    dst.push_str(src);
}
