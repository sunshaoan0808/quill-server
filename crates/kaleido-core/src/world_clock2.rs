//! world_clock2.rs — 世界时钟/日历/天气/日程现状/世界模板（吸收自 pnotisdev/rp world/calendar.ts + worldTemplates.ts，MIT）。
//! 全纯函数：day number + seed 定一切；只存 currentDay/currentPhaseIndex。

/// 四季×28天=112天一年；早中晚夜四段；周一开季。
pub const DAYS_PER_SEASON: i64 = 28;
pub const DAYS_PER_YEAR: i64 = 112;

pub fn season_of(day: i64) -> &'static str {
    let w = ((day % DAYS_PER_YEAR) + DAYS_PER_YEAR) % DAYS_PER_YEAR;
    match w / DAYS_PER_SEASON {
        0 => "spring",
        1 => "summer",
        2 => "autumn",
        _ => "winter",
    }
}

pub fn day_of_season(day: i64) -> i64 {
    let w = ((day % DAYS_PER_YEAR) + DAYS_PER_YEAR) % DAYS_PER_YEAR;
    (w % DAYS_PER_SEASON) + 1
}

pub fn weekday_of(day: i64) -> &'static str {
    const WD: [&str; 7] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
    WD[((day_of_season(day) - 1) % 7) as usize]
}

/// 每季第14天固定节日。
pub fn holiday_of(day: i64) -> Option<&'static str> {
    if day_of_season(day) != 14 {
        return None;
    }
    Some(match season_of(day) {
        "spring" => "First Bloom",
        "summer" => "Midsummer Night",
        "autumn" => "Lantern Festival",
        _ => "Long Night",
    })
}

/// 环形倒数（生日/纪念日/节日通用；今天=0）。
pub fn days_until_annual_112(today: i64, target_day_of_year: i64) -> i64 {
    let t = ((today % DAYS_PER_YEAR) + DAYS_PER_YEAR) % DAYS_PER_YEAR;
    (target_day_of_year - t + DAYS_PER_YEAR) % DAYS_PER_YEAR
}

/// FNV-1a 确定性小数 [0,1)。
pub fn seeded_fraction(seed: &str) -> f64 {
    let mut h: u32 = 2166136261;
    for b in seed.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h % 100000) as f64 / 100000.0
}

pub fn pick_from<'a, T>(options: &'a [T], seed: &str) -> Option<&'a T> {
    if options.is_empty() {
        return None;
    }
    let i = (seeded_fraction(seed) * options.len() as f64) as usize;
    Some(&options[i.min(options.len() - 1)])
}

/// 按季天气（重复=加权）。
pub fn weather_of(world_id: &str, day: i64) -> &'static str {
    let pool: &[&str] = match season_of(day) {
        "spring" => &["clear", "rain", "rain", "overcast", "wind"],
        "summer" => &["clear", "clear", "clear", "storm", "overcast"],
        "autumn" => &["clear", "wind", "rain", "fog", "overcast"],
        _ => &["clear", "clear", "snow", "storm", "fog"],
    };
    let w = ((day % DAYS_PER_YEAR) + DAYS_PER_YEAR) % DAYS_PER_YEAR;
    *pick_from(pool, &format!("weather:{world_id}:{w}")).unwrap()
}

/// 当日行动池：周末4平日3；剩余=上限-phase（兜底0）。
pub fn max_energy(day: i64) -> i64 {
    match weekday_of(day) {
        "saturday" | "sunday" => 4,
        _ => 3,
    }
}

pub fn energy_remaining(day: i64, phase_index: i64) -> i64 {
    (max_energy(day) - phase_index).max(0)
}

/// 花1行动：phase+1，夜后进明日晨；用完当日最后一动直接睡到次日晨。
pub fn spend_energy(day: i64, phase: i64) -> (i64, i64, bool) {
    let (mut d, mut p) = (day, phase + 1);
    if p >= 4 {
        d += 1;
        p = 0;
        return (d, p, true);
    }
    if energy_remaining(d, p) > 0 {
        (d, p, false)
    } else {
        (d + 1, 0, true)
    }
}

/// 世界模板三件套 + 退役别名（slice_of_life→freeform，免迁移）。
pub fn normalize_template(id: &str) -> &str {
    match id {
        "freeform" | "visual_novel" | "dating_sim" => id,
        "slice_of_life" => "freeform",
        _ => "dating_sim",
    }
}

