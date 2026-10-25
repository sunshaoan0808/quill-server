//! triggers.rs — Rules硬规则引擎（吸收自 pnotisdev/rp `src/lib/world/triggers.ts`，MIT）。
//!
//! 作者可配的 when→then 硬规则：封闭条件集 + 封闭动作集，纯函数求值，
//! 跑在回合后（数值先落，规则再看），不调模型、不炸回合。
//!
//! 纪律（抄rp一字不改语义）：
//! - 未知 condition kind → false（新版数据不炸旧版）。
//! - 空 when 永不 fire。
//! - 非 repeatable 已 fire 跳过；fire 后记 id。
//! - `trigger_fired` 读"历史 + 本 pass 已 fire"，支持链式。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 条件：全部可序列化，`tag="kind"` 与 TS 侧同名（snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerCondition {
    StatAtLeast { stat: String, value: i64 },
    StatBelow { stat: String, value: i64 },
    FlagSet { flag: String },
    CommitmentAtLeast { status: String },
    DayAtLeast { day: i64 },
    TriggerFired { trigger_id: String },
}

/// 动作：封闭集，只能干 app 本来就能干的事。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TriggerAction {
    SetFlag { flag: String },
    Remember { text: String },
    Notify { text: String },
    StyleGuidance { text: String },
    StartScene {
        title: String,
        description: String,
        objective_title: String,
        objective_description: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Trigger {
    pub id: String,
    pub label: String,
    /// 缺省=开（免迁移，作者关才需显式 false）。
    #[serde(default)]
    pub enabled: Option<bool>,
    pub when: Vec<TriggerCondition>,
    pub then: Vec<TriggerAction>,
    /// 缺省一次性。
    #[serde(default)]
    pub repeatable: Option<bool>,
}

/// 求值上下文：调用方组装一次。
#[derive(Debug, Clone, Default)]
pub struct TriggerCtx {
    pub affection: i64,
    pub warmth: i64,
    pub stats: HashMap<String, i64>,
    pub flags: HashSet<String>,
    pub commitment: String,
    /// 世界时钟天；None = 无时钟（day 条件永不满足）。
    pub day: Option<i64>,
    pub fired: HashSet<String>,
}

/// 承诺阶梯序（与六维 spec 同口径，比较器先行，具体语义后续）。
pub const COMMITMENT_ORDER: &[&str] =
    &["none", "dating", "exclusive", "living_together", "married"];

fn stat_value(stat: &str, ctx: &TriggerCtx) -> i64 {
    match stat {
        "affection" => ctx.affection,
        "warmth" => ctx.warmth,
        _ => ctx.stats.get(stat).copied().unwrap_or(0),
    }
}

fn commitment_rank(s: &str) -> Option<usize> {
    COMMITMENT_ORDER.iter().position(|x| *x == s)
}

pub fn condition_holds(c: &TriggerCondition, ctx: &TriggerCtx) -> bool {
    match c {
        TriggerCondition::StatAtLeast { stat, value } => stat_value(stat, ctx) >= *value,
        TriggerCondition::StatBelow { stat, value } => stat_value(stat, ctx) < *value,
        TriggerCondition::FlagSet { flag } => ctx.flags.contains(flag),
        TriggerCondition::CommitmentAtLeast { status } => match (
            commitment_rank(&ctx.commitment),
            commitment_rank(status),
        ) {
            (Some(have), Some(need)) => have >= need,
            _ => false,
        },
        TriggerCondition::DayAtLeast { day } => match ctx.day {
            Some(d) => d >= *day,
            None => false,
        },
        TriggerCondition::TriggerFired { trigger_id } => ctx.fired.contains(trigger_id),
    }
}

/// 空 when 永不 fire（防"无条件常发"）。
pub fn trigger_satisfied(t: &Trigger, ctx: &TriggerCtx) -> bool {
    if t.when.is_empty() {
        return false;
    }
    t.when.iter().all(|c| condition_holds(c, ctx))
}

pub struct TriggerEvaluation {
    pub fired: Vec<Trigger>,
    pub actions: Vec<TriggerAction>,
    pub fired_ids: Vec<String>,
}

/// 纯函数：调用方负责落动作 + 持久 fired_ids。
pub fn evaluate_triggers(
    triggers: &[Trigger],
    ctx: &TriggerCtx,
    already: &[String],
) -> TriggerEvaluation {
    let mut fired_ids: HashSet<String> = already.iter().cloned().collect();
    let mut fired: Vec<Trigger> = Vec::new();
    for t in triggers {
        if t.enabled == Some(false) {
            continue;
        }
        let repeatable = t.repeatable == Some(true);
        if !repeatable && fired_ids.contains(&t.id) {
            continue;
        }
        // fired 含历史 + 本 pass 已 fire（链式 trigger_fired 用）。
        let live = TriggerCtx {
            fired: fired_ids.clone(),
            ..ctx.clone()
        };
        // 注意：ctx.clone 开销小（HashMap 小），保持纯函数签名简单。
        if !trigger_satisfied(t, &live) {
            continue;
        }
        fired.push(t.clone());
        if !repeatable {
            fired_ids.insert(t.id.clone());
        }
    }
    let actions: Vec<TriggerAction> =
        fired.iter().flat_map(|t| t.then.clone()).collect();
    let mut ids: Vec<String> = fired_ids.into_iter().collect();
    ids.sort();
    TriggerEvaluation { fired, actions, fired_ids: ids }
}

/// warmth 口径（与六维 spec 同）：五维均值，缺省回退 affinity。
pub fn warmth_of(
    affection: i64,
    dims: &HashMap<String, i64>,
) -> i64 {
    let keys = ["trust", "chemistry", "comfort", "respect"];
    let mut sum = affection;
    let mut n = 1i64;
    for k in keys {
        if let Some(v) = dims.get(k) {
            sum += *v;
            n += 1;
        } else {
            sum += affection;
            n += 1;
        }
    }
    sum / n
}

/// 把自由label转稳定id（id 是"已fire"记忆键，label 改了 id 不能变）。
pub fn slugify_trigger_id(label: &str, existing: &[String]) -> String {
    let mut base: String = label
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    while base.contains("--") {
        base = base.replace("--", "-");
    }
    let base = base.trim_matches('-').to_string();
    let base = if base.is_empty() { "trigger".to_string() } else { base };
    if !existing.iter().any(|e| e == &base) {
        return base;
    }
    let mut i = 2u32;
    loop {
        let cand = format!("{}-{}", base, i);
        if !existing.iter().any(|e| e == &cand) {
            return cand;
        }
        i += 1;
    }
}

/// 条件行文（抄rp describeCondition；编辑器摘要行）。
pub fn describe_condition(kind: &str, stat: &str, value: i64, flag: &str, status: &str, day: i64, trigger_id: &str) -> String {
    match kind {
        "stat_at_least" => format!("{stat} ≥ {value}"),
        "stat_below" => format!("{stat} < {value}"),
        "flag_set" => format!("flag \"{flag}\""),
        "commitment_at_least" => format!("at least {}", status.replace('_', " ")),
        "day_at_least" => format!("day {day}+"),
        "trigger_fired" => format!("rule \"{trigger_id}\" has already fired"),
        _ => "unknown condition".to_string(),
    }
}
/// 动作行文（抄rp describeAction；编辑器摘要行）。
pub fn describe_action(kind: &str, flag: &str, text: &str, topic: &str, title: &str) -> String {
    match kind {
        "set_flag" => format!("set flag \"{flag}\""),
        "remember" => format!("remember \"{text}\""),
        "notify" => format!("notify \"{text}\""),
        "social_reaction" => format!("a named connection reacts to \"{topic}\""),
        "style_guidance" => format!("steer: \"{text}\""),
        "start_scene" => format!("start scene \"{title}\""),
        _ => "unknown action".to_string(),
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::*;

    fn ctx() -> TriggerCtx {
        TriggerCtx {
            affection: 60,
            warmth: 55,
            stats: [("tension".to_string(), 85)].into_iter().collect(),
            flags: ["first_kiss".to_string()].into_iter().collect(),
            commitment: "dating".to_string(),
            day: Some(10),
            fired: HashSet::new(),
        }
    }

    fn mk(id: &str, when: Vec<TriggerCondition>) -> Trigger {
        Trigger {
            id: id.to_string(),
            label: id.to_string(),
            enabled: None,
            when,
            then: vec![TriggerAction::Notify { text: "hi".to_string() }],
            repeatable: None,
        }
    }

    #[test]
    fn empty_when_never_fires() {
        assert!(!trigger_satisfied(&mk("a", vec![]), &ctx()));
    }

    #[test]
    fn stat_conditions() {
        let c = ctx();
        assert!(condition_holds(
            &TriggerCondition::StatAtLeast { stat: "warmth".to_string(), value: 50 },
            &c
        ));
        assert!(condition_holds(
            &TriggerCondition::StatBelow { stat: "affection".to_string(), value: 61 },
            &c
        ));
        assert!(!condition_holds(
            &TriggerCondition::StatAtLeast { stat: "nope".to_string(), value: 1 },
            &c
        ));
    }

    #[test]
    fn flag_day_commitment() {
        let c = ctx();
        assert!(condition_holds(
            &TriggerCondition::FlagSet { flag: "first_kiss".to_string() },
            &c
        ));
        assert!(!condition_holds(
            &TriggerCondition::FlagSet { flag: "nope".to_string() },
            &c
        ));
        assert!(condition_holds(
            &TriggerCondition::DayAtLeast { day: 10 },
            &c
        ));
        assert!(!condition_holds(
            &TriggerCondition::DayAtLeast { day: 11 },
            &c
        ));
        let mut no_day = ctx();
        no_day.day = None;
        assert!(!condition_holds(
            &TriggerCondition::DayAtLeast { day: 0 },
            &no_day
        ));
        assert!(condition_holds(
            &TriggerCondition::CommitmentAtLeast { status: "dating".to_string() },
            &c
        ));
        assert!(!condition_holds(
            &TriggerCondition::CommitmentAtLeast { status: "exclusive".to_string() },
            &c
        ));
        assert!(!condition_holds(
            &TriggerCondition::CommitmentAtLeast { status: "bogus".to_string() },
            &c
        ));
    }

    #[test]
    fn oneshot_no_refire() {
        let t = mk(
            "a",
            vec![TriggerCondition::StatAtLeast { stat: "warmth".to_string(), value: 1 }],
        );
        let c = ctx();
        let r1 = evaluate_triggers(&[t.clone()], &c, &[]);
        assert_eq!(r1.fired.len(), 1);
        assert_eq!(r1.actions.len(), 1);
        let r2 = evaluate_triggers(&[t], &c, &r1.fired_ids);
        assert!(r2.fired.is_empty());
    }

    #[test]
    fn repeatable_refires() {
        let mut t = mk(
            "a",
            vec![TriggerCondition::StatAtLeast { stat: "warmth".to_string(), value: 1 }],
        );
        t.repeatable = Some(true);
        let c = ctx();
        let r1 = evaluate_triggers(&[t.clone()], &c, &[]);
        let r2 = evaluate_triggers(&[t], &c, &r1.fired_ids);
        assert_eq!(r2.fired.len(), 1);
    }

    #[test]
    fn disabled_skipped() {
        let mut t = mk(
            "a",
            vec![TriggerCondition::StatAtLeast { stat: "warmth".to_string(), value: 1 }],
        );
        t.enabled = Some(false);
        let r = evaluate_triggers(&[t], &ctx(), &[]);
        assert!(r.fired.is_empty());
    }

    #[test]
    fn chained_trigger_fired() {
        let a = mk(
            "a",
            vec![TriggerCondition::StatAtLeast { stat: "warmth".to_string(), value: 1 }],
        );
        let b = mk("b", vec![TriggerCondition::TriggerFired { trigger_id: "a".to_string() }]);
        let r = evaluate_triggers(&[a, b], &ctx(), &[]);
        assert_eq!(r.fired.len(), 2);
        assert!(r.fired_ids.contains(&"a".to_string()));
        assert!(r.fired_ids.contains(&"b".to_string()));
    }

    #[test]
    fn warmth_fallback() {
        let d: HashMap<String, i64> = HashMap::new();
        assert_eq!(warmth_of(60, &d), 60);
        let d2: HashMap<String, i64> =
            [("trust".to_string(), 80)].into_iter().collect();
        // (60+80+60+60+60)/5 = 64
        assert_eq!(warmth_of(60, &d2), 64);
    }

    #[test]
    fn slug_stable() {
        assert_eq!(slugify_trigger_id("Hello World", &[]), "hello-world");
        assert_eq!(
            slugify_trigger_id("Hello World", &["hello-world".to_string()]),
            "hello-world-2"
        );
        assert_eq!(slugify_trigger_id("!!!", &[]), "trigger");
    }

    #[test]
    fn serde_roundtrip() {
        let t = mk(
            "a",
            vec![TriggerCondition::StatAtLeast { stat: "warmth".to_string(), value: 50 }],
        );
        let s = serde_json::to_string(&t).unwrap();
        let back: Trigger = serde_json::from_str(&s).unwrap();
        assert_eq!(t, back);
    }
}
