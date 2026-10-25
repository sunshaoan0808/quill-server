//! catalog2.rs — 亲密目录门控/剧本图校验（吸收自 pnotisdev/rp dating/intimacyCatalog.ts + intimacyStages.ts + scenarios.ts，MIT）。
//!
//! 纪律（抄rp）：
//! - 解锁=数值门（warmth+承诺阶），硬限制一票否决；解锁≠可用（模型只许引用已解锁）。
//! - kissing_spot是浪漫内容永不进explicit门；其余三类prompt侧只在explicit出现。
//! - 剧本图导入先验结构再验图规则，坏图进不了场景。
//! - 全纯函数，零迁移。

/// 四类：吻位/体位/玩具/活动。
pub fn is_explicit_category(cat: &str) -> bool {
    cat != "kissing_spot"
}

/// 玩家面板可选类：只有fade_to_black/suggestive拿走选择权；default全留（存量承诺）。
pub fn allowed_categories(level: &str) -> Vec<&'static str> {
    let all = vec!["kissing_spot", "position", "toy", "activity"];
    match level {
        "fade_to_black" | "suggestive" => vec!["kissing_spot"],
        _ => all,
    }
}

/// 解锁判定：warmth够 + 承诺阶够 + 无硬限制 + 部位全可用。
pub fn is_unlocked(
    min_warmth: f64,
    warmth: f64,
    min_commitment_rank: Option<usize>,
    commitment_rank: usize,
    has_hard_limit: bool,
    regions_ok: bool,
) -> bool {
    if warmth < min_warmth {
        return false;
    }
    if let Some(need) = min_commitment_rank {
        if commitment_rank < need {
            return false;
        }
    }
    !has_hard_limit && regions_ok
}

/// 承诺阶排名（与relationship_stage::COMMITMENT_ORDER同口径）。
pub fn commitment_rank(status: &str) -> Option<usize> {
    ["none", "dating", "exclusive", "living_together", "married"]
        .iter()
        .position(|s| *s == status)
}

/// prompt行每类最多4个（取最高阈值的最新鲜者）。
pub fn top_labels<'a>(labels_by_warmth_desc: &'a [(String, f64)], cap: usize) -> Vec<&'a str> {
    labels_by_warmth_desc.iter().take(cap).map(|(l, _)| l.as_str()).collect()
}

/// 玩家动作行：{char}替换；无actionText按类兜底。
pub fn compose_action_text(action_text: Option<&str>, category: &str, label: &str, char_name: &str) -> String {
    let t = match action_text {
        Some(s) => s.to_string(),
        None if category == "kissing_spot" => format!("*kisses {{char}} on the {label}*"),
        None => format!("*brings up trying {label}*"),
    };
    t.replace("{char}", char_name)
}