/// 每日心情（确定性：同角色同天同心情，只染语气不决定剧情）。
pub fn mood_of_day(character_id: &str, day: i64) -> &'static str {
    const MOODS: [&str; 8] = [
        "upbeat", "tired", "a little anxious", "content",
        "irritable", "wistful", "restless", "unusually cheerful",
    ];
    let w = ((day % DAYS_PER_YEAR) + DAYS_PER_YEAR) % DAYS_PER_YEAR;
    *pick_from(&MOODS, &format!("mood:{character_id}:{w}")).unwrap()
}

/// 天气文案（抄rp WEATHER_DESCRIPTIONS；纯展示）。
pub fn describe_weather(kind: &str) -> &'static str {
    match kind {
        "clear" => "clear and mild",
        "rain" => "raining steadily",
        "storm" => "stormy",
        "overcast" => "gray and overcast",
        "snow" => "snowing",
        "wind" => "blustery and windy",
        "fog" => "thick with fog",
        _ => "clear and mild",
    }
}

/// 全年节日清单（抄rp ALL_HOLIDAYS：每季第14天；day-of-year 0-111）。
pub fn all_holidays_112() -> [(&'static str, i64); 4] {
    [("First Bloom", 13), ("Midsummer Night", 41), ("Lantern Festival", 69), ("Long Night", 97)]
}

/// 夜灯判定：evening/night算夜，其余算昼；越界默认昼。
pub fn is_night_phase(phase: &str) -> bool {
    matches!(phase, "evening" | "night")
}

/// UI音效事件契约（吸收自rp audio/sfx.ts：后端只发事件，前端WebAudio合成，无音频文件）。
/// 前端收到后：send→720Hz 0.08s blip；reward→660+880双音chime；失败/无AudioContext静默跳过。
pub fn sfx_event_for(kind: &str) -> Option<&'static str> {
    match kind {
        "send" => Some("sfx:send"),
        "reward" => Some("sfx:reward"),
        _ => None,
    }
}

/// 模板隐藏的世界编辑器tab。
pub fn hidden_tabs(template: &str) -> Vec<&'static str> {
    match normalize_template(template) {
        "freeform" => vec!["dating", "clock"],
        "visual_novel" => vec!["dating"],
        _ => vec![],
    }
}

/// 世界模板三型（抄rp worldTemplates；freeform/visual_novel/dating_sim）。
pub fn normalize_world_template(id: Option<&str>) -> &'static str {
    match id.unwrap_or("dating_sim") {
        "freeform"|"slice_of_life" => "freeform",
        "visual_novel" => "visual_novel",
        _ => "dating_sim",
    }
}
/// 模板隐藏tab（抄rp HIDDEN_TABS；freeform藏dating+clock）。
pub fn hidden_world_tabs(template: &str) -> &'static [&'static str] {
    match normalize_world_template(Some(template)) {
        "freeform" => &["dating","clock"],
        "visual_novel" => &["dating"],
        _ => &[],
    }
}
/// 模板默认关关系辅助（抄rp disablesRelationshipAssists；仅freeform默认关）。
pub fn template_disables_assists(template: &str) -> bool {
    normalize_world_template(Some(template)) == "freeform"
}

/// 时钟推一格（抄rp advancePhase；night后进次日早晨）。
pub fn advance_phase_112(day: i64, phase_index: i64) -> (i64, i64) {
    if phase_index + 1 >= 4 { (day+1, 0) } else { (day, phase_index+1) }
}

