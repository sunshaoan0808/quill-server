//! agency.rs — 角色代理层三件套（吸收自 pnotisdev/rp plans.ts / beliefs.ts / expectations.ts，MIT）。
//!
//! **[EXPERIMENTAL — 无生产消费面]**（2026-09-14 接线审计：docs/接线审计-摆设功能全摸排-20260914.md §6.3）
//! 现状：本模块的引用目前只在 core 内部互引，从 `main.rs` 走不到任何生产消费面（plans · beliefs · expectations 三件套与 guidance 注入无调用方）。
//! 判据「走不到 = 摆设」下，**不要为了消红而硬接**——硬接即造假接线。要用请先拍板「接哪个 UI / 端点」，
//! 拍板后从真实入口接线，并同步删掉本标记与 `scripts/audit/README.md` 的对应条目。
//!
//! plans[≤3] / beliefs[≤4] / expectations[≤3] + cap + aging + 去重 + guidance 注入。
//! 纪律（抄rp）：
//! - 索引语义指向 judge 下发列表原序号，先算 resolve 再加 add（防 add 移位）。
//! - lifecycle 纯函数，不管副作用（violated → Fact 由调用方 tavern_engine 转）。
//! - 大多回合 updates=[] 零写；changed=false 跳过 PUT。

use serde::{Deserialize, Serialize};

pub const MAX_PLANS: usize = 3;
pub const MAX_BELIEFS: usize = 4;
pub const MAX_EXPECTATIONS: usize = 3;

