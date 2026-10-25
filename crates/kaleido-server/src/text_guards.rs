//! text_guards.rs — 接线层：把「吞噬后未接线」的 `slop2` / `continuity2` 接进回合管线。
//!
//! 背景：这两个模块在 `museai-core` 里完整可编译、自带单测，但从 `main.rs` 生产入口
//! 走不到（见 `docs/接线审计-摆设功能全摸排-20260914.md` §4）。本文件是它们唯一的
//! 生产调用方 —— 接线后二者不再「不可达」。
//!
//! 门控（默认关，与 L4 delta 同款习惯）：
//! - `MUSEAI_SLOP_GUARD=1`       → ① 落库前正文清洗 ② 下轮注入「刚用过的腔/复述过的整句」
//! - `MUSEAI_CONTINUITY_GUARD=1` → ① 注入场景状态块 ② 时间相位冲突检查 ③ 场景停滞 nudge
//! 两个开关都不开时：调用点全部 no-op，生产行为逐字节不变（纯增量，可秒回退）。
//!
//! 纪律：全部确定性词法/统计判断，**零额外 LLM 调用**；清洗只删「绝不可能是角色台词」
//! 的东西（markdown 标题、尾随未配平 `*`、连续空行、串台行），不做语义改写。
//!
//! 已知局限（诚实标注，勿当能力）：
//! - `detect_time_break` 的吞词表是英文（tomorrow/yesterday…），中文正文里
//!   「清晨/夜晚」这类相位词命中后可能误报 → 本层只**记日志/写 guard_events**，
//!   绝不用它打回或重生成。
//! - 衣物 break 需要「已脱层」记账（`removed_layers`），当前会话模型未提供该数据源，
//!   故本层不接衣物 break（宁漏勿错）。

use kaleido_core::{continuity2, slop2};

/// 环境变量解析：1/true/on/yes 视为开（大小写不敏感），其余/未设置＝关。
fn env_flag(v: Option<&str>) -> bool {
    match v.map(|s| s.trim().to_ascii_lowercase()) {
        Some(s) => matches!(s.as_str(), "1" | "true" | "on" | "yes"),
        None => false,
    }
}

/// `MUSEAI_SLOP_GUARD`（默认关）。
pub fn slop_guard_enabled() -> bool {
    env_flag(std::env::var("MUSEAI_SLOP_GUARD").ok().as_deref())
}

/// `MUSEAI_CONTINUITY_GUARD`（默认关）。
pub fn continuity_guard_enabled() -> bool {
    env_flag(std::env::var("MUSEAI_CONTINUITY_GUARD").ok().as_deref())
}

/// 清洗审计信息：落库前后差异必须可观测（失败/改动可见性）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanOutcome {
    /// 清洗掉的字符数（0 = 正文未被改动）。
    pub removed_chars: usize,
    /// 命中的 AI 腔标签（只报不改）。
    pub slop_labels: Vec<String>,
    /// 本回合正文与最近正文逐字重复（rp 的 verbatim echo）。
    pub verbatim_echo: bool,
    /// 本回合正文与最近任一轮正文高度重复。
    pub duplicate_of_recent: bool,
}

impl CleanOutcome {
    pub fn changed(&self) -> bool {
        self.removed_chars > 0
    }
}

/// 落库前正文清洗 + 审计。`recent` 为最近几轮 assistant 正文（用于重复检测）。
pub fn clean_assistant_text(text: &str, recent: &[&str]) -> (String, CleanOutcome) {
    let cleaned = slop2::clean_model_output_basic(text);
    let removed_chars = text.chars().count().saturating_sub(cleaned.chars().count());
    let slop_labels = slop2::find_slop(text)
        .into_iter()
        .map(|h| h.label)
        .collect::<Vec<_>>();
    let recent_nonempty: Vec<&str> = recent.iter().copied().filter(|t| !t.trim().is_empty()).collect();
    let prior = recent_nonempty.last().copied();
    let mut out = CleanOutcome {
        removed_chars,
        slop_labels,
        verbatim_echo: slop2::is_verbatim_echo(text, prior),
        duplicate_of_recent: slop2::is_duplicate_of_recent_text(text, &recent_nonempty),
    };
    // 空正文不算「重复」（避免把空回复报成 echo 噪音）。
    if cleaned.trim().is_empty() {
        out.verbatim_echo = false;
        out.duplicate_of_recent = false;
    }
    (cleaned, out)
}

/// 下轮 system prompt 注入：点名模型刚用过的腔 + 复述过的整句；干净返回 None。
pub fn slop_avoidance_block(recent_assistant: &[&str]) -> Option<String> {
    slop2::build_slop_avoidance_note(recent_assistant, 3, 2)
}

