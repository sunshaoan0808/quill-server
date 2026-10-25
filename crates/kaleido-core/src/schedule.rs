//! schedule.rs — 周计划日程 + presence + 日计划三菜单 + 主动 outreach 文本提示（吸收自 pnotisdev/rp world/calendar.ts + dayPlanner.ts + dating/outreach.ts 后半，MIT）。
//!
//! 纪律（抄rp）：
//! - 日计划三菜单固定 meet/text/rest；meet=text走hangout低赌注（无隐藏议程、无walkout）；
//!   rest不碰activeEvent、不调LLM。
//! - presence只给叙事参考，不做硬门控（忙/睡/旅行由opener prompt自己反应）。
//! - 世界时钟只手动推进；outreach不碰时钟。
//! - 全纯函数，零迁移。

/// 一天内的时段（phase index由世界卡定义数量，此处只做顺序比较）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct ScheduleSlot {
    pub day: i32,
    pub phase_index: i32,
    pub activity: String,
    pub location: Option<String>,
    /// sleeping/busy/traveling/available 等；sleeping会拦outreach。
    pub status: String,
}

pub fn current_activity<'a>(schedule: &'a [ScheduleSlot], day: i32, phase: i32) -> Option<&'a ScheduleSlot> {
    schedule.iter().find(|s| s.day == day && s.phase_index == phase)
}

pub fn is_sleeping(schedule: &[ScheduleSlot], day: i32, phase: i32) -> bool {
    current_activity(schedule, day, phase)
        .map(|s| s.status.eq_ignore_ascii_case("sleeping"))
        .unwrap_or(false)
}

/// 日计划三菜单（固定顺序meet/text/rest；能量耗尽则meet/text禁用）。
pub struct DayActivity {
    pub id: &'static str,
    pub label: String,
    pub kind: &'static str,
    pub location: Option<String>,
    pub disabled: bool,
    pub disabled_reason: Option<String>,
}

pub fn build_day_activities(
    char_name: &str,
    schedule: &[ScheduleSlot],
    day: i32,
    phase: i32,
    frequented: &[String],
    home: Option<&str>,
    energy_left: i32,
) -> [DayActivity; 3] {
    let out_of_energy = energy_left <= 0;
    let reason = out_of_energy.then(|| "No actions left today — rest to start a new day.".to_string());
    let location = current_activity(schedule, day, phase)
        .and_then(|s| s.location.clone())
        .or_else(|| frequented.first().cloned())
        .or_else(|| home.map(|h| h.to_string()));
    let meet_label = match &location {
        Some(l) => format!("Meet {char_name} at {l}"),
        None => format!("Meet {char_name}"),
    };
    [
        DayActivity {
            id: "meet",
            label: meet_label,
            kind: "hangout",
            location,
            disabled: out_of_energy,
            disabled_reason: reason.clone(),
        },
        DayActivity {
            id: "text",
            label: format!("Text {char_name}"),
            kind: "hangout",
            location: None,
            disabled: out_of_energy,
            disabled_reason: reason,
        },
        DayActivity {
            id: "rest",
            label: "Rest".into(),
            kind: "rest",
            location: None,
            disabled: false,
            disabled_reason: None,
        },
    ]
}

/// outreach文本提示（模型只写文本；资格门控在intent_full::outreach_eligible）。
pub fn outreach_reason_hint(reason: &str, char_name: &str, user_name: &str) -> String {
    match reason {
        "silence" => format!("You haven\u{2019}t heard from {user_name} in a while and decided to reach out first, unprompted — a short, casual check-in, the kind of thing you\u{2019}d actually text someone."),
        "schedule" => format!("Given what you\u{2019}re currently doing right now, you decided to text {user_name} first, unprompted — casual and brief, mentioning what\u{2019}s going on with you only if it comes up naturally."),
        "warmth" => format!("You\u{2019}ve been growing closer to {user_name} lately ({char_name}\u{2019}s own feeling, not a number), and you felt like reaching out first with something warm and personal."),
        _ => format!("Something in your day reminded you of {user_name}, so you decided to text first, unprompted — brief and natural."),
    }
}

