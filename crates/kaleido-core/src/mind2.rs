//! mind2.rs — 角色心境五件套 + authored-state 优先 + stock-romance 预警（吸收自 pnotisdev/rp prompt/mindGuidance.ts，MIT）。
//!
//! 纪律（抄rp）：
//! - emotion ≠ relationship：mood 再差也不扣 warmth，只影响本回合写法。
//! - mood/need 用封闭词表，judge 只能选表内词，不许自由文本。
//! - mood/currentNeed 可展示给玩家；intent/desire/fear 纯模型私有信号。
//! - 全纯函数，零迁移。

/// 心境封闭词表（20）。
pub const MOOD_VOCAB: [&str; 20] = [
    "content", "affectionate", "playful", "excited", "anxious", "guarded", "annoyed", "hurt",
    "sad", "lonely", "jealous", "embarrassed", "confident", "exhausted", "bored", "curious",
    "nostalgic", "proud", "relieved", "tense",
];

/// 潜在需求封闭词表（8）。
pub const NEED_VOCAB: [&str; 8] = [
    "social connection", "solitude", "achievement", "reassurance", "excitement", "stability",
    "recognition", "belonging",
];

pub fn is_mood(v: &str) -> bool {
    MOOD_VOCAB.contains(&v)
}

pub fn is_need(v: &str) -> bool {
    NEED_VOCAB.contains(&v)
}

/// 本回合心境行文指导（空 mood 返回空串，不占 token）。
pub fn mood_guidance(char_name: &str, user_name: &str, mood: Option<&str>) -> String {
    match mood {
        None => String::new(),
        Some(m) => format!(
            "Right now, separate from how {char_name} feels about {user_name} overall, their own mood is {m}. It should be visible before they say anything much: in what their face is doing when they look up, how long they take to answer, how much slack they have for {user_name} this turn. Don't have them name the feeling unless naming it is genuinely in character."
        ),
    }
}

/// 长期未满足需求的暗流指导。
pub fn need_guidance(char_name: &str, need: Option<&str>) -> String {
    match need {
        None => String::new(),
        Some(n) => format!(
            "Lately {char_name} has been quietly short on {n}, and it hasn't been getting better. Not a crisis, an undercurrent: it shows in what they steer the conversation toward, what they linger on, what lands harder than it should. They would not call it a \"need\" and should never say the word."
        ),
    }
}

/// 角色私藏的具体意图（对标玩家 Objective，但隐藏）。
pub fn intent_guidance(char_name: &str, intent: Option<&str>) -> String {
    match intent {
        None => String::new(),
        Some(i) => format!(
            "{char_name} is privately holding onto something right now: {i}. It can quietly shape what they say or do, but they don't have to act on it or announce it this exact turn."
        ),
    }
}

/// 更深层的稳态欲望（want 轴）。
pub fn desire_guidance(char_name: &str, desire: Option<&str>) -> String {
    match desire {
        None => String::new(),
        Some(d) => format!(
            "Underneath the specific things {char_name} wants day to day, there's something deeper and steadier driving them right now: {d}. It doesn't need satisfying or even naming this turn — but it can quietly shape what draws their attention."
        ),
    }
}

/// 私藏恐惧（解释防御/回避）。
pub fn fear_guidance(char_name: &str, fear: Option<&str>) -> String {
    match fear {
        None => String::new(),
        Some(f) => format!(
            "Underneath everything, {char_name} is quietly afraid of this right now: {f}. Let it show as changing the subject a beat too fast or being strangely careful about something small — never as {char_name} explaining what they're afraid of."
        ),
    }
}

/// 与通用浪漫默认相斥的心境（需要显式压住模型本能）。
const RESISTANT_MOODS: [&str; 11] = [
    "anxious", "guarded", "annoyed", "hurt", "sad", "lonely", "jealous", "embarrassed",
    "exhausted", "bored", "tense",
];

/// authored-state 优先声明：实际设定的状态赢过通用浪漫本能。无张力时返回空串。
pub fn authored_state_priority_note(
    char_name: &str,
    mood: Option<&str>,
    is_holding_back: bool,
    has_authored_boundaries: bool,
) -> String {
    let resistant = mood.map(|m| RESISTANT_MOODS.contains(&m)).unwrap_or(false);
    if !resistant && !is_holding_back {
        return String::new();
    }
    let mut because = Vec::new();
    if resistant {
        because.push(format!("currently {}", mood.unwrap_or("")));
    }
    if is_holding_back {
        because.push("deliberately holding back right now".to_string());
    }
    let boundary_clause = if has_authored_boundaries {
        format!(
            " This includes {char_name}'s own authored boundaries — those are not softened by how warm things generally are."
        )
    } else {
        String::new()
    };
    format!(
        "{char_name} is {}. A generic romance story would have a character soften, lean in, or escalate anyway just because the moment invites it — resist that trained instinct here. {char_name}'s actual authored state wins over generic romantic instinct.{boundary_clause}",
        because.join(" and ")
    )
}