/// 动作实际落点（抄rp activityPhase；spend跨天则活动发生在原phase，不是次日晨）。
pub fn activity_phase_112(day: i64, phase_index: i64) -> (i64, i64) {
    let (d, p) = advance_phase_112(day, phase_index);
    if d != day { (day, phase_index) } else { (d, p) }
}
/// 此刻白话（抄rp describeWorldMoment；phase+季+周+节日+天气+心情一句）。
pub fn describe_world_moment(day: i64, phase_index: i64, weather: &str, weather_mark: &str, mood: &str) -> String {
    const PH: [&str; 4] = ["morning","afternoon","evening","night"];
    let ph = PH[phase_index.clamp(0,3) as usize];
    let hol = match holiday_of(day) { Some(h) => format!(" — today is {h}"), None => String::new() };
    format!("It's {ph} on a {} {}{hol}. The weather is {}{weather_mark}. Mood today: {mood}.", season_of(day), weekday_of(day), describe_weather(weather))
}
/// 当前行程（抄rp getCurrentActivity；day-specific优先于everyday，无匹配回available）。
pub fn get_current_activity<'a>(entries: &'a [(String, Vec<String>, String, String, String, Option<String>)], weekday: &str, phase: &str) -> &'a (String, Vec<String>, String, String, String, Option<String>) {
    entries.iter().find(|(_, days, p, _, _, _)| *p == phase && days.iter().any(|d| d == weekday))
        .or_else(|| entries.iter().find(|(_, days, p, _, _, _)| *p == phase && days.is_empty())).unwrap_or(&entries[0])
}
/// 叙事时间锚（抄rp detectNarratedPhase；ordered first-match，早现先赢；收关键词简化版）。
pub fn detect_narrated_phase(text: &str) -> Option<&'static str> {
    let t = text.to_lowercase();
    let hits: &[(&str, &str)] = &[("next morning","morning"),("that night","night"),("that evening","evening"),("next day","morning"),("dawn","morning"),("sunrise","morning"),("morning","morning"),("lunch","afternoon"),("noon","afternoon"),("afternoon","afternoon"),("after school","afternoon"),("after work","afternoon"),("dusk","evening"),("sunset","evening"),("evening","evening"),("midnight","night"),("night","night")];
    let mut best: Option<(usize, &str)> = None;
    for (k, p) in hits { if let Some(i) = t.find(k) { if best.map(|(bi,_)| i < bi).unwrap_or(true) { best = Some((i, p)); } } }
    best.map(|(_, p)| p)
}
/// 在场调和（抄rp resolveScheduledPresence；场景地点优先，时钟只保留不打架的status/activity）。
pub fn resolve_scheduled_presence(clock_status: &str, clock_loc: Option<&str>, scene_loc: Option<&str>, alt_available_loc: Option<&str>) -> (String, bool) {
    let loc = match scene_loc { Some(s) if !s.trim().is_empty() => s.trim(), _ => return (clock_status.to_string(), true) };
    let overlap = |a: &str, b: &str| { let (x, y) = (a.trim().to_lowercase(), b.trim().to_lowercase()); !x.is_empty() && !y.is_empty() && (x == y || x.contains(&y) || y.contains(&x)) };
    if let Some(c) = clock_loc { if overlap(c, loc) { return (clock_status.to_string(), true); } }
    if let Some(a) = alt_available_loc { if overlap(a, loc) { return ("available".to_string(), true); } }
    (clock_status.to_string(), false)
}
/// 在场标签（抄rp PRESENCE_LABELS；available=free其余直译）。
pub fn presence_label(status: &str) -> &'static str {
    match status { "busy" => "busy", "sleeping" => "asleep", "traveling" => "traveling", _ => "free" }
}
/// 在场白话（抄rp describePresence；有activity拼破折号，否则一句）。
pub fn describe_presence(status: &str, activity: Option<&str>, location: Option<&str>) -> String {
    let lab = presence_label(status);
    match activity { Some(a) => format!("currently {lab} — {a}{}", location.map(|l| format!(" at {l}")).unwrap_or_default()), None => format!("currently {lab}.") }
}
/// 环境钩子八种（抄rp AMBIENT_EVENT_KINDS；holiday/birthday/weather×2/routine/goal/free）。
pub const AMBIENT_EVENT_KINDS: [&str; 8] = ["holiday","birthday","birthday_soon","weather_loved","weather_hated","routine_absence","goal_on_mind","free_time_interest"];
/// 环境钩子精选（抄rp selectAmbientEvent；holiday>birthday置顶，其余seeded uniform一抽）。
pub fn select_ambient_event(holiday: Option<&str>, is_birthday: bool, candidates: &[String], seed: &str) -> Option<String> {
    if let Some(h) = holiday { return Some(format!("holiday:{h}")); }
    if is_birthday { return Some("birthday".to_string()); }
    if candidates.is_empty() { return None; }
    pick_from(candidates, seed).cloned()
}
/// 钩子行文（抄rp describeAmbientEvent；八种各一句，实名直插）。
pub fn describe_ambient_event(kind: &str, detail: &str, char_name: &str) -> String {
    match kind {
        "holiday" => format!("Today is {detail} — fair for {char_name} to notice or feel some way about today."),
        "birthday" => format!("Today is {char_name}'s actual birthday — fair for them to notice or bring it up today."),
        "birthday_soon" => format!("{char_name}'s birthday is coming up in about {detail} days — fair to mention or hint."),
        "weather_loved" => format!("Today's weather ({detail}) is exactly the kind {char_name} loves — worth noticing."),
        "weather_hated" => format!("Today's weather ({detail}) is exactly the kind {char_name} dislikes — can color their mood a little."),
        "routine_absence" => format!("{char_name} hasn't been to {detail} in a while — could plausibly cross their mind today."),
        "goal_on_mind" => format!("Something {char_name} has been working toward — {detail} — is weighing on them today."),
        _ => format!("{char_name} has no obligation right now, and {detail} is something they'd enjoy."),
    }
}
/// 环境钩子门（抄rp ambientEventGuidance；4回合起步+15%概率，miss回空）。
pub fn ambient_event_guidance(has_event: bool, char_turns: i64, seed: &str) -> bool {
    has_event && char_turns >= 4 && seeded_fraction(seed) < 0.15
}
/// 场外反应人选（抄rp selectSocialReaction；seeded pick，无人选=None）。
pub fn select_social_reaction<'a>(connections: &'a [String], seed: &str) -> Option<&'a String> {
    if connections.is_empty() { None } else { pick_from(connections, seed) }
}
/// 日程冲突行文（抄rp scheduleConflictGuidance；available回空，其余三档诚实成本）。
pub fn schedule_conflict_guidance(char_name: &str, status: &str, activity: Option<&str>, location: Option<&str>) -> String {
    if status == "available" { return String::new(); }
    let a = activity.map(|x| format!(" ({x})")).unwrap_or_default();
    let l = location.map(|x| format!(" at {x}")).unwrap_or_default();
    match status {
        "sleeping" => format!("{char_name} is supposed to be asleep right now — being woken is a real disruption, not free."),
        "traveling" => format!("{char_name} is currently traveling{l}, not settled somewhere a deep scene can start."),
        _ => format!("{char_name} is genuinely supposed to be busy right now{a}{l} — going along reads as a real, noticed cost."),
    }
}
/// 场外反应存档行（抄rp describeSocialReaction；转述口吻，不进场）。
pub fn describe_social_reaction(char_name: &str, conn_name: &str, relation: &str, topic: &str) -> String {
    format!("{conn_name} ({char_name}'s {relation}) heard about {topic} and had something to say — {char_name} can bring this up later in their own words.")
}
/// 日计划三件套（抄rp buildDayPlannerActivities；meet/text/hangout低赌注，rest不进场景；无行动meet/text禁用）。
pub fn day_planner_menu(out_of_energy: bool, meet_label: &str) -> Vec<(String, String, String, bool)> {
    vec![("meet".to_string(), meet_label.to_string(), "hangout".to_string(), out_of_energy), ("text".to_string(), "Text".to_string(), "hangout".to_string(), out_of_energy), ("rest".to_string(), "Rest".to_string(), "rest".to_string(), false)]
}
/// 日计划卡（抄rp dateEventCardForActivity；恒hangout，planner不开date门）。
pub fn date_card_for_activity(activity_id: &str, char_name: &str, location: Option<&str>) -> (String, String) {
    if activity_id == "meet" {
        let d = match location { Some(l) => format!("{char_name} spends this part of the day with you at {l}."), None => format!("{char_name} spends this part of the day with you.") };
        (format!("Spend time with {char_name}"), d)
    } else { (format!("Check in with {char_name}"), format!("A quick text exchange with {char_name}.")) }
}
/// 世界模板三型（抄rp WORLD_TEMPLATES；freeform/visual_novel/dating_sim，freeform默认关关系辅助）。
pub fn world_template_ids() -> [&'static str; 3] { ["freeform", "visual_novel", "dating_sim"] }
pub fn normalize_world_template_id(id: Option<&str>) -> &'static str {
    match id.unwrap_or("dating_sim") { "freeform"|"slice_of_life" => "freeform", "visual_novel" => "visual_novel", _ => "dating_sim" }
}
pub fn world_template_disables_assists(template: &str) -> bool { normalize_world_template_id(Some(template)) == "freeform" }
pub fn assist_overrides_for_template(template: &str) -> (bool, Option<&'static str>, Option<&'static str>) {
    match normalize_world_template_id(Some(template)) {
        "freeform" => (true, None, Some("balanced")),
        "visual_novel" => (false, Some("auto"), None),
        _ => (false, None, None),
    }
}