// ---------- ambient事件钩子（吸收自rp world/ambientEvents.ts выбира） ----------
// 确定性“有事发生”钩子：节日>生日>生日临近/天气/缺席/目标/空闲，无新持久态。

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AmbientKind {
    Holiday,
    Birthday,
    BirthdaySoon,
    WeatherLoved,
    WeatherHated,
    RoutineAbsence,
    GoalOnMind,
    FreeTimeInterest,
}

pub struct AmbientEvent {
    pub kind: AmbientKind,
    pub detail: String,
    pub days: Option<i64>,
}

/// 生日还有几天（day-of-year环形；今天=0）。
pub fn days_until_annual(today: i64, birthday: i64) -> i64 {
    (birthday - today).rem_euclid(365)
}

/// ambient选择：holiday>birthday>其余候选按seed轮转（确定性，无随机）。
/// 入参全由调用方组装（worldId/day/phase/schedule/likes/goals/地点/天气偏好/生日）。
#[allow(clippy::too_many_arguments)]
pub fn select_ambient_event(
    holiday: Option<&str>,
    birthday: Option<i64>,
    today: i64,
    weather: Option<&str>,
    loves: &[String],
    hates: &[String],
    frequented: &[String],
    current_location: Option<&str>,
    goals: &[String],
    likes: &[String],
    status_available: bool,
    seed: u64,
) -> Option<AmbientEvent> {
    if let Some(h) = holiday {
        return Some(AmbientEvent { kind: AmbientKind::Holiday, detail: h.into(), days: None });
    }
    if let Some(b) = birthday {
        let until = days_until_annual(today, b);
        if until == 0 {
            return Some(AmbientEvent { kind: AmbientKind::Birthday, detail: String::new(), days: None });
        }
        if (1..=7).contains(&until) {
            return Some(AmbientEvent { kind: AmbientKind::BirthdaySoon, detail: String::new(), days: Some(until) });
        }
    }
    let mut cands: Vec<AmbientEvent> = vec![];
    if let Some(w) = weather {
        if loves.iter().any(|l| l == w) {
            cands.push(AmbientEvent { kind: AmbientKind::WeatherLoved, detail: w.into(), days: None });
        }
        if hates.iter().any(|h| h == w) {
            cands.push(AmbientEvent { kind: AmbientKind::WeatherHated, detail: w.into(), days: None });
        }
    }
    let eligible: Vec<&String> = frequented.iter().filter(|l| Some(l.as_str()) != current_location).collect();
    if !eligible.is_empty() {
        let loc = eligible[(seed as usize) % eligible.len()];
        cands.push(AmbientEvent {
            kind: AmbientKind::RoutineAbsence,
            detail: (*loc).clone(),
            days: Some(5 + (seed % 14) as i64),
        });
    }
    if !goals.is_empty() {
        cands.push(AmbientEvent {
            kind: AmbientKind::GoalOnMind,
            detail: goals[(seed as usize) % goals.len()].clone(),
            days: None,
        });
    }
    if status_available && !likes.is_empty() {
        cands.push(AmbientEvent {
            kind: AmbientKind::FreeTimeInterest,
            detail: likes[(seed as usize) % likes.len()].clone(),
            days: None,
        });
    }
    if cands.is_empty() {
        return None;
    }
    Some(cands.remove((seed as usize) % cands.len()))
}

pub fn describe_ambient_event(e: &AmbientEvent, char_name: &str) -> String {
    match e.kind {
        AmbientKind::Holiday => format!("Today is {} — that colors {char_name}\u{2019}s mood and what\u{2019}s going on around them.", e.detail),
        AmbientKind::Birthday => format!("Today is {char_name}\u{2019}s birthday — a real, once-a-year occasion."),
        AmbientKind::BirthdaySoon => format!("{char_name}\u{2019}s birthday is in {} day(s) — it\u{2019}s on their mind.", e.days.unwrap_or(0)),
        AmbientKind::WeatherLoved => format!("{char_name} loves this kind of weather ({}).", e.detail),
        AmbientKind::WeatherHated => format!("{char_name} hates this kind of weather ({}).", e.detail),
        AmbientKind::RoutineAbsence => format!("{char_name} hasn\u{2019}t been to {} in a while ({} days or so).", e.detail, e.days.unwrap_or(0)),
        AmbientKind::GoalOnMind => format!("{char_name} has been thinking about: {}.", e.detail),
        AmbientKind::FreeTimeInterest => format!("{char_name} has some free time and feels like: {}.", e.detail),
    }
}