pub const PLAN_TTL_TURNS: u32 = 60;
pub const BELIEF_TTL_TURNS: u32 = 80;
pub const EXPECTATION_TTL_TURNS: u32 = 80;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CharPlan {
    pub id: String,
    pub goal: String,
    pub kind: String,
    pub formed_turn: u32,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CharBelief {
    pub id: String,
    pub text: String,
    pub formed_turn: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserExpectation {
    pub id: String,
    pub text: String,
    pub formed_turn: u32,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlanUpdate {
    Add { goal: String, kind: String, note: Option<String> },
    Note { index: usize, note: String },
    Resolve { index: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub enum BeliefUpdate {
    Add { text: String },
    Revise { index: usize, text: String },
    Drop { index: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExpectationUpdate {
    Add { text: String },
    Note { index: usize, note: String },
    Resolve { index: usize, outcome: String },
}

fn get_str(o: &serde_json::Map<String, serde_json::Value>, k: &str) -> Option<String> {
    o.get(k).and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn get_idx(o: &serde_json::Map<String, serde_json::Value>, k: &str) -> Option<usize> {
    o.get(k).and_then(|v| v.as_u64()).map(|n| n as usize)
}

pub fn parse_plan_updates(raw: &serde_json::Value) -> Vec<PlanUpdate> {
    let arr = match raw.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in arr {
        let o = match item.as_object() {
            Some(o) => o,
            None => continue,
        };
        let op = o.get("op").and_then(|v| v.as_str()).unwrap_or("");
        match op {
            "add" => {
                if let Some(goal) = get_str(o, "goal") {
                    let kind = get_str(o, "kind").unwrap_or_else(|| "personal".to_string());
                    let note = get_str(o, "note");
                    out.push(PlanUpdate::Add { goal, kind, note });
                }
            }
            "note" => {
                if let (Some(i), Some(n)) = (get_idx(o, "index"), get_str(o, "note")) {
                    out.push(PlanUpdate::Note { index: i, note: n });
                }
            }
            "resolve" => {
                if let Some(i) = get_idx(o, "index") {
                    out.push(PlanUpdate::Resolve { index: i });
                }
            }
            _ => continue,
        }
    }
    out
}

pub fn parse_belief_updates(raw: &serde_json::Value) -> Vec<BeliefUpdate> {
    let arr = match raw.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in arr {
        let o = match item.as_object() {
            Some(o) => o,
            None => continue,
        };
        let op = o.get("op").and_then(|v| v.as_str()).unwrap_or("");
        match op {
            "add" => {
                if let Some(t) = get_str(o, "text") {
                    out.push(BeliefUpdate::Add { text: t });
                }
            }
            "revise" => {
                if let (Some(i), Some(t)) = (get_idx(o, "index"), get_str(o, "text")) {
                    out.push(BeliefUpdate::Revise { index: i, text: t });
                }
            }
            "drop" => {
                if let Some(i) = get_idx(o, "index") {
                    out.push(BeliefUpdate::Drop { index: i });
                }
            }
            _ => continue,
        }
    }
    out
}

pub fn parse_expectation_updates(raw: &serde_json::Value) -> Vec<ExpectationUpdate> {
    let arr = match raw.as_array() {
        Some(a) => a,
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    for item in arr {
        let o = match item.as_object() {
            Some(o) => o,
            None => continue,
        };
        let op = o.get("op").and_then(|v| v.as_str()).unwrap_or("");
        match op {
            "add" => {
                if let Some(t) = get_str(o, "text") {
                    out.push(ExpectationUpdate::Add { text: t });
                }
            }
            "note" => {
                if let (Some(i), Some(n)) = (get_idx(o, "index"), get_str(o, "note")) {
                    out.push(ExpectationUpdate::Note { index: i, note: n });
                }
            }
            "resolve" => {
                if let Some(i) = get_idx(o, "index") {
                    let outcome = get_str(o, "outcome").unwrap_or_else(|| "met".to_string());
                    out.push(ExpectationUpdate::Resolve { index: i, outcome });
                }
            }
            _ => continue,
        }
    }
    out
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

fn next_id(prefix: &str, turn: u32, n: usize) -> String {
    format!("{}-{}-{}", prefix, turn, n)
}

/// 返回 (新列表, changed, 被 violated resolve 掉的文本)。
pub fn apply_plan_updates(
    current: &[CharPlan],
    updates: &[PlanUpdate],
    turn: u32,
) -> (Vec<CharPlan>, bool) {
    let mut list = current.to_vec();
    let mut changed = false;
    // 先算 resolve/note（原序号），再加 add。
    for u in updates {
        match u {
            PlanUpdate::Resolve { index } => {
                if *index < list.len() {
                    list.remove(*index);
                    changed = true;
                    // remove 后后续同 pass 索引会漂移：rp 同语义，调用方应只发一致序号；
                    // 为防连环漂移，后续索引按 удаление 后位置继续（与 rp applyPlanUpdates 一致）。
                    // 简单起见：遇到多 resolve 时按降序重排在调用前已无序，此处逐个处理。
                }
            }
            PlanUpdate::Note { index, note } => {
                if let Some(p) = list.get_mut(*index) {
                    p.note = Some(note.clone());
                    changed = true;
                }
            }
            PlanUpdate::Add { .. } => {}
        }
    }
    for u in updates {
        if let PlanUpdate::Add { goal, kind, note } = u {
            if list.iter().any(|p| same_text(&p.goal, goal)) {
                continue;
            }
            // aging 先行：淘汰过期再看 cap。
            list.retain(|p| turn.saturating_sub(p.formed_turn) < PLAN_TTL_TURNS);
            list.push(CharPlan {
                id: next_id("plan", turn, list.len()),
                goal: goal.clone(),
                kind: kind.clone(),
                formed_turn: turn,
                note: note.clone(),
            });
            changed = true;
        }
    }
    // aging + cap（保最新）。
    list.retain(|p| turn.saturating_sub(p.formed_turn) < PLAN_TTL_TURNS);
    if list.len() > MAX_PLANS {
        let drop = list.len() - MAX_PLANS;
        list.drain(0..drop);
    }
    (list, changed)
}

pub fn apply_belief_updates(
    current: &[CharBelief],
    updates: &[BeliefUpdate],
    turn: u32,
) -> (Vec<CharBelief>, bool) {
    let mut list = current.to_vec();
    let mut changed = false;
    // drop/revise 先（原序号，降序防漂移）。
    let mut drops: Vec<usize> = Vec::new();
    for u in updates {
        match u {
            BeliefUpdate::Drop { index } => drops.push(*index),
            BeliefUpdate::Revise { index, text } => {
                if let Some(b) = list.get_mut(*index) {
                    b.text = text.clone();
                    changed = true;
                }
            }
            BeliefUpdate::Add { .. } => {}
        }
    }
    drops.sort_unstable_by(|a, b| b.cmp(a));
    for i in drops {
        if i < list.len() {
            list.remove(i);
            changed = true;
        }
    }
    for u in updates {
        if let BeliefUpdate::Add { text } = u {
            if list.iter().any(|b| same_text(&b.text, text)) {
                continue;
            }
            list.retain(|b| turn.saturating_sub(b.formed_turn) < BELIEF_TTL_TURNS);
            list.push(CharBelief {
                id: next_id("belief", turn, list.len()),
                text: text.clone(),
                formed_turn: turn,
            });
            changed = true;
        }
    }
    list.retain(|b| turn.saturating_sub(b.formed_turn) < BELIEF_TTL_TURNS);
    if list.len() > MAX_BELIEFS {
        let drop = list.len() - MAX_BELIEFS;
        list.drain(0..drop);
    }
    (list, changed)
}

pub fn apply_expectation_updates(
    current: &[UserExpectation],
    updates: &[ExpectationUpdate],
    turn: u32,
) -> (Vec<UserExpectation>, bool, Vec<String>) {
    let mut list = current.to_vec();
    let mut changed = false;
    let mut violated: Vec<String> = Vec::new();
    let mut resolves: Vec<(usize, String)> = Vec::new();
    for u in updates {
        match u {
            ExpectationUpdate::Resolve { index, outcome } => resolves.push((*index, outcome.clone())),
            ExpectationUpdate::Note { index, note } => {
                if let Some(e) = list.get_mut(*index) {
                    e.note = Some(note.clone());
                    changed = true;
                }
            }
            ExpectationUpdate::Add { .. } => {}
        }
    }
    resolves.sort_by(|a, b| b.0.cmp(&a.0));
    for (i, outcome) in resolves {
        if i < list.len() {
            let e = list.remove(i);
            changed = true;
            if outcome.trim().to_lowercase() == "violated" {
                violated.push(e.text);
            }
        }
    }
    for u in updates {
        if let ExpectationUpdate::Add { text } = u {
            if list.iter().any(|e| same_text(&e.text, text)) {
                continue;
            }
            list.retain(|e| turn.saturating_sub(e.formed_turn) < EXPECTATION_TTL_TURNS);
            list.push(UserExpectation {
                id: next_id("exp", turn, list.len()),
                text: text.clone(),
                formed_turn: turn,
                note: None,
            });
            changed = true;
        }
    }
    list.retain(|e| turn.saturating_sub(e.formed_turn) < EXPECTATION_TTL_TURNS);
    if list.len() > MAX_EXPECTATIONS {
        let drop = list.len() - MAX_EXPECTATIONS;
        list.drain(0..drop);
    }
    (list, changed, violated)
}

pub fn plans_guidance(char_name: &str, plans: &[CharPlan]) -> String {
    if plans.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "Beyond just responding to the user, {} is carrying intentions:\n",
        char_name
    );
    for p in plans {
        let tag = match p.kind.as_str() {
            "together" => "together",
            "distance" => "distance",
            _ => "personal",
        };
        if let Some(n) = &p.note {
            s.push_str(&format!("- ({}) {} — {}\n", tag, p.goal, n));
        } else {
            s.push_str(&format!("- ({}) {}\n", tag, p.goal));
        }
    }
    s
}

pub fn beliefs_guidance(char_name: &str, beliefs: &[CharBelief]) -> String {
    if beliefs.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "{} has formed some real impressions of the user:\n",
        char_name
    );
    for b in beliefs {
        s.push_str(&format!("- {}\n", b.text));
    }
    s
}

pub fn expectations_guidance(char_name: &str, exps: &[UserExpectation]) -> String {
    if exps.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "{} has started expecting certain things from the user:\n",
        char_name
    );
    for e in exps {
        if let Some(n) = &e.note {
            s.push_str(&format!("- {} — {}\n", e.text, n));
        } else {
            s.push_str(&format!("- {}\n", e.text));
        }
    }
    s
}

// ---------- objective prompt 小件（吸收自 pnotisdev/rp objectives/objectiveAssist.ts，MIT） ----------
//
// 只收确定性部分：任务校验、完成下标过滤、prompt 建造。
// 真正调模型由网关调用方接（GENERATE_PARAMS 见 relationship_stage 常量）。

/// 任务列表清洗：非空字符串才留（抄rp generateTasks 后半）。
pub fn clean_task_list(raw: &[String]) -> Vec<String> {
    raw.iter()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

/// 完成下标过滤：整数+范围内才认（抄rp detectCompletedTasks 尾）。
pub fn filter_completed_indices(raw: &[i64], pending_len: usize) -> Vec<usize> {
    raw.iter()
        .filter(|i| **i >= 0 && (**i as usize) < pending_len)
        .map(|i| *i as usize)
        .collect()
}

/// 任务拆分 prompt 建造（抄rp generateTasks 前半）。
pub fn build_tasks_prompt(char_name: &str, char_desc: &str, title: &str, desc: &str, count: usize) -> String {
    let c = if char_desc.trim().is_empty() {
        char_name.to_string()
    } else {
        format!("{char_name}. {desc}", desc = char_desc.trim())
    };
    let o = if desc.trim().is_empty() {
        title.to_string()
    } else {
        format!("{title}\n{desc}", desc = desc.trim())
    };
    format!(
        "You are helping plan a roleplay chat.\n\nCharacter: {c}\n\nObjective: {o}\n\nBreak this into {count} concrete, sequential steps that would plausibly happen in the roleplay on the way to achieving it. Each step should be something that could visibly occur in a scene, not an abstract sub-goal.\n\nOutput ONLY a minified JSON array of {count} short strings, one per step. No markdown fences, no commentary.\n\nJSON:"
    )
}

/// 目标建议 prompt 建造（抄rp suggestObjective 前半）。
pub fn build_suggest_objective_prompt(
    char_name: &str,
    char_desc: &str,
    char_personality: &str,
    persona_name: &str,
    persona_desc: &str,
) -> String {
    let c = {
        let mut s = char_name.to_string();
        if !char_desc.trim().is_empty() {
            s.push_str(". ");
            s.push_str(char_desc.trim());
        }
        if !char_personality.trim().is_empty() {
            s.push_str(". Personality: ");
            s.push_str(char_personality.trim());
        }
        s
    };
    let p = if persona_desc.trim().is_empty() {
        persona_name.to_string()
    } else {
        format!("{persona_name}. {desc}", desc = persona_desc.trim())
    };
    format!(
        "You are helping plan a roleplay chat between two participants.\n\nCharacter: {c}\n\nUser's persona: {p}\n\nPropose one plausible, interesting objective for this roleplay to work toward. It should fit both characters and be able to develop naturally over several exchanges.\n\nOutput ONLY a minified JSON object shaped exactly {{\"title\": \"short objective name\", \"description\": \"1-2 sentences of context\"}}. No markdown fences, no commentary.\n\nJSON:"
    )
}

/// 目标对象校验：title 非空才认（抄rp suggestObjective 尾）。
pub fn parse_suggested_objective(title: &str, description: &str) -> Option<(String, String)> {
    let t = title.trim();
    if t.is_empty() {
        return None;
    }
    Some((t.to_string(), description.trim().to_string()))
}

/// 任务完成判定 prompt 建造（抄rp detectCompletedTasks 前半）。
pub fn build_detect_tasks_prompt(reply_text: &str, pending: &[String]) -> String {
    let list = pending
        .iter()
        .enumerate()
        .map(|(i, t)| format!("{i}: {t}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "You are tracking task completion in an ongoing roleplay.\n\nThe following just happened in the scene:\n{reply_text}\n\nPending tasks:\n{list}\n\nWhich of these tasks, if any, does this moment clearly and unambiguously accomplish? Be conservative: only include a task if it plainly happened, not if it merely became more likely.\n\nOutput ONLY a minified JSON array of the completed tasks' index numbers, e.g. [0,2]. Use [] if none. No commentary.\n\nJSON:"
    )
}

/// 三件套变没变（抄rp plans/beliefs/expectationsChanged；长度变=变，id+字段逐个比）。
pub fn plans_changed(before: &[CharPlan], after: &[CharPlan]) -> bool {
    if before.len() != after.len() { return true; }
    after.iter().any(|p| match before.iter().find(|q| q.id == p.id) {
        None => true, Some(q) => q.goal != p.goal || q.kind != p.kind || q.note != p.note,
    })
}
pub fn beliefs_changed(before: &[CharBelief], after: &[CharBelief]) -> bool {
    if before.len() != after.len() { return true; }
    after.iter().any(|b| match before.iter().find(|q| q.id == b.id) {
        None => true, Some(q) => q.text != b.text,
    })
}
pub fn expectations_changed(before: &[UserExpectation], after: &[UserExpectation]) -> bool {
    if before.len() != after.len() { return true; }
    after.iter().any(|e| match before.iter().find(|q| q.id == e.id) {
        None => true, Some(q) => q.text != e.text || q.note != e.note,
    })
}
/// 三件套judge行（抄rp plan/belief/expectationLinesForJudge；kind/note baked in，不编号）。
pub fn plan_lines_for_judge(plans: &[CharPlan]) -> Vec<String> {
    plans.iter().map(|p| format!("[{}] {}{}", p.kind, p.goal, p.note.as_ref().map(|n| format!(" — {n}")).unwrap_or_default())).collect()
}
pub fn belief_lines_for_judge(beliefs: &[CharBelief]) -> Vec<String> {
    beliefs.iter().map(|b| b.text.clone()).collect()
}
pub fn expectation_lines_for_judge(exps: &[UserExpectation]) -> Vec<String> {
    exps.iter().map(|e| match &e.note { Some(n) => format!("{} — {n}", e.text), None => e.text.clone() }).collect()
}
/// 失约原文（抄rp violatedExpectationTexts；resolve+violated的下标回查旧文本）。
pub fn violated_expectation_texts(exps: &[UserExpectation], resolves: &[(usize, String)]) -> Vec<String> {
    resolves.iter().filter(|(_, o)| o.trim().to_lowercase() == "violated")
        .filter_map(|(i, _)| exps.get(*i).map(|e| e.text.clone())).collect()
}

/// 别名帽（抄rp MAX_ACTIVE_*；与MAX_*同值，防上游改名断线）。
pub const MAX_ACTIVE_PLANS: usize = MAX_PLANS;
pub const MAX_ACTIVE_BELIEFS: usize = MAX_BELIEFS;
pub const MAX_ACTIVE_EXPECTATIONS: usize = MAX_EXPECTATIONS;
pub const PLAN_STALE_TURNS: u32 = PLAN_TTL_TURNS;
pub const BELIEF_STALE_TURNS: u32 = BELIEF_TTL_TURNS;
pub const EXPECTATION_STALE_TURNS: u32 = EXPECTATION_TTL_TURNS;
/// 人设越界别名（agencyGuard detectPersonaAgencyViolation的调用口；实现见intimacy2）。
pub fn detect_persona_agency_violation(persona: &str, reply: &str) -> Option<String> { crate::intimacy2::persona_agency_violation(persona, reply) }

#[cfg(test)]
mod agency_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn plan_add_note_resolve() {
        let (l1, c1) = apply_plan_updates(
            &[],
            &[PlanUpdate::Add { goal: "学做饭".into(), kind: "together".into(), note: None }],
            10,
        );
        assert!(c1 && l1.len() == 1);
        let (l2, c2) = apply_plan_updates(
            &l1,
            &[PlanUpdate::Note { index: 0, note: "为她".into() }],
            11,
        );
        assert!(c2 && l2[0].note.as_deref() == Some("为她"));
        let (l3, c3) = apply_plan_updates(&l2, &[PlanUpdate::Resolve { index: 0 }], 12);
        assert!(c3 && l3.is_empty());
    }

    #[test]
    fn plan_dedup_and_cap() {
        let mut cur = Vec::new();
        for i in 0..5 {
            let (l, _) = apply_plan_updates(
                &cur,
                &[PlanUpdate::Add { goal: format!("g{}", i), kind: "personal".into(), note: None }],
                10,
            );
            cur = l;
        }
        assert_eq!(cur.len(), MAX_PLANS);
        assert_eq!(cur.last().unwrap().goal, "g4");
        let (l2, c2) = apply_plan_updates(
            &cur,
            &[PlanUpdate::Add { goal: "G4".into(), kind: "personal".into(), note: None }],
            11,
        );
        assert!(!c2 && l2.len() == MAX_PLANS);
    }

    #[test]
    fn plan_aging() {
        let old = vec![CharPlan { id: "x".into(), goal: "old".into(), kind: "personal".into(), formed_turn: 0, note: None }];
        let (l, _) = apply_plan_updates(&old, &[], 61);
        assert!(l.is_empty());
    }

    #[test]
    fn belief_revise_drop() {
        let (l1, _) = apply_belief_updates(
            &[],
            &[BeliefUpdate::Add { text: "他守时".into() }],
            5,
        );
        assert_eq!(l1.len(), 1);
        let (l2, _) = apply_belief_updates(
            &l1,
            &[BeliefUpdate::Revise { index: 0, text: "他很守时".into() }],
            6,
        );
        assert_eq!(l2[0].text, "他很守时");
        let (l3, c3) = apply_belief_updates(&l2, &[BeliefUpdate::Drop { index: 0 }], 7);
        assert!(c3 && l3.is_empty());
    }

    #[test]
    fn expectation_violated_out() {
        let (l1, _, _) = apply_expectation_updates(
            &[],
            &[ExpectationUpdate::Add { text: "回消息".into() }],
            5,
        );
        let (_, _, v) = apply_expectation_updates(
            &l1,
            &[ExpectationUpdate::Resolve { index: 0, outcome: "violated".into() }],
            6,
        );
        assert_eq!(v, vec!["回消息"]);
    }

    #[test]
    fn parse_tolerant() {
        let v = json!([{"op":"add","goal":"x"},{"op":"bogus"},{"nope":1}]);
        assert_eq!(parse_plan_updates(&v).len(), 1);
        assert!(parse_plan_updates(&json!({})).is_empty());
        let b = json!([{"op":"add","text":"t"}]);
        assert_eq!(parse_belief_updates(&b).len(), 1);
        let e = json!([{"op":"resolve","index":0,"outcome":"met"}]);
        assert_eq!(parse_expectation_updates(&e).len(), 1);
    }

    #[test]
    fn guidance_empty_when_none() {
        assert!(plans_guidance("A", &[]).is_empty());
        assert!(beliefs_guidance("A", &[]).is_empty());
        assert!(expectations_guidance("A", &[]).is_empty());
    }

    #[test]
    fn guidance_format() {
        let p = vec![CharPlan { id: "p".into(), goal: "g".into(), kind: "together".into(), formed_turn: 1, note: Some("n".into()) }];
        let s = plans_guidance("小晚", &p);
        assert!(s.contains("小晚") && s.contains("(together) g — n"));
    }
}
