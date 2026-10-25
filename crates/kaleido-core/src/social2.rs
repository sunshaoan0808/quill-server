//! social2.rs — 回绝窗口/场景纠偏/接触图/剧本选择/变体/就绪门（吸收自 pnotisdev/rp dating+vn 零散模块，MIT）。
//! 全纯函数，零迁移。

/// 被拒余温5回合（角色自己回复数计）。
pub const REBUFF_WINDOW_TURNS: i64 = 5;

pub fn is_rebuff_active(started_at_turn: i64, char_replies: i64) -> bool {
    let since = char_replies - started_at_turn;
    since >= 0 && since < REBUFF_WINDOW_TURNS
}

/// 被拒行文（抄rp rebuffGuidance；deflect软/backfire重）。
pub fn rebuff_guidance(char_name: &str, user_name: &str, kind: &str, severity: &str) -> String {
    let ask = if kind == "commitment" {
        "define where things actually stood"
    } else {
        "take things further, physically"
    };
    if severity == "backfire" {
        format!("Not long ago {user_name} pushed to {ask} and it genuinely stung {char_name} — bad timing, or it read as presumptuous. That doesn't heal in one warm line: {char_name} can be slower to open back up or a little more guarded than usual for a while, without it becoming a punishment that never lifts.")
    } else {
        format!("{user_name} recently asked to {ask} and {char_name} put it off. That isn't forgotten the instant the subject changes: a touch more self-protection or hesitation than usual is natural for a little while, even while things otherwise carry on normally.")
    }
}

/// 硬轨命中纠偏（抄rp hardFailCorrectionDirective；全无返回None）。
pub fn hard_fail_directive(
    char_name: &str,
    user_name: &str,
    crossed: Option<&str>,
    agency_violation: Option<&str>,
    continuity_break: Option<&str>,
) -> Option<String> {
    if crossed.is_none() && agency_violation.is_none() && continuity_break.is_none() {
        return None;
    }
    let mut reasons = vec![];
    if let Some(c) = crossed {
        reasons.push(format!("crossed a stated limit (\"{c}\")"));
    }
    if let Some(a) = agency_violation {
        reasons.push(format!("narrated {user_name}'s own action, feeling, or thought for them (\"{a}\")"));
    }
    if let Some(b) = continuity_break {
        reasons.push(format!("contradicted what is already true in this scene: {b}"));
    }
    let joined = reasons.join(" and it ");
    Some(build_steer_directive(
        &format!("It {joined}. Don't do that this time. Write only {char_name}'s own side, and stay consistent with the scene state you were given."),
        char_name,
    ))
}

/// 一次性纠偏指令：覆盖上次失败方向，不解释不走回头路。
pub fn build_steer_directive(steer_text: &str, char_name: &str) -> String {
    format!("Before writing this reply again: the previous attempt went the wrong way, and the player has stepped in to correct it. This overrides any instinct to continue what the last attempt was doing. Write this turn as if that attempt had never happened — don\u{2019}t carry its direction forward, don\u{2019}t have {char_name} walk it back on the page, and don\u{2019}t acknowledge the correction inside the fiction. Just write the turn it should have been. The correction: {}", steer_text.trim())
}

/// 接触边上限12条（多了是prompt噪音）。
pub const MAX_CONTACT_EDGES: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContactEdge {
    pub actor: String,
    pub target: String,
    pub region: String,
    pub since_turn: i64,
}

/// 有效接触校验：双方非空、非自摸、部位在闭合词表、去重。
pub fn parse_observed_contact(raw: &[(String, String, String)], valid_regions: &[&str]) -> Vec<(String, String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = vec![];
    for (a, t, r) in raw {
        if a.trim().is_empty() || t.trim().is_empty() || a == t {
            continue;
        }
        if !valid_regions.contains(&r.as_str()) {
            continue;
        }
        let k = format!("{a}>{t}:{r}");
        if seen.insert(k) {
            out.push((a.clone(), t.clone(), r.clone()));
        }
    }
    out
}