/// 剧本结构校验（导入期）：坏图直接拒，场景见不到。
pub fn validate_scenario_source(
    id: &str,
    version: i64,
    title: &str,
    entry_stage: &str,
    stages: &[(&str, &str, f64, f64, usize)],
) -> Vec<String> {
    let mut problems = vec![];
    if id.trim().is_empty() {
        problems.push("missing id".to_string());
    }
    if version != 1 {
        problems.push("unsupported version — expected 1".to_string());
    }
    if title.trim().is_empty() {
        problems.push("missing title".to_string());
    }
    if entry_stage.trim().is_empty() {
        problems.push("missing entryStage".to_string());
    }
    let ids: Vec<&str> = stages.iter().map(|(sid, _, _, _, _)| *sid).collect();
    if !ids.contains(&entry_stage) {
        problems.push("entryStage points nowhere".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for (sid, kind, _, _, edges) in stages {
        if !seen.insert(*sid) {
            problems.push(format!("duplicate stage {sid}"));
        }
        if !["opening", "foreplay", "oral", "penetrative", "climax"].contains(kind) {
            problems.push(format!("stage {sid} has no kind"));
        }
        let _ = (kind, edges);
    }
    problems
}

/// 剧本选择：offered内specificity（eligibility数+内容档要求）最高者胜；保底默认。
pub fn select_scenario_by_specificity<'a>(offered: &'a [(String, usize)], default_id: &'a str) -> &'a str {
    offered.iter().max_by_key(|(_, s)| *s).map(|(id, _)| id.as_str()).unwrap_or(default_id)
}

/// 内容档位序（抄rp DETAIL_ORDER；fade<suggestive<default<explicit）。
pub const DETAIL_ORDER: [&str; 4] = ["fade_to_black","suggestive","default","explicit"];
pub fn detail_level_met(required: Option<&str>, actual: &str) -> bool {
    let rank = |s: &str| DETAIL_ORDER.iter().position(|x| *x==s).unwrap_or(0);
    match required { None=>true, Some(r)=> rank(actual) >= rank(r) }
}
/// 分支场景门槛（抄rp BRANCHING_SCENARIO；explicit档+dating以上才给分支）。
pub const BRANCHING_MIN_DETAIL: &str = "explicit";
pub const BRANCHING_MIN_COMMITMENT: &str = "dating";

/// 后继剧本（抄rp successorScenario；首个条件成立且在catalog sendoffered的才跟）。
pub fn successor_scenario_id(successors: &[(String, bool, String)], catalog_ids_offered: &[String]) -> Option<String> {
    for (scenario_id, cond_met, _label) in successors {
        if !cond_met { continue; }
        if catalog_ids_offered.iter().any(|id| id == scenario_id) { return Some(scenario_id.clone()); }
    }
    None
}
/// 剧本专一度（抄rp specificity；eligibility条数+detail门1分）。
pub fn scenario_specificity(eligibility_len: usize, has_detail_gate: bool) -> usize {
    eligibility_len + if has_detail_gate { 1 } else { 0 }
}

/// 条目唤起权重（抄rp intimacyArousalWeight；自带值优先，否则类基+90档2/75档1）。
pub fn intimacy_arousal_weight(category: &str, authored: Option<f64>, min_warmth: f64) -> f64 {
    if let Some(w) = authored { return w; }
    let base = match category { "kissing_spot" => 1.0, "position" => 7.0, "toy" => 4.0, _ => 3.0 };
    let bump = if min_warmth >= 90.0 { 2.0 } else if min_warmth >= 75.0 { 1.0 } else { 0.0 };
    base + bump
}
/// 条目准入（抄rp isEntryAllowed；硬雷一票否，部位warmth门全过才行）。
pub fn is_intimacy_entry_allowed(is_hard_limit: bool, region_ok: bool) -> bool {
    !is_hard_limit && region_ok
}
/// 解锁过滤（抄rp getUnlockedIntimacyOptions；warmth+承诺+玩具持有+准入四门）。
pub fn is_intimacy_unlocked(warmth: f64, min_warmth: f64, commitment_ok: bool, toy_owned: bool, entry_allowed: bool) -> bool {
    warmth >= min_warmth && commitment_ok && toy_owned && entry_allowed
}
/// 默认prompt注记（抄rp defaultIntimacyPromptNote；四类各一句白话）。
pub fn default_intimacy_prompt_note(category: &str, label: &str, char_name: &str) -> String {
    match category {
        "kissing_spot" => format!("a kiss to {char_name}'s {label}"),
        "position" => format!("the {label} position"),
        "toy" => format!("using {label} on {char_name}"),
        _ => label.to_string(),
    }
}

/// 下一解锁（抄rp nextLockedInCategory；同类未解锁取最低minWarmth）。
pub fn next_locked_min_warmth(entries: &[(String, String, f64)], category: &str, unlocked_ids: &[String]) -> Option<(String, f64)> {
    entries.iter().filter(|(id, c, _)| c == category && !unlocked_ids.iter().any(|u| u == id))
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(id, _, w)| (id.clone(), *w))
}
/// 解锁行文（抄rp intimacyOptionsGuidance；吻位常显，余类仅explicit；每类最多4个）。
pub const MAX_PER_CATEGORY: usize = 4;
pub fn intimacy_options_guidance(has_kiss: bool, explicit: bool, has_pos: bool, has_toy: bool, has_act: bool) -> String {
    if !has_kiss && !(explicit && (has_pos || has_toy || has_act)) { return String::new(); }
    let mut parts: Vec<String> = Vec::new();
    if has_kiss { parts.push("Places a kiss could land now that you're this close.".to_string()); }
    if explicit {
        if has_pos { parts.push("Positions this relationship has earned, if a scene goes there.".to_string()); }
        if has_toy { parts.push("Toys or props that would fit.".to_string()); }
        if has_act { parts.push("Other things that could come up.".to_string()); }
    }
    format!("{} Use whichever, if any, genuinely fits this exact moment, and never force one in just because it's unlocked.", parts.join(" "))
}
/// 动作指令行文（抄rp intimacyActionDirective；玩家已动手，写角色下一拍）。
pub fn intimacy_action_directive(persona_name: &str, char_name: &str, note: &str) -> String {
    format!("{persona_name} has just moved the scene into {note}. This is something {persona_name} is doing on purpose, right now, not a passing detail. Write {char_name}'s response to it as the actual next beat.")
}

/// 边稳定id（抄rp stageEdgeId；stage:index，两边同目标也区分）。
pub fn stage_edge_id(stage_id: &str, index: usize) -> String { format!("{stage_id}:{index}") }
/// 阶段新老两相读（抄rp phaseForStageKind；oral/penetrative/climax=peak其余building）。
pub fn phase_for_stage_kind(kind: &str) -> &'static str {
    match kind { "oral" | "penetrative" | "climax" => "peak", _ => "building" }
}
/// 超时逗留（抄rp stageOverstayed；softMaxSome且turns>cap才算，只劝不拦）。
pub fn stage_overstayed(soft_max_turns: Option<i64>, turns_in_stage: i64) -> bool {
    match soft_max_turns { None => false, Some(cap) => turns_in_stage > cap }
}
/// 分支默认接管（抄rp choiceDefaultDue；有默认项且since≥3回合）。
pub fn choice_default_due(has_default: bool, since_turn: i64, char_replies: i64) -> bool {
    has_default && char_replies - since_turn >= crate::intimacy2::CHOICE_DEFAULT_AFTER_TURNS
}
/// 选项命中（抄rp pendingChoiceOption；先对id再对edgeTo兜底，老存档不断线）。
pub fn pending_choice_option<'a>(options: &'a [(String, String)], option_id: &str) -> Option<&'a (String, String)> {
    options.iter().find(|(id, _)| id == option_id).or_else(|| options.iter().find(|(_, to)| to == option_id))
}
/// 条件全过（抄rp stageConditionsMet；空=全过）。
pub fn stage_conditions_met(conds: &[bool]) -> bool { conds.iter().all(|c| *c) }
/// 单条件判定（抄rp stageConditionHolds；arousal/到访/持有/kink四种，relationship半交给调用方）。
pub fn stage_condition_holds(kind: &str, arousal: f64, value: f64, visited: bool, owned: bool, kink_valence: i32, kink_need: i32) -> bool {
    match kind {
        "arousal_at_least" => arousal >= value,
        "arousal_below" => arousal < value,
        "stage_visited" => visited,
        "inventory_has" => owned,
        "kink_not_limited" => kink_valence > -2,
        "kink_at_least" => kink_valence >= kink_need,
        _ => false,
    }
}
/// 自动边（抄rp nextAutoEdge；首条条件成立的auto非resolve，终结必须judge亲眼见）。
pub fn next_auto_edge<'a>(edges: &'a [(String, String, bool)], conds_met: &[bool]) -> Option<&'a (String, String, bool)> {
    for (i, e) in edges.iter().enumerate() {
        if e.1 == "auto" && conds_met.get(i).copied().unwrap_or(false) && e.0 != crate::intimacy2::RESOLVE_STAGE { return Some(e); }
    }
    None
}
/// 终结边（抄rp resolveEdge；auto+resolve+条件成立才给走，choice终结必须玩家亲手点）。
pub fn resolve_edge<'a>(edges: &'a [(String, String, bool)], conds_met: &[bool]) -> Option<&'a (String, String, bool)> {
    for (i, e) in edges.iter().enumerate() {
        if e.1 == "auto" && conds_met.get(i).copied().unwrap_or(false) && e.0 == crate::intimacy2::RESOLVE_STAGE { return Some(e); }
    }
    None
}
/// 可选分支边下标（抄rp eligibleChoiceEdges；choice+条件成立才上屏，什么都不自动走）。
pub fn eligible_choice_edge_indices(edges: &[(String, String, bool)], conds_met: &[bool]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, e) in edges.iter().enumerate() {
        if e.1 == "choice" && conds_met.get(i).copied().unwrap_or(false) { out.push(i); }
    }
    out
}