/// 场景状态块（`[SCENE STATE]`）：「此刻为真」的事实清单，防模型自相矛盾/重复叙述。
pub fn scene_state_prompt_block(
    char_name: &str,
    user_name: &str,
    location: Option<&str>,
    time_phase: Option<&str>,
    day: Option<i64>,
    scene_turns: i64,
    present: &[&str],
) -> String {
    let facts = continuity2::SceneStateFacts {
        char_name,
        user_name,
        location,
        time_phase,
        day,
        activity: None,
        scene_turns,
        present: present.to_vec(),
        contact: None,
        arousal_line: None,
    };
    continuity2::scene_state_block(&facts)
}

/// 时间相位冲突检查（**只报不改**）：正文把异相位当现在时。`None` = 未命中。
pub fn time_break_note(reply: &str, current_phase: &str) -> Option<String> {
    if reply.trim().is_empty() || current_phase.trim().is_empty() {
        return None;
    }
    let known: Vec<String> = kaleido_core::time_clock::TIME_SLOTS
        .iter()
        .map(|s| s.to_string())
        .collect();
    continuity2::detect_time_break(reply, Some(current_phase), &known)
        .map(|b| continuity2::describe_continuity_break(&b))
}

/// 场景停滞 nudge：`turns_since_progress` 为「距上次时间/场景推进的回合数」。
/// 未达阈值（`STATIC_SCENE_THRESHOLD`）返回空串。
pub fn stagnation_note(char_name: &str, turns_since_progress: i64, location: Option<&str>) -> String {
    continuity2::scene_progression_nudge(turns_since_progress, char_name, location, &[])
}

/// 从正文的场景标签里取**地点段**：`<场景：地点｜时段｜角色>`（繁体 `場景`、
/// 全角/半角冒号都认，取第一个 `｜`/`|` 之前的内容）。无标签 → `None`。
///
/// 这是 P1 里缺失的那个「地点数据源」：正文本来就把地点写在标签里，
/// 以前没有任何代码把它取出来做检查。
pub fn parse_scene_location(text: &str) -> Option<String> {
    const PRES: [&str; 4] = ["<场景：", "<场景:", "<場景：", "<場景:"];
    for pre in PRES {
        if let Some(i) = text.find(pre) {
            let rest = &text[i + pre.len()..];
            let end = rest.find('>').unwrap_or(rest.len());
            let loc = rest[..end]
                .split(['｜', '|'])
                .next()
                .unwrap_or("")
                .trim();
            if !loc.is_empty() {
                return Some(loc.to_string());
            }
        }
    }
    None
}