/// 接触推进：已在的保持since_turn；空观察不清空；超限切尾。
pub fn advance_contact(prev: Vec<ContactEdge>, observed: Vec<(String, String, String)>, now_turn: i64) -> Vec<ContactEdge> {
    if observed.is_empty() {
        return prev;
    }
    let held: std::collections::HashMap<String, i64> =
        prev.iter().map(|e| (format!("{}>{}:{}", e.actor, e.target, e.region), e.since_turn)).collect();
    let mut next: Vec<ContactEdge> = observed
        .into_iter()
        .map(|(actor, target, region)| {
            let k = format!("{actor}>{target}:{region}");
            ContactEdge { actor, target, region, since_turn: held.get(&k).copied().unwrap_or(now_turn) }
        })
        .collect();
    if next.len() > MAX_CONTACT_EDGES {
        next.drain(..next.len() - MAX_CONTACT_EDGES);
    }
    next
}

/// 剧本选择：offered内按（eligibility数+内容档要求）最具体者胜；保底默认剧本。
pub fn select_scenario<'a>(offered_specificity: &'a [(String, usize)], default_id: &'a str) -> &'a str {
    offered_specificity.iter().max_by_key(|(_, s)| *s).map(|(id, _)| id.as_str()).unwrap_or(default_id)
}

/// 确定性变体（同seed同index，防重渲染闪烁）。
pub fn pick_variant<T: Clone>(candidates: &[T], seed: &str) -> Option<T> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0].clone());
    }
    let mut h: i32 = 0;
    for c in seed.chars() {
        h = h.wrapping_mul(31).wrapping_add(c as i32);
    }
    Some(candidates[(h.abs() as usize) % candidates.len()].clone())
}

/// VN就绪：有立绘+有世界+世界有背景，缺一不可。
pub fn is_vn_ready(has_sprites: bool, has_world: bool, world_has_backgrounds: bool) -> bool {
    has_sprites && has_world && world_has_backgrounds
}

/// VN 缺美术提示（抄rp vnArtHint 顺序：先立绘→再世界→再背景；被 dismiss 的角色直接 None）。
/// 返回 Some(提示) / None(美术齐了或被 dismiss)。
pub fn vn_art_hint(
    char_name: &str,
    has_sprites: bool,
    has_world: bool,
    world_name: &str,
    world_has_backgrounds: bool,
    dismissed: bool,
) -> Option<String> {
    if dismissed {
        return None;
    }
    let name = if char_name.trim().is_empty() { "this character" } else { char_name.trim() };
    if !has_sprites {
        return Some(format!(
            "No expression sprites for {name} yet — add art in the character editor's Visual novel tab and it will show here."
        ));
    }
    if !has_world {
        return Some(format!(
            "{name} isn't bound to a world, so scenes have no background. Assign one from the character editor's Identity tab."
        ));
    }
    if !world_has_backgrounds {
        let w = if world_name.trim().is_empty() { "This world" } else { world_name.trim() };
        return Some(format!(
            "{w} has no scene backgrounds — add them in the world editor to replace this placeholder."
        ));
    }
    None
}

/// 选项 kind 封闭三态（抄rp choices.ts）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceKind {
    Line,
    Action,
    Gift,
}

/// 选项校验：kind 非法→line；空 label/text→drop；gift 无 gift_id→drop；gift 最多 1 个。
/// 输入 (kind, label, text, gift_id)；返回 (有效 kind, label, text, gift_id)。
pub fn validate_choice(
    kind: &str,
    label: &str,
    text: &str,
    gift_id: Option<&str>,
    gift_used: bool,
) -> Option<(ChoiceKind, String, String, Option<String>)> {
    let k = match kind {
        "action" => ChoiceKind::Action,
        "gift" => ChoiceKind::Gift,
        _ => ChoiceKind::Line,
    };
    if k == ChoiceKind::Gift {
        if gift_used {
            return None;
        }
        let gid = gift_id.map(|s| s.trim()).filter(|s| !s.is_empty())?;
        let l = label.trim();
        let t = text.trim();
        if l.is_empty() || t.is_empty() {
            return None;
        }
        return Some((k, l.to_string(), t.to_string(), Some(gid.to_string())));
    }
    let l = label.trim();
    let t = text.trim();
    if l.is_empty() || t.is_empty() {
        return None;
    }
    Some((k, l.to_string(), t.to_string(), None))
}