/// 日历全量（抄rp getCalendarInfo；112天环形，季首周一，季中节日）。
pub fn get_calendar_info_112(day: i64) -> (i64, &'static str, i64, &'static str, Option<&'static str>) {
    let w = ((day % DAYS_PER_YEAR) + DAYS_PER_YEAR) % DAYS_PER_YEAR;
    (w, season_of(day), day_of_season(day), weekday_of(day), holiday_of(day))
}
/// 最大行动数别名（抄rp getMaxEnergyForDay；周末4平日3）。
pub fn get_max_energy_for_day(day: i64) -> i64 { max_energy(day) }
/// 剩余行动别名（抄rp getEnergyRemaining；floor0）。
pub fn get_energy_remaining(day: i64, phase_index: i64) -> i64 { energy_remaining(day, phase_index) }
/// 模板查表（抄rp getWorldTemplate；unknown回dating_sim）。
pub fn get_world_template(id: Option<&str>) -> &'static str { normalize_world_template_id(id) }
/// 事实合成lorebook预算（抄rp FACTS_TOKEN_BUDGET=200；约15-20条短事实）。
pub const FACTS_TOKEN_BUDGET: usize = 200;
/// 事实行文（抄rp factContent；未 resolved 负面 Valence≤-0.15 强标，其余 open thread）。
pub fn fact_content(text: &str, unresolved: bool, valence: f64) -> String {
    if !unresolved { return text.to_string(); }
    if valence <= -0.15 { format!("Still unsettled, not resolved: {text}") } else { format!("Still an open thread: {text}") }
}
/// 事实打分（抄rp factScore；0.45重要+0.3新近+0.45未决，open thread不因老被挤掉）。
pub fn fact_score(importance: f64, recency_rank: f64, unresolved: bool) -> f64 { 0.45 * importance + 0.3 * recency_rank + if unresolved { 0.45 } else { 0.0 } }
/// 章节数解析（抄rp requestedChapterCount；1-20合法，其余None）。
pub fn requested_chapter_count(text: &str) -> Option<i64> {
    let low = text.to_lowercase();
    for pat in ["chapters","chapter"] {
        if let Some(i) = low.find(pat) {
            let before = low[..i].trim(); let num: String = before.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<String>().chars().rev().collect();
            if let Ok(n) = num.parse::<i64>() { if (1..=20).contains(&n) { return Some(n); } }
        }
    }
    None
}
pub const DEFAULT_CHAPTER_COUNT: i64 = 6;
/// 线程标题（抄rp threadTitleFrom；首句/首行60字，空回New conversation）。
pub fn thread_title_from(text: &str) -> String {
    let line = text.trim().lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let sent = line.split(|c| ".!?".contains(c)).next().unwrap_or(line).trim();
    let c: String = sent.chars().take(60).collect();
    if c.trim().is_empty() { "New conversation".to_string() } else { c }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_round() {
        assert_eq!(season_of(0), "spring");
        assert_eq!(weekday_of(0), "monday");
        assert_eq!(holiday_of(13), Some("First Bloom"));
        assert_eq!(days_until_annual_112(0, 0), 0);
        assert_eq!(max_energy(5), 4);
        assert_eq!(energy_remaining(0, 0), 3);
        let (d, p, slept) = spend_energy(0, 3);
        assert_eq!((d, p, slept), (1, 0, true));
    }

    #[test]
    fn weather_deterministic() {
        assert_eq!(weather_of("w", 5), weather_of("w", 5));
        assert_eq!(normalize_template("slice_of_life"), "freeform");
        assert_eq!(hidden_tabs("freeform"), vec!["dating", "clock"]);
        assert_eq!(mood_of_day("c", 3), mood_of_day("c", 3));
        assert!(is_night_phase("evening") && !is_night_phase("morning"));
        assert_eq!(sfx_event_for("send"), Some("sfx:send"));
        assert_eq!(sfx_event_for("nope"), None);
    }
}
