//! intent_full.rs — 意图标签chips + rapport + outreach资格（吸收自 pnotisdev/rp dating/intent.ts + rapport.ts + outreach.ts，MIT）。
//!
//! 纪律（抄rp）：
//! - intent本身不加减数值，只当judge上下文；错判翻车（时机不对代价更大）。
//! - reassure/apologize只在tension>=12时出现（无摩擦不给修复chip）。
//! - 同一intent连打3轮起边际递减（diminishing），judge nudge提醒。
//! - rapport是约会中的定性读数（5档），不碰数值；walkOut只认真dealbreaker。
//! - outreach资格是纯确定性门控（频率/沉默时长/冷却/睡眠），模型只写文本不决策。
//! - 全纯函数，零迁移。

/// 意图chip id。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageIntent {
    Flirt,
    Tease,
    OpenUp,
    Reassure,
    Apologize,
}

pub struct IntentSpec {
    pub id: MessageIntent,
    pub label: &'static str,
    pub hint: &'static str,
    pub judge_line: &'static str,
    /// 需要tension>=此值才出现；None=常驻。
    pub gate_tension_at_least: Option<f64>,
}

pub const MESSAGE_INTENTS: [IntentSpec; 5] = [
    IntentSpec {
        id: MessageIntent::Flirt,
        label: "Flirt",
        hint: "Meant as an advance — charm, a compliment with weight behind it.",
        judge_line: "a flirt: read whether it landed. Welcome and well-timed, it builds chemistry; unwanted, clumsy, or too fast for where things stand, it adds tension or costs comfort.",
        gate_tension_at_least: None,
    },
    IntentSpec {
        id: MessageIntent::Tease,
        label: "Tease",
        hint: "Playful needling, not serious.",
        judge_line: "playful teasing, not a real jab. If she enjoys the banter it warms things; if it hits a sore spot or reads as mocking, it stings.",
        gate_tension_at_least: None,
    },
    IntentSpec {
        id: MessageIntent::OpenUp,
        label: "Open up",
        hint: "Sharing something real or vulnerable.",
        judge_line: "the player opening up — sharing something real or vulnerable. Met with warmth it builds trust and closeness; brushed off or overshared too early, it can feel exposing.",
        gate_tension_at_least: None,
    },
    IntentSpec {
        id: MessageIntent::Reassure,
        label: "Reassure",
        hint: "Meant to steady or comfort them.",
        judge_line: "an attempt to reassure or comfort her. If it addresses what is actually wrong it eases tension and builds trust; if it feels generic or dismissive of their feelings, it does little or lands badly.",
        gate_tension_at_least: Some(12.0),
    },
    IntentSpec {
        id: MessageIntent::Apologize,
        label: "Apologize",
        hint: "Meant as a genuine apology.",
        judge_line: "a genuine apology. A real one that names the wrong can clear tension and rebuild respect; a hollow or defensive one (sorry you feel that way) can make things worse.",
        gate_tension_at_least: Some(12.0),
    },
];

pub fn intent_spec(id: MessageIntent) -> &'static IntentSpec {
    MESSAGE_INTENTS.iter().find(|s| s.id == id).unwrap()
}

/// 当前可用的chips（摩擦修复类需tension门控）。
pub fn available_intents(tension: f64) -> Vec<MessageIntent> {
    MESSAGE_INTENTS
        .iter()
        .filter(|s| s.gate_tension_at_least.map(|t| tension >= t).unwrap_or(true))
        .map(|s| s.id)
        .collect()
}

/// judge上下文行（untagged=None）。
pub fn describe_intent_for_judge(id: Option<MessageIntent>, char_name: &str) -> Option<String> {
    let spec = intent_spec(id?);
    Some(format!(
        "The player tagged their latest line as {}. Weigh {}\u{2019}s honest reaction to that; do not just reward the attempt.",
        spec.judge_line.replace("{{char}}", char_name),
        char_name,
    ))
}

/// 同一intent连打轮数（user_intents oldest-first尾部连击）。
pub fn trailing_intent_run(user_intents: &[Option<MessageIntent>]) -> usize {
    let Some(&Some(last)) = user_intents.last() else {
        return 0;
    };
    user_intents.iter().rev().take_while(|i| **i == Some(last)).count()
}

/// 连打3轮起的judge nudge（阈值下=None）。
pub fn repeated_intent_nudge(streak: usize, char_name: &str, user_name: &str) -> Option<String> {
    if streak < 3 {
        return None;
    }
    Some(format!(
        "{user_name} has now played the same kind of move several turns in a row. The novelty has worn off for {char_name}: it lands with much less weight than the first time, and {char_name} might get wry about it, gently point it out, or simply be less visibly affected. Don\u{2019}t keep rewarding the repetition as if it were fresh."
    ))
}

// ---------- rapport（约会定性读数，不碰数值） ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RapportTrajectory {
    LightingUp,
    Warming,
    AtEase,
    PullingBack,
    OnEdge,
}