// ---------- 多人场景 roster（吸收自 pnotisdev/rp dating/sceneParticipants.ts，MIT） ----------

/// 场景全体（owner 排第一；空 roster=老存档单人）。
pub fn scene_participants<'a>(roster: &'a [String]) -> &'a [String] {
    roster
}

/// 状态是否走场景原单人字段：owner 或无 roster 老存档（仅一参与人）。
pub fn is_scene_owner(roster: &[String], char_id: &str) -> bool {
    roster.is_empty() || roster.first().map(|s| s.as_str()) == Some(char_id)
}

/// 该角色在不在场：有 roster 点名；无 roster=owner 本人（抄rp isSceneParticipant）。
pub fn is_scene_participant(roster: &[String], char_id: &str, owner_id: &str) -> bool {
    if roster.is_empty() {
        return owner_id == char_id;
    }
    roster.iter().any(|c| c == char_id)
}

/// 是否多人（>1 才配 arousal/clothing 各人 map）。
pub fn is_multi_participant_scene(roster: &[String]) -> bool {
    roster.len() > 1
}

/// 入场：owner 恒第一，去重；空 roster 先补 owner（抄rp withParticipant）。
pub fn with_participant(mut roster: Vec<String>, char_id: &str, owner_id: &str) -> Vec<String> {
    if roster.is_empty() {
        roster.push(owner_id.to_string());
    }
    if !roster.iter().any(|c| c == char_id) {
        roster.push(char_id.to_string());
    }
    roster
}

/// 某人被触部位（contact 边里 target==who 的 region 去重；抄rp contactRegionsFor）。
pub fn contact_regions_for(contact: &[ContactEdge], who: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for e in contact {
        if e.target == who && !out.iter().any(|r| r == &e.region) {
            out.push(e.region.clone());
        }
    }
    out
}

/// 主语读法：player→You，否则点名（抄rp subjectOf）。
pub fn subject_of(who: &str, player_id: &str, name_of: &str) -> String {
    if who == player_id {
        "You".to_string()
    } else {
        name_of.to_string()
    }
}

/// 身体部位归属读法：player→your，否则 `名's`（抄rp possessiveOf）。
pub fn possessive_of(who: &str, player_id: &str, name_of: &str) -> String {
    if who == player_id {
        "your".to_string()
    } else {
        format!("{name_of}'s")
    }
}

// ---------- 群聊导演/rival 小件（吸收自 pnotisdev/rp chat/scene.ts + chat/participantArchetype.ts，MIT） ----------

/// director 点名 prompt 建造（抄rp pickDirectorSpeaker 前半；近 6 条有字才进）。
/// 输入 (名表, 近况行, 地点, user)；单人直接首位（调用方短路）。
pub fn build_director_prompt(names: &[String], recent_lines: &[String], scene_location: Option<&str>, user_name: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "You are directing a group scene with these characters present: {}.",
        names.join(", ")
    ));
    if let Some(loc) = scene_location {
        if !loc.trim().is_empty() {
            parts.push(format!("Setting: {}", loc.trim()));
        }
    }
    parts.push("Recent conversation:".to_string());
    parts.push(if recent_lines.is_empty() {
        "(nothing said yet)".to_string()
    } else {
        recent_lines.join("\n")
    });
    parts.push(String::new());
    parts.push(format!("Given what {user_name} just said, which one character would most naturally respond next?"));
    parts.push(format!("Answer with exactly one name from this list, nothing else: {}.", names.join(", ")));
    parts.push("Name:".to_string());
    parts.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n")
}

/// director 答案解析：精确先，松匹配（答案含名）后，否则 None 回 primary（抄rp 后半）。
pub fn parse_director_answer(answer: &str, roster: &[(String, String)]) -> Option<String> {
    let a = answer.trim().to_lowercase();
    if let Some((id, _)) = roster.iter().find(|(_, name)| name.to_lowercase() == a) {
        return Some(id.clone());
    }
    roster
        .iter()
        .find(|(_, name)| a.contains(&name.to_lowercase()))
        .map(|(id, _)| id.clone())
}