/// 条目部位（抄rp intimacyEntryRegions；自带优先，内置表兜底，否则空）。
pub fn intimacy_entry_regions<'a>(own: Option<&'a [String]>, builtin: Option<&'a [String]>) -> &'a [String] {
    match (own, builtin) { (Some(o), _) if !o.is_empty() => o, (_, Some(b)) => b, _ => &[] }
}
/// 条目kink（抄rp intimacyEntryKinks；同上三段式）。
pub fn intimacy_entry_kinks<'a>(own: Option<&'a [String]>, builtin: Option<&'a [String]>) -> &'a [String] {
    match (own, builtin) { (Some(o), _) if !o.is_empty() => o, (_, Some(b)) => b, _ => &[] }
}
/// 内置目录条数（抄rp DEFAULT_INTIMACY_CATALOG；kiss11+pos9+toy8+act9=37）。
pub const DEFAULT_INTIMACY_CATALOG_LEN: usize = 37;
/// 逐类目录（抄rp getIntimacyCatalog；replace=true则只用world自带，否则内置+world叠加）。
pub fn get_intimacy_catalog<'a>(builtin: &'a [String], world: Option<&'a [String]>, replace: bool) -> Vec<&'a String> {
    if replace { return world.unwrap_or(&[]).iter().collect(); }
    builtin.iter().chain(world.unwrap_or(&[]).iter()).collect()
}
/// 单条查表（抄rp intimacyItemById；buyToy同口径）。
pub fn intimacy_item_by_id<'a>(catalog: &'a [(String, String)], id: &str) -> Option<&'a (String, String)> {
    catalog.iter().find(|(i, _)| i == id)
}
/// 下一解锁（抄rp nextLockedInCategory；同类未解锁取最低minWarmth）。
pub fn next_locked_in_category(entries: &[(String, String, f64)], category: &str, unlocked_ids: &[String]) -> Option<(String, f64)> {
    next_locked_min_warmth(entries, category, unlocked_ids)
}
/// 动作行合成（抄rp composeIntimacyActionText；有actionText直接换{char}，否则吻位/其他两套兜底）。
pub fn compose_intimacy_action_text(action_text: Option<&str>, category: &str, label: &str, char_name: &str) -> String {
    compose_action_text(action_text, category, label, char_name)
}
/// prompt注记（抄rp resolveIntimacyPromptNote；有promptNote直接换{char}，否则四类各一句白话）。
pub fn resolve_intimacy_prompt_note(prompt_note: Option<&str>, category: &str, label: &str, char_name: &str) -> String {
    match prompt_note { Some(s) => s.replace("{char}", char_name), None => default_intimacy_prompt_note(category, label, char_name) }
}
/// 档位准入（抄rp allowedIntimacyCategories；fade/suggestive只给吻位，default全留存量承诺）。
pub fn allowed_intimacy_categories(level: &str) -> Vec<&'static str> { allowed_categories(level) }
/// 解锁全集（抄rp getUnlockedIntimacyOptions；warmth+承诺+玩具持有+硬限制部位四门）。
pub fn get_unlocked_intimacy_options(entries: &[(String, f64, Option<usize>, bool, bool)], warmth: f64, commitment_rank_now: usize, toy_owned: &[bool]) -> Vec<usize> {
    entries.iter().enumerate().filter(|(i, ( _, min_w, min_c, hard, regions_ok))| {
        let owned = toy_owned.get(*i).copied().unwrap_or(true);
        is_unlocked(*min_w, warmth, *min_c, commitment_rank_now, *hard, *regions_ok) && owned
    }).map(|(i, _)| i).collect()
}
/// 单条准入（抄rp isEntryAllowed；硬雷一票否，部位warmth门全过才行）。
pub fn is_entry_allowed(is_hard_limit: bool, region_ok: bool) -> bool { is_intimacy_entry_allowed(is_hard_limit, region_ok) }