pub fn rapport_label(t: RapportTrajectory) -> &'static str {
    match t {
        RapportTrajectory::LightingUp => "lighting up",
        RapportTrajectory::Warming => "warming to you",
        RapportTrajectory::AtEase => "at ease",
        RapportTrajectory::PullingBack => "pulling back",
        RapportTrajectory::OnEdge => "on edge",
    }
}

/// rapport 趋势方向（驱动前端 glyph/颜色；抄rp RAPPORT_READS tone）。
pub fn rapport_tone(t: RapportTrajectory) -> &'static str {
    match t {
        RapportTrajectory::LightingUp => "up-strong",
        RapportTrajectory::Warming => "up",
        RapportTrajectory::AtEase => "flat",
        RapportTrajectory::PullingBack => "down",
        RapportTrajectory::OnEdge => "down-strong",
    }
}

/// judge 选这档的依据（抄rp judgeHint）。
pub fn rapport_judge_hint(t: RapportTrajectory) -> &'static str {
    match t {
        RapportTrajectory::LightingUp => "clearly delighted — leaning in, initiating, visibly enjoying this",
        RapportTrajectory::Warming => "opening up, softening, the conversation is flowing better than it started",
        RapportTrajectory::AtEase => "comfortable and steady — neither pulling closer nor away, just present",
        RapportTrajectory::PullingBack => "getting shorter, more guarded, or politely disengaging; the energy is dropping",
        RapportTrajectory::OnEdge => "irritated, hurt, or genuinely uncomfortable; this could tip into them ending the date",
    }
}

// ---------- outreach资格（纯确定性门控） ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutreachFrequency {
    Never,
    Rare,
    Normal,
    Eager,
}

/// 沉默阈值（小时）：rare=48/normal=20/eager=8。
pub fn silence_threshold_hours(f: OutreachFrequency) -> Option<f64> {
    match f {
        OutreachFrequency::Never => None,
        OutreachFrequency::Rare => Some(48.0),
        OutreachFrequency::Normal => Some(20.0),
        OutreachFrequency::Eager => Some(8.0),
    }
}

/// 基础概率：rare=0.12/normal=0.25/eager=0.4；warmth满分再+0.2。
pub fn outreach_base_chance(f: OutreachFrequency) -> f64 {
    match f {
        OutreachFrequency::Never => 0.0,
        OutreachFrequency::Rare => 0.12,
        OutreachFrequency::Normal => 0.25,
        OutreachFrequency::Eager => 0.4,
    }
}

pub fn outreach_chance(f: OutreachFrequency, warmth: f64) -> f64 {
    outreach_base_chance(f) + (warmth.clamp(0.0, 100.0) / 100.0) * 0.2
}

/// 资格门控（纯确定性）：频率never/群聊/约会中/沉默不够/冷却30min内/睡着→skip。
/// 返回Some(reason)才值得roll点；SeedRoll由调用方做（此处只给chance）。
pub fn outreach_eligible(
    frequency: OutreachFrequency,
    is_group: bool,
    in_live_event: bool,
    has_last_message: bool,
    silence_hours: f64,
    minutes_since_last_check: f64,
    is_sleeping: bool,
) -> Option<f64> {
    if frequency == OutreachFrequency::Never || is_group || in_live_event || !has_last_message {
        return None;
    }
    let threshold = silence_threshold_hours(frequency)?;
    if silence_hours < threshold || minutes_since_last_check < 30.0 || is_sleeping {
        return None;
    }
    Some(threshold)
}

/// 约会终判意图小结（抄rp describeIntentsForDate；Flirt ×3）。
pub fn describe_intents_for_date(labels: &[(&str, i64)]) -> Option<String> {
    if labels.is_empty() { return None; }
    let parts: Vec<String> = labels.iter().map(|(l,n)| if *n>1 { format!("{l} ×{n}") } else { l.to_string() }).collect();
    Some(parts.join(", "))
}
/// outreach判定四因（抄rp evaluateOutreach核心序：群聊/事件中/未达静默/睡着/roll）。
pub fn outreach_roll(seed: &str) -> f64 {
    let mut h: u32 = 2166136261;
    for b in seed.bytes() { h ^= b as u32; h = h.wrapping_mul(16777619); }
    ((h % 100000) as f64) / 100000.0
}
pub fn outreach_should_roll(freq: &str, elapsed_ms: i64, last_checked_ms: i64, now_ms: i64, sleeping: bool, in_event: bool, in_group: bool) -> bool {
    if freq=="never" || in_group || in_event || sleeping { return false; }
    let th_h: f64 = match freq { "rare"=>48.0, "eager"=>8.0, _=>20.0 };
    if (elapsed_ms as f64) < th_h*3_600_000.0 { return false; }
    if now_ms - last_checked_ms < 30*60*1000 { return false; }
    true
}