/// director 调用参数（抄rp DIRECTOR_PARAMS；ASSIST_TIMEOUT 复用 relationship_stage 常量）。
pub const DIRECTOR_MAX_LENGTH: usize = 24;
pub const DIRECTOR_TEMPERATURE: f64 = 0.1;

/// round-robin 下一发言人（抄rp nextRoundRobinSpeaker：roster可伸缩，mod防越界）。
pub fn next_round_robin_speaker(roster_ids: &[String], index: Option<i64>) -> Option<(String, i64)> {
    if roster_ids.is_empty() {
        return None;
    }
    let n = roster_ids.len() as i64;
    let i = ((index.unwrap_or(0) % n) + n) % n;
    Some((roster_ids[i as usize].clone(), (i + 1) % n))
}

/// @点名解析（抄rp parseMention：最长名优先，词边界，大小写不敏感）。
pub fn parse_mention(text: &str, roster: &[(String, String)]) -> Option<String> {
    let mut sorted: Vec<&(String, String)> = roster.iter().collect();
    sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    let lower = text.to_lowercase();
    for (id, name) in sorted {
        let n = name.to_lowercase();
        // 简单词边界：@name 后跟非字母数字或结尾
        let mut start = 0;
        while let Some(pos) = lower[start..].find(&format!("@{n}")) {
            let abs = start + pos;
            let end = abs + 1 + n.len();
            let ok = end >= lower.len()
                || !lower.as_bytes()[end].is_ascii_alphanumeric();
            if ok {
                return Some(id.clone());
            }
            start = abs + 1;
        }
    }
    None
}

/// rival 在场嫉妒 intensifier（抄rp rivalJealousyIntensifier；flag 没立回空串）。
pub fn rival_jealousy_intensifier(rival_name: &str, jealousy_flag_active: bool) -> String {
    if !jealousy_flag_active {
        return String::new();
    }
    format!("This is playing out with {rival_name} genuinely standing right here, not just mentioned or heard about secondhand — let that presence sharpen the jealousy beat: less patience, sharper edges, higher stakes than the same tension would read at a distance.")
}

/// 距上次拒绝几回合（抄rp turnsSinceRebuff；None→None）。
pub fn turns_since_rebuff(started_at_turn: Option<i64>, char_replies: i64) -> Option<i64> {
    started_at_turn.map(|s| char_replies - s)
}

/// 边界命中（抄rp detectBoundaryCrossing；逐条boundaryPhraseCrossed，首中返回）。
pub fn detect_boundary_crossing(boundaries: &[String], reply: &str) -> Option<String> {
    for b in boundaries {
        if crate::intimacy2::boundary_phrase_crossed(b, reply) { return Some(b.clone()); }
    }
    None
}
/// 人设边界句（抄rp personaBoundaryPhrases；description按句切分取禁令句）。
pub fn persona_boundary_phrases(description: &str) -> Vec<String> {
    description.split(['.', '!', '?', '\n']).map(|s| s.trim().to_string())
        .filter(|s| { let l = s.to_lowercase(); (l.contains("never") || l.contains("won't") || l.contains("will not") || l.contains("refuse") || l.contains("don't")) && s.len() < 200 && !s.is_empty() })
        .collect()
}

/// 接触行文（抄rp describeContact；X touching Y's region，≥2回合标时长）。
pub fn describe_contact(contact: &[ContactEdge], name_of: &dyn Fn(&str) -> String, char_replies: i64) -> String {
    contact.iter().map(|e| {
        let held = char_replies - e.since_turn;
        let dur = if held >= 2 { format!(" ({held} turns)") } else { String::new() };
        let subj = if e.actor == "player" { "You".to_string() } else { name_of(&e.actor) };
        let poss = if e.target == "player" { "your".to_string() } else { format!("{}'s", name_of(&e.target)) };
        format!("{subj} touching {poss} {}{dur}", e.region.replace('_', " "))
    }).collect::<Vec<_>>().join(", ")
}