// ---------- 立绘/背景解锁门控（吸收自rp vn/unlocks.ts） ----------

/// 当前可用的表情ids：无立绘=全默认槽；有立绘按affection门控；全锁=neutral保底。
pub fn unlocked_expression_ids(
    sprite_ids: &[String],
    unlocks: &std::collections::HashMap<String, f64>,
    default_ids: &[String],
    affection: f64,
) -> Vec<String> {
    if sprite_ids.is_empty() {
        return default_ids.to_vec();
    }
    let open: Vec<String> = sprite_ids
        .iter()
        .filter(|id| affection >= *unlocks.get(*id).unwrap_or(&0.0))
        .cloned()
        .collect();
    if open.is_empty() {
        return vec!["neutral".into()];
    }
    open
}

/// 群聊配角四型（抄rp RELATIONSHIP_ARCHETYPES；rival/found_family/mentor_mentee/power_imbalanced，非浪漫）。
pub const RELATIONSHIP_ARCHETYPES: [&str; 4] = ["rival","found_family","mentor_mentee","power_imbalanced"];
/// 原型关键词分类（抄rp classifyArchetype；优先级rival>found_family>mentor>power，大小写不敏感）。
pub fn classify_archetype(text: &str) -> Option<&'static str> {
    let t = text.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| t.contains(w));
    if has(&["rival","nemesis","competitor","frenemy"]) { return Some("rival"); }
    if has(&["sister","brother","sibling","aunt","uncle","cousin","like family","found family","chosen family","adopted"]) { return Some("found_family"); }
    if has(&["mentor","mentee","teacher","student","senpai","kohai","instructor","coach","apprentice","protege","protégé"]) { return Some("mentor_mentee"); }
    if has(&["boss","manager","supervisor","employer","employee","subordinate","superior officer","commanding officer","landlord","client"]) { return Some("power_imbalanced"); }
    None
}
/// 配角关系行文（抄rp participantRelationshipGuidance；自有warmth基线+原型行，不借主轨浪漫）。
pub fn participant_relationship_guidance(speaker: &str, persona: &str, other: &str, warmth: f64, archetype: Option<&str>) -> String {
    let reg = if warmth >= 70.0 { "a real, comfortable closeness has built up" } else if warmth >= 35.0 { "a friendly, still-developing familiarity" } else { "a fairly early, still-forming acquaintance" };
    let base = format!("{speaker} has their own independent footing with {persona} here, separate from {other}'s — right now that reads as {reg}.");
    match archetype {
        Some("rival") => format!("{base} {speaker} and {other} have a real rivalry — competitive edge, guarded respect, its own dynamic not a stand-in for romance."),
        Some("found_family") => format!("{base} {speaker} treats {other} like family — protective, warm, familial not romantic."),
        Some("mentor_mentee") => format!("{base} There's a teacher/student dynamic — guidance one way, deference with room to push back the other."),
        Some("power_imbalanced") => format!("{base} Genuine professional power gap — formality/caution/authority rather than peer equality."),
        _ => base,
    }
}
/// rival承诺框（抄rp rivalCommitmentFraming；dating可争，exclusive收敛，married/living只剩历史）。
pub fn rival_commitment_framing(status: &str) -> &'static str {
    match status { "dating" => "Nothing locked in yet — live rivalry, still competing, fair.", "exclusive" => "Exclusive now — jealousy fair, acting like field is open reads delusional.", "living_together"|"married" => "Settled commitment — rivalry reads as history/private feeling, not active contest.", _ => "" }
}
/// 名单组装（抄rp rosterFrom；主角置顶，配角按序）。
pub fn roster_from(primary: Option<(String, String)>, parts: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = vec![]; if let Some(p) = primary { out.push(p); } out.extend(parts.iter().cloned()); out
}
/// 轮转发言（抄rp nextRoundRobinSpeaker；mod防越界，空名单=None）。
pub fn next_round_robin_speaker(len: usize, index: i64) -> Option<(usize, usize)> {
    if len == 0 { return None; }
    let l = len as i64; let i = ((index % l + l) % l) as usize; Some((i, (i + 1) % len))
}
/// @点名（抄rp parseMention；长名优先+词边界，@Ari不误命中Aria）。
pub fn parse_mention<'a>(text: &str, names: &'a [String]) -> Option<&'a String> {
    let mut ord: Vec<&String> = names.iter().collect(); ord.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let low = text.to_lowercase();
    for n in ord { let pat = format!("@{}", n.to_lowercase()); if let Some(i) = low.find(&pat) { let after = low[pat.len()+i..].chars().next(); if after.map(|c| !c.is_alphanumeric() && c != '_').unwrap_or(true) { return Some(n); } } }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_menu_fixed_order_and_energy_gate() {
        let acts = build_day_activities("她", &[], 0, 0, &[], None, 0);
        assert_eq!([acts[0].id, acts[1].id, acts[2].id], ["meet", "text", "rest"]);
        assert!(acts[0].disabled && acts[1].disabled && !acts[2].disabled);
        let acts2 = build_day_activities("她", &[], 0, 0, &["咖啡馆".into()], None, 2);
        assert_eq!(acts2[0].label, "Meet 她 at 咖啡馆");
    }

    #[test]
    fn sleeping_blocks_but_presence_not_hard_gate() {
        let sch = vec![ScheduleSlot {
            day: 0,
            phase_index: 1,
            activity: "睡觉".into(),
            location: None,
            status: "sleeping".into(),
        }];
        assert!(is_sleeping(&sch, 0, 1));
        assert!(!is_sleeping(&sch, 0, 0));
    }

    #[test]
    fn ambient_holiday_beats_birthday() {
        let e = select_ambient_event(Some("除夕"), Some(100), 100, None, &[], &[], &[], None, &[], &[], true, 1).unwrap();
        assert_eq!(e.kind, AmbientKind::Holiday);
        let e2 = select_ambient_event(None, Some(100), 100, None, &[], &[], &[], None, &[], &[], true, 1).unwrap();
        assert_eq!(e2.kind, AmbientKind::Birthday);
        let e3 = select_ambient_event(None, Some(103), 100, None, &[], &[], &[], None, &[], &[], true, 1).unwrap();
        assert_eq!((e3.kind, e3.days), (AmbientKind::BirthdaySoon, Some(3)));
        assert_eq!(days_until_annual(364, 1), 2);
    }

    #[test]
    fn unlock_gates_neutral_fallback() {
        let d: Vec<String> = vec![];
        let u = std::collections::HashMap::new();
        let got = unlocked_expression_ids(&d, &u, &["happy".into()], 0.0);
        assert_eq!(got, vec!["happy".to_string()]);
        let got2 = unlocked_expression_ids(&["happy".into()], &u, &["happy".into()], 0.0);
        assert_eq!(got2, vec!["happy".to_string()]);
        let mut u2 = std::collections::HashMap::new();
        u2.insert("happy".into(), 80.0);
        let got3 = unlocked_expression_ids(&["happy".into()], &u2, &["happy".into()], 10.0);
        assert_eq!(got3, vec!["neutral".to_string()]);
    }
}

/// 当前可用背景ids（抄rp getUnlockedBackgroundIds：无背景=空；全锁=空由调用方兜默认）。
pub fn unlocked_background_ids(
    bg_ids: &[String],
    unlocks: &std::collections::HashMap<String, f64>,
    affection: f64,
) -> Vec<String> {
    if bg_ids.is_empty() { return vec![]; }
    bg_ids.iter().filter(|id| affection >= *unlocks.get(*id).unwrap_or(&0.0)).cloned().collect()
}