/// 剧本查表（抄rp scenarioById/getScenarioCatalog；找不到回默认open scene，场景永不裸奔）。
pub fn scenario_by_id<'a>(catalog: &'a [(String, String)], id: &str, default_id: &str) -> &'a (String, String) {
    catalog.iter().find(|(i, _)| i == id).unwrap_or_else(|| catalog.iter().find(|(i, _)| i == default_id).unwrap_or(&catalog[0]))
}
pub const BUILT_IN_SCENARIOS: [&str; 2] = ["default", "branching-explicit"];
pub fn get_scenario_catalog<'a>(builtin: &'a [String], world: Option<&'a [String]>) -> Vec<&'a String> {
    builtin.iter().chain(world.unwrap_or(&[]).iter()).collect()
}
/// 导入校验（抄rp parseScenarioGraph/validateScenarioSource；先验结构再验图规则，坏图进不了场景）。
pub fn parse_scenario_graph(id_ok: bool, version_ok: bool, title_ok: bool, entry_ok: bool, stages_ok: bool, graph_problems: &[String]) -> Vec<String> {
    let mut p = vec![];
    if !id_ok { p.push("missing id".to_string()); }
    if !version_ok { p.push("unsupported version — expected 1".to_string()); }
    if !title_ok { p.push("missing title".to_string()); }
    if !entry_ok { p.push("missing entryStage".to_string()); }
    if !stages_ok { p.push("stages must be an array".to_string()); }
    if p.is_empty() { p.extend(graph_problems.iter().cloned()); }
    p
}
/// 图规则校验（抄rp validateScenarioGraph；缺entry/边指空/死胡同/单choice陷阱/不可达/无resolve全算病）。
pub fn validate_scenario_graph(entry_in_graph: bool, edge_unknown: &[String], dead_ends: &[String], single_choice_traps: &[String], unreachable: &[String], has_resolve: bool) -> Vec<String> {
    let mut p = vec![];
    if !entry_in_graph { p.push("entry stage is not in the graph".to_string()); }
    for e in edge_unknown { p.push(format!("edge to unknown stage \"{e}\"")); }
    for s in dead_ends { p.push(format!("stage \"{s}\" is a dead end")); }
    for s in single_choice_traps { p.push(format!("stage \"{s}\" can never be left — a single choice edge is not a decision")); }
    for s in unreachable { p.push(format!("stage \"{s}\" is unreachable")); }
    if !has_resolve { p.push("no stage can resolve the scene".to_string()); }
    p
}
/// 默认open scene（抄rp DEFAULT_SCENARIO；两段式，build→peak，观察到完成才resolve）。
pub const DEFAULT_SCENARIO_ID: &str = "default";
/// stage查表（抄rp stageById；找不到=None，调用方回entry防 stranded）。
pub fn stage_by_id<'a>(stages: &'a [(String, String)], id: &str) -> Option<&'a (String, String)> {
    stages.iter().find(|(s, _)| s == id)
}
/// 可选分支下标（抄rp eligibleChoiceEdges；choice+条件成立才上屏）。
pub fn eligible_choice_edges(indices: &[usize]) -> Vec<usize> { indices.to_vec() }
/// 分支组装（抄rp buildPendingChoice；单选项不成决策直接None，默认项isDefault优先）。
pub fn build_pending_choice(option_ids: &[String], default_idx: Option<usize>, since_turn: i64) -> Option<(Vec<String>, usize, i64)> {
    if option_ids.len() < 2 { return None; }
    Some((option_ids.to_vec(), default_idx.unwrap_or(0), since_turn))
}
/// 默认选项（抄rp defaultChoiceOption；先对id再对edgeTo兜底）。
pub fn default_choice_option<'a>(options: &'a [(String, String)], default_id: Option<&str>, default_edge_to: Option<&str>) -> Option<&'a (String, String)> {
    pending_choice_option(options, default_id.unwrap_or(default_edge_to.unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gates() {
        assert!(is_explicit_category("position"));
        assert!(!is_explicit_category("kissing_spot"));
        assert_eq!(allowed_categories("fade_to_black"), vec!["kissing_spot"]);
        assert_eq!(allowed_categories("default").len(), 4);
        assert!(is_unlocked(50.0, 60.0, Some(1), 1, false, true));
        assert!(!is_unlocked(50.0, 60.0, Some(2), 1, false, true));
        assert!(!is_unlocked(50.0, 60.0, None, 0, true, true));
        assert_eq!(commitment_rank("married"), Some(4));
        assert_eq!(
            compose_action_text(None, "kissing_spot", "额头", "她"),
            "*kisses 她 on the 额头*"
        );
    }

    #[test]
    fn scenario_validate_and_select() {
        let p = validate_scenario_source("", 2, "", "", &[]);
        assert!(p.len() >= 3);
        let ok = validate_scenario_source("s", 1, "t", "a", &[("a", "foreplay", 0.0, 3.0, 1)]);
        assert!(ok.is_empty());
        let offered1 = [("x".to_string(), 1)];
        assert_eq!(select_scenario_by_specificity(&offered1, "d"), "x");
        assert_eq!(select_scenario_by_specificity(&[], "d"), "d");
    }
}