/// 合法意图校验（抄rp isMessageIntent；5枚chip白名单）。
pub fn is_message_intent(v: Option<&str>) -> bool {
    matches!(v, Some("flirt") | Some("tease") | Some("open_up") | Some("reassure") | Some("apologize"))
}
/// rapport合法校验（抄rp isRapportTrajectory；5档白名单）。
pub fn is_rapport_trajectory(v: &str) -> bool {
    matches!(v, "lighting_up" | "warming" | "at_ease" | "pulling_back" | "on_edge")
}
/// rapport judge参数（抄rp RAPPORT_PARAMS；max90/temp0.3/rep1.05）。
pub const RAPPORT_MAX_LENGTH: i64 = 90;
pub const RAPPORT_TEMPERATURE: f64 = 0.3;
pub const RAPPORT_REP_PEN: f64 = 1.05;

/// outreach综合判定（抄rp evaluateOutreach；never/群聊/无消息/事件中/阈值内/冷却内/睡觉→skip）。
pub fn evaluate_outreach_simple(freq: &str, has_participants: bool, has_last_msg: bool, in_event: bool, elapsed_ms: i64, last_checked_ms: i64, now_ms: i64, sleeping: bool, warmth: f64, seed_roll: f64) -> (&'static str, bool, Option<&'static str>) {
    if freq == "never" || freq.is_empty() { return ("skip", false, None); }
    if has_participants || !has_last_msg || in_event { return ("skip", false, None); }
    let threshold_ms = match silence_threshold_hours(match freq { "rare"=>OutreachFrequency::Rare, "eager"=>OutreachFrequency::Eager, _=>OutreachFrequency::Normal }) { Some(h)=> (h as i64)*3600_000, None=> return ("skip", false, None) };
    if elapsed_ms < threshold_ms { return ("skip", false, None); }
    if now_ms - last_checked_ms < crate::relationship_stage::OUTREACH_CHECK_COOLDOWN_MS { return ("skip", false, None); }
    if sleeping { return ("skip", false, None); }
    let chance = outreach_base_chance(match freq { "rare"=>OutreachFrequency::Rare, "eager"=>OutreachFrequency::Eager, _=>OutreachFrequency::Normal }) + (warmth/100.0)*crate::relationship_stage::OUTREACH_MAX_WARMTH_BONUS;
    if seed_roll >= chance { return ("rolled", false, None); }
    let reason = if elapsed_ms >= threshold_ms*2 { "silence" } else if warmth >= 60.0 { "warmth" } else { "silence" };
    ("rolled", true, Some(reason))
}
/// outreach原因行文（抄rp outreachReasonHint；silence/schedule/warmth/life_event四档）。
pub fn outreach_reason_hint(reason: &str, char_name: &str) -> &'static str {
    match reason {
        "schedule" => "the character has something going on in their day worth mentioning",
        "warmth" => "things have been genuinely warm lately and they are thinking of the player",
        "life_event" => "something in their life or surroundings gives them a reason to reach out",
        _ => "it has been quiet for a while and they miss the conversation",
    }
}

/// 离席铁律（抄rp walkOut判词；只认真dealbreaker三件：公然敌意/残忍/露骨求欢，普通摩擦不走）。
pub fn walkout_rule() -> &'static str {
    "walkOut: true ONLY for a genuine dealbreaker — overt hostility, cruelty, or an explicit/crude proposition. false for ordinary friction, awkwardness, a bad joke, or garden-variety on_edge tension. This should be rare."
}
/// rapport note上限120字（抄rp note.slice(0,120)；3-8词场记）。
pub fn rapport_note_clip(note: &str) -> String {
    let t = note.trim();
    t.chars().take(120).collect::<String>().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn friction_chips_gated_by_tension() {
        assert_eq!(available_intents(0.0).len(), 3);
        assert_eq!(available_intents(12.0).len(), 5);
    }

    #[test]
    fn trailing_run_and_nudge() {
        use MessageIntent as MI;
        let v = vec![Some(MI::Flirt), Some(MI::Flirt), Some(MI::Flirt)];
        assert_eq!(trailing_intent_run(&v), 3);
        assert!(repeated_intent_nudge(3, "她", "你").is_some());
        assert!(repeated_intent_nudge(2, "她", "你").is_none());
        assert_eq!(trailing_intent_run(&[Some(MI::Flirt), None]), 0);
    }

    #[test]
    fn outreach_gates() {
        assert!(outreach_eligible(OutreachFrequency::Never, false, false, true, 99.0, 99.0, false).is_none());
        assert!(outreach_eligible(OutreachFrequency::Normal, false, false, true, 5.0, 99.0, false).is_none());
        assert!(outreach_eligible(OutreachFrequency::Normal, false, false, true, 25.0, 99.0, false).is_some());
        assert!(outreach_eligible(OutreachFrequency::Normal, false, false, true, 25.0, 5.0, false).is_none());
        let c = outreach_chance(OutreachFrequency::Normal, 100.0);
        assert!((c - 0.45).abs() < 1e-9);
    }
}