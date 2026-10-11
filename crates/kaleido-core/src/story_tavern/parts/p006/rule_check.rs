//! p006 测试集 · rule_check（R6 自 p006.rs 机械迁入，仅 use 路径改绝对）
#![allow(unused_imports)]

use crate::{
    st_compass::{Compass, CompassStore},
    st_skimming::SkimIssue,
    text_hash, CoreError, CoreResult, DataRoot,
};
use crate::world_state::{EntityKind as WsEntityKind, WorldEntity, WorldEvent, WorldState};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};
use uuid::Uuid;
use super::*;


#[cfg(test)]
mod rule_check_tests {
    use crate::story_tavern::parts::*;

    fn req() -> TurnCheckRequest {
        TurnCheckRequest {
            action: "潜行".into(),
            intent: "不被发现接近目标".into(),
            challenge: "走廊有巡逻守卫".into(),
            cost: "被发现后警报响起".into(),
            difficulty: "normal".into(),
            template_id: Some("stealth".into()),
            bonuses: vec![TurnCheckBonus {
                reason: "潜行专长".into(),
                value: 2.0,
            }],
            outcomes: TurnCheckOutcomes {
                critical_success: TurnCheckOutcome {
                    result: "无声无息".into(),
                    state_changes: vec![],
                },
                success: TurnCheckOutcome {
                    result: "成功抵达".into(),
                    state_changes: vec![],
                },
                failure: TurnCheckOutcome {
                    result: "被守卫察觉".into(),
                    state_changes: vec![],
                },
                critical_failure: TurnCheckOutcome {
                    result: "直接暴露".into(),
                    state_changes: vec![],
                },
            },
        }
    }

    #[test]
    fn parse_dice_basic() {
        assert_eq!(parse_dice("1d20"), Some((1, 20)));
        assert_eq!(parse_dice("2d6"), Some((2, 6)));
        assert_eq!(parse_dice("bad"), None);
        assert_eq!(parse_dice(""), None);
    }

    #[test]
    fn difficulty_to_dc_mapping() {
        assert_eq!(difficulty_to_dc("very_easy"), 5.0);
        assert_eq!(difficulty_to_dc("easy"), 8.0);
        assert_eq!(difficulty_to_dc("normal"), 12.0);
        assert_eq!(difficulty_to_dc("hard"), 15.0);
        assert_eq!(difficulty_to_dc("very_hard"), 18.0);
        assert_eq!(difficulty_to_dc("whatever"), 12.0);
    }

    #[test]
    fn roll_check_natural_20_is_critical_success() {
        let r = roll_check(&req(), None, || 20);
        assert_eq!(r.outcome, "critical_success");
        assert_eq!(r.natural, 20);
        assert_eq!(r.result_text, "无声无息");
    }

    #[test]
    fn roll_check_natural_1_is_critical_failure() {
        let r = roll_check(&req(), None, || 1);
        assert_eq!(r.outcome, "critical_failure");
        assert_eq!(r.natural, 1);
        assert_eq!(r.result_text, "直接暴露");
    }

    #[test]
    fn roll_check_hit_and_miss() {
        let mut q = req();
        q.difficulty = "normal".into();
        q.bonuses.clear();
        let hit = roll_check(&q, None, || 12);
        assert_eq!(hit.outcome, "success");
        let miss = roll_check(&q, None, || 11);
        assert_eq!(miss.outcome, "failure");
    }

    #[test]
    fn roll_check_bonuses_add_to_total() {
        let r = roll_check(&req(), None, || 10);
        // dc normal=12，total=10+2=12 → success
        assert_eq!(r.outcome, "success");
        assert_eq!(r.total, 12.0);
        assert_eq!(r.dc, 12.0);
    }

    #[test]
    fn roll_check_template_modifier_raises_dc() {
        let tpl = RuleCheck {
            id: "stealth".into(),
            label: "潜行".into(),
            dice: "1d20".into(),
            modifier: 4.0,
            ..Default::default()
        };
        let r = roll_check(&req(), Some(&tpl), || 12);
        // dc=16，total=14 → failure
        assert_eq!(r.dc, 16.0);
        assert_eq!(r.outcome, "failure");
    }

    #[test]
    fn roll_check_template_state_binding_on_fail() {
        let tpl = RuleCheck {
            id: "stealth".into(),
            label: "潜行".into(),
            dice: "1d20".into(),
            modifier: 0.0,
            state_bindings: vec![RuleStateBinding {
                field: "敌方.压力".into(),
                on_success: None,
                on_fail: Some(json!("+1")),
            }],
            ..Default::default()
        };
        let r = roll_check(&req(), Some(&tpl), || 5);
        assert_eq!(r.outcome, "failure");
        assert!(r.state_changes.iter().any(|c| c.actor_id == "敌方"
            && c.field_id == "压力"
            && c.change == 1.0));
    }

    #[test]
    fn roll_check_result_falls_back_to_template_hint() {
        let tpl = RuleCheck {
            id: "stealth".into(),
            label: "潜行".into(),
            dice: "1d20".into(),
            modifier: 0.0,
            failure_hint: "你弄出了声响，可能引起注意。".into(),
            ..Default::default()
        };
        let mut q = req();
        q.outcomes.failure.result = String::new();
        let r = roll_check(&q, Some(&tpl), || 5);
        assert_eq!(r.outcome, "failure");
        assert_eq!(r.result_text, "你弄出了声响，可能引起注意。");
    }

    #[test]
    fn apply_state_changes_numeric_add_and_ignore_non_numeric() {
        let mut sys = ActorStateSystem::default();
        sys.actors.insert(
            "敌方".into(),
            ActorStateEntry {
                template_id: "t1".into(),
                fields: [
                    (
                        "压力".into(),
                        ActorFieldValue {
                            value_type: "number".into(),
                            value: Some(json!(3)),
                            min: Some(0.0),
                            max: Some(100.0),
                            options: vec![],
                            display: Some("压力".into()),
                            update_instruction: None,
                            updated_by: String::new(),
                            updated_turn: 0,
                            updated_reason: String::new(),
                        },
                    ),
                    (
                        "名字".into(),
                        ActorFieldValue {
                            value_type: "string".into(),
                            value: Some(json!("守卫长")),
                            min: None,
                            max: None,
                            options: vec![],
                            display: None,
                            update_instruction: None,
                            updated_by: String::new(),
                            updated_turn: 0,
                            updated_reason: String::new(),
                        },
                    ),
                ]
                .into_iter()
                .collect(),
                traits: vec![],
            },
        );
        let changes = vec![
            TurnStateChange {
                actor_id: "敌方".into(),
                field_id: "压力".into(),
                change: 1.0,
                reason: "检定".into(),
            },
            TurnStateChange {
                actor_id: "敌方".into(),
                field_id: "名字".into(),
                change: 5.0,
                reason: "不应生效".into(),
            },
            TurnStateChange {
                actor_id: "不存在".into(),
                field_id: "压力".into(),
                change: 1.0,
                reason: "忽略".into(),
            },
        ];
        let n = sys.apply_state_changes(&changes);
        assert_eq!(n, 1);
        assert_eq!(sys.actors["敌方"].fields["压力"].value.as_ref(), Some(&json!(4.0)));
    }

    #[test]
    fn apply_state_changes_numeric_string_value() {
        let mut sys = ActorStateSystem::default();
        sys.actors.insert(
            "敌方".into(),
            ActorStateEntry {
                template_id: "t1".into(),
                fields: [(
                    "压力".into(),
                    ActorFieldValue {
                        value_type: "string".into(),
                        value: Some(json!("10")),
                        min: None,
                        max: None,
                        options: vec![],
                        display: None,
                        update_instruction: None,
                        updated_by: String::new(),
                        updated_turn: 0,
                        updated_reason: String::new(),
                    },
                )]
                .into_iter()
                .collect(),
                traits: vec![],
            },
        );
        let changes = vec![TurnStateChange {
            actor_id: "敌方".into(),
            field_id: "压力".into(),
            change: -2.0,
            reason: "检定".into(),
        }];
        let n = sys.apply_state_changes(&changes);
        assert_eq!(n, 1);
        assert_eq!(sys.actors["敌方"].fields["压力"].value.as_ref(), Some(&json!(8.0)));
    }
}