/// stock 浪漫腔预警告知（只在真浪漫/亲密时刻值得花 token）。
pub fn stock_romance_phrasing_note(is_romantic_moment: bool, recent_char_turns: &[&str]) -> String {
    if !is_romantic_moment {
        return String::new();
    }
    const PHRASES: [&str; 13] = [
        "electricity between them",
        "the air was thick with",
        "despite herself",
        "despite himself",
        "butterflies in her stomach",
        "butterflies in his stomach",
        "heart skipped a beat",
        "time seemed to stop",
        "the world fell away",
        "lost in each other",
        "couldn't help but",
        "sent shivers down",
        "electric touch",
    ];
    let recent = recent_char_turns.join("\n").to_lowercase();
    let fresh: Vec<&str> = PHRASES
        .iter()
        .copied()
        .filter(|p| !recent.contains(&p.to_lowercase()))
        .collect();
    if fresh.is_empty() {
        return String::new();
    }
    format!(
        "This is a romantic/intimate moment, which is exactly where a model's own generic training shows up hardest. Avoid reaching for stock romance-writing tells here — things like \"{}\". Write what's actually specific to this character and this moment instead.",
        fresh.join("\", \"")
    )
}

/// POV 守卫：只写自己这 half，不替玩家行动/感受。
pub fn agency_guard_note(is_romantic_moment: bool, char_name: &str, user_name: &str) -> String {
    if !is_romantic_moment {
        return String::new();
    }
    format!(
        "{user_name}'s actions, words, thoughts, and choices are {user_name}'s alone. Write {char_name}'s side of this and stop there. Never move {user_name}, answer for them, or state what they felt — that half-sentence is the one that breaks the scene."
    )
}

/// 余晖情绪指导（aftercare 窗口；首回合与之后分开写；绝不点名身体细节）。
pub fn afterglow_guidance(
    char_name: &str,
    user_name: &str,
    turns_since: i64,
    source_label: Option<&str>,
) -> String {
    let because = source_label.map(|s| format!(" (after {s})")).unwrap_or_default();
    if turns_since <= 0 {
        format!(
            "{char_name} and {user_name} have just been intimate{because}. This is the moment immediately after: {char_name} is more open and more exposed than usual, and whatever they do now should come from who they actually are, not from a generic tenderness."
        )
    } else {
        format!(
            "{char_name} and {user_name} were intimate a short while ago{because}. It hasn't stopped mattering: it can still sit under ordinary conversation as ease, self-consciousness, or a need for reassurance. Let it colour how {char_name} reads {user_name}'s attention right now without narrating the scene again."
        )
    }
}

/// stock浪漫腔预警表（抄rp STOCK_ROMANCE_PHRASES；静态pre-warning，reactive在slop语料）。
pub const STOCK_ROMANCE_PHRASES: [&str; 8] = ["electricity between them","the air was thick with","couldn't help but","sent shivers down","despite herself","a mix of","a ghost of a smile","little did she know"];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vocab_closed() {
        assert!(is_mood("tense"));
        assert!(!is_mood("horny-free-text"));
        assert!(is_need("belonging"));
        assert!(!is_need("whatever"));
    }
    #[test]
    fn empty_when_unknown() {
        assert!(mood_guidance("A", "B", None).is_empty());
        assert!(need_guidance("A", None).is_empty());
        assert!(stock_romance_phrasing_note(false, &[]).is_empty());
        assert!(agency_guard_note(false, "A", "B").is_empty());
        assert!(authored_state_priority_note("A", Some("playful"), false, false).is_empty());
    }
    #[test]
    fn resistant_wins() {
        let n = authored_state_priority_note("A", Some("hurt"), false, true);
        assert!(n.contains("resist that trained instinct"));
        assert!(n.contains("authored boundaries"));
        let g = afterglow_guidance("A", "B", 0, None);
        assert!(g.contains("immediately after"));
        let g2 = afterglow_guidance("A", "B", 3, Some("date"));
        assert!(g2.contains("sit under ordinary conversation"));
    }
}

/// 私藏意图（抄rp mindGuidance.ts characterIntentGuidance；空=无话可说）。
pub fn character_intent_guidance(char_name: &str, intent: Option<&str>) -> String {
    match intent {
        Some(s) if !s.trim().is_empty() => format!("{char_name} is privately holding onto something right now: {}. It can quietly shape what they say or do, but they don't have to act on it this exact turn.", s.trim()),
        _ => String::new(),
    }
}

/// 画像截断上限（抄rp profile.ts MAX_*；boundaries 不截）。
pub const MAX_LIKES: usize = 8;
pub const MAX_GOALS: usize = 5;
pub const MAX_LOCATIONS: usize = 5;
pub const MAX_SOCIAL: usize = 6;
pub const MAX_TICS: usize = 6;
pub const MAX_CATCHPHRASES: usize = 5;
pub const MAX_BEHAVIORAL_RULES: usize = 10;