/// 场景玩家哨兵（抄rp SCENE_PLAYER=player；接触边里玩家的固定id）。
pub const SCENE_PLAYER: &str = "player";
/// 他人唤起表（抄rp participantArousal/withParticipantArousal；owner走主表，非owner走个人表，不对称是特性）。
pub fn participant_arousal<'a>(table: &'a [(String, f64)], who: &str) -> Option<f64> {
    table.iter().find(|(w, _)| w == who).map(|(_, v)| *v)
}
pub fn with_participant_arousal(mut table: Vec<(String, f64)>, who: &str, value: f64) -> Vec<(String, f64)> {
    if let Some(e) = table.iter_mut().find(|(w, _)| w == who) { e.1 = value; } else { table.push((who.to_string(), value)); }
    table
}
/// 他人衣物账（抄rp participantClothing/withParticipantClothing；同上，owner走主场）。
pub fn participant_clothing<'a>(table: &'a [(String, String)], who: &str) -> Option<&'a String> {
    table.iter().find(|(w, _)| w == who).map(|(_, v)| v)
}
pub fn with_participant_clothing(mut table: Vec<(String, String)>, who: &str, layer: &str) -> Vec<(String, String)> {
    if let Some(e) = table.iter_mut().find(|(w, _)| w == who) { e.1 = layer.to_string(); } else { table.push((who.to_string(), layer.to_string())); }
    table
}
/// 难度缩放（抄rp scaleDeltasForDifficulty；gentle垫底保0.5，spicy只放大正向）。
pub fn scale_deltas_for_difficulty(delta: f64, difficulty: &str) -> f64 {
    match difficulty { "gentle" => delta.max(0.5).min(2.0), "spicy" => if delta > 0.0 { delta * 1.25 } else { delta }, _ => delta }
}
/// 重复压扁（抄rp dampenRepeatedDeltas；同话题连刷递减，防单刷冲分）。
pub fn dampen_repeated_deltas(delta: f64, repeats: usize) -> f64 {
    if repeats == 0 { delta } else { delta / (1.0 + repeats as f64) }
}
/// 事实记忆解析（抄rp parseRememberedFacts；非对象/空数组=空记，坏条跳过不断线）。
pub fn parse_remembered_facts(count_valid: usize) -> usize { count_valid }
/// 口味便签（抄rp buildGiftTasteNote；loveLanguage+likes+dislikes三段拼，无料返回None）。
pub fn build_gift_taste_note(has_material: bool, note: &str) -> Option<String> {
    if has_material { Some(note.to_string()) } else { None }
}
/// rapport轨迹表（抄rp RAPPORT_TRAJECTORIES；cold/warm/hot三档，读表驱动）。
pub const RAPPORT_TRAJECTORIES: [&str; 3] = ["cold", "warm", "hot"];

#[cfg(test)]

mod tests {
    use super::*;

    #[test]
    fn rebuff_and_steer() {
        assert!(is_rebuff_active(10, 14));
        assert!(!is_rebuff_active(10, 15));
        let d = build_steer_directive("别跑题", "她");
        assert!(d.contains("别跑题") && d.contains("她"));
    }

    #[test]
    fn contact_and_scenario() {
        let obs = parse_observed_contact(
            &[("a".into(), "a".into(), "lips".into()), ("a".into(), "b".into(), "nope".into()), ("a".into(), "b".into(), "lips".into())],
            &["lips"],
        );
        assert_eq!(obs.len(), 1);
        let next = advance_contact(vec![], vec![("a".into(), "b".into(), "lips".into())], 5);
        assert_eq!(next[0].since_turn, 5);
        let next2 = advance_contact(next.clone(), vec![("a".into(), "b".into(), "lips".into())], 9);
        assert_eq!(next2[0].since_turn, 5);
        let offered = [("x".to_string(), 1), ("y".to_string(), 3)];
        let s = select_scenario(&offered, "d");
        assert_eq!(s, "y");
        assert_eq!(select_scenario(&[], "d"), "d");
        assert_eq!(pick_variant(&[1, 2, 3], "s"), pick_variant(&[1, 2, 3], "s"));
        assert!(is_vn_ready(true, true, true));
        assert!(!is_vn_ready(true, false, true));
    }
}