/// 地点漂移检查（**只报不改**）：正文回到了「别的已知地点」而当前场景标签
/// 不是它。数据源 = 历史正文里的场景标签，调用 `continuity2::detect_location_break`。
///
/// 纪律「宁漏勿错」：本回合正文如果自己写了场景标签（= 模型明说了换场），
/// 一律不算漂移，直接放过。
pub fn location_break_note(history: &[String], reply: &str) -> Option<String> {
    if reply.trim().is_empty() {
        return None;
    }
    let mut known: Vec<String> = Vec::new();
    for t in history {
        if let Some(l) = parse_scene_location(t) {
            if !known.iter().any(|k| k.eq_ignore_ascii_case(&l)) {
                known.push(l);
            }
        }
    }
    if known.is_empty() {
        return None;
    }
    // 本回合自己声明了换场 → 不是漂移
    if let Some(cur) = parse_scene_location(reply) {
        if !known.iter().any(|k| k.eq_ignore_ascii_case(&cur)) {
            return None;
        }
    }
    let current = known.last()?.clone();
    continuity2::detect_location_break(reply, Some(&current), &known)
        .map(|b| continuity2::describe_continuity_break(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_values_default_off() {
        assert!(!env_flag(None));
        assert!(!env_flag(Some("")));
        assert!(!env_flag(Some("0")));
        assert!(!env_flag(Some("false")));
        assert!(!env_flag(Some("off")));
        assert!(env_flag(Some("1")));
        assert!(env_flag(Some("TRUE")));
        assert!(env_flag(Some(" on ")));
        assert!(env_flag(Some("yes")));
    }

    #[test]
    fn clean_strips_markdown_heading_and_dangling_star() {
        // 真实语义（读 slop2::clean_model_output_basic）：标题只在带 `#` 时删；
        // 尾随 `*` 只在全文 `*` 计数为奇数时删（未配平标记），偶数个 `**` 是
        // 合法强调、必须原样保留 —— 这里两条都断言，防「过度清洗」回归。
        let raw = "## 第三章\n\n她笑了。\n\n\n*";
        let (cleaned, out) = clean_assistant_text(raw, &[]);
        assert!(out.changed(), "应记录字符删减: {out:?}");
        assert!(!cleaned.contains("##"), "标题应被去掉: {cleaned:?}");
        assert!(cleaned.ends_with("她笑了。"), "尾随未配平 * 应被去掉: {cleaned:?}");
        assert!(out.removed_chars >= 5, "{}", out.removed_chars);

        let paired = "**她把伞递过来。**";
        let (kept, kept_out) = clean_assistant_text(paired, &[]);
        assert_eq!(kept, paired, "配平的 ** 强调不得被清洗");
        assert_eq!(kept_out.removed_chars, 0);
    }

    #[test]
    fn clean_keeps_clean_text_untouched() {
        let raw = "她抬头看了他一眼，轻声说：「你回来了。」";
        let (cleaned, out) = clean_assistant_text(raw, &[]);
        assert_eq!(cleaned, raw);
        assert_eq!(out.removed_chars, 0);
        assert!(!out.changed());
        assert!(!out.verbatim_echo);
        assert!(!out.duplicate_of_recent);
    }

    #[test]
    fn avoidance_note_flags_repeated_phrases() {
        let a = "She could not help but moan again in the quiet room tonight.";
        let b = "He could not help but moan about the cold water in the room tonight.";
        let note = slop_avoidance_block(&[a, b]).expect("重复短语应产出避雷提示");
        assert!(note.to_lowercase().contains("could not help but"), "{note}");
        // 干净文本不注入
        assert!(slop_avoidance_block(&["她笑了。", "他点了点头。"]).is_none());
        assert!(slop_avoidance_block(&[]).is_none());
    }

    #[test]
    fn time_break_report_only_when_phase_conflicts() {
        let hit = time_break_note("外面的清晨阳光还很刺眼。", "夜晚").expect("异相位应命中");
        assert!(hit.contains("夜晚"), "{hit}");
        assert!(time_break_note("她靠在窗边没有说话。", "夜晚").is_none());
        assert!(time_break_note("", "夜晚").is_none());
    }

    #[test]
    fn scene_block_lists_facts_only() {
        let b = scene_state_prompt_block(
            "林晚",
            "你",
            None,
            Some("夜晚"),
            Some(3),
            2,
            &["苏黎"],
        );
        assert!(b.contains("[SCENE STATE]"), "{b}");
        assert!(b.contains("夜晚"));
        assert!(b.contains("Day 3"));
        assert!(b.contains("林晚"));
        assert!(b.contains("苏黎"));
        // 全空 → 空块（不污染 prompt）
        assert!(scene_state_prompt_block("A", "B", None, None, None, 0, &[]).is_empty());
    }

    #[test]
    fn stagnation_note_only_past_threshold() {
        assert!(stagnation_note("A", continuity2::STATIC_SCENE_THRESHOLD - 1, None).is_empty());
        let n = stagnation_note("A", continuity2::STATIC_SCENE_THRESHOLD, Some("dorm"));
        assert!(!n.is_empty());
        assert!(n.contains("dorm"));
    }

    #[test]
    fn parse_scene_location_handles_both_scripts() {
        assert_eq!(
            parse_scene_location("<场景：雨巷茶馆门前｜夜｜沈棠>她收起伞。").as_deref(),
            Some("雨巷茶馆门前")
        );
        assert_eq!(
            parse_scene_location("正文…<場景:宿舍|清晨|林晚>").as_deref(),
            Some("宿舍")
        );
        assert_eq!(parse_scene_location("没有标签的一回合").as_deref(), None);
        assert_eq!(parse_scene_location("<场景：｜夜｜她>").as_deref(), None);
    }

    #[test]
    fn location_break_only_when_returning_to_known_place() {
        let history = vec![
            "<场景：卧室｜夜｜林晚>她把灯关了。".to_string(),
            "<场景：厨房｜清晨｜林晚>水壶在响。".to_string(),
        ];
        // 正文把「卧室」当现在时（当前应是厨房）→ 命中，且只报不改
        let hit = location_break_note(&history, "林晚回到卧室，坐在床沿翻那本旧相册。")
            .expect("回到已知的旧地点应命中");
        assert!(hit.contains("卧室"), "{hit}");
        // 本回合自己声明换场 → 不算漂移
        assert!(location_break_note(&history, "<场景：客厅｜夜｜林晚>她走进客厅。").is_none());
        // 没有历史标签 → 无数据源，宁漏勿错
        assert!(location_break_note(&["普通正文，没有标签".to_string()], "她走进卧室。").is_none());
        assert!(location_break_note(&history, "").is_none());
    }
}
