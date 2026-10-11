//! p006 测试集 · actor_state（R6 自 p006.rs 机械迁入，仅 use 路径改绝对）
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
mod actor_state_tests {
    use crate::story_tavern::parts::*;

    fn field(v: Value) -> ActorFieldValue {
        ActorFieldValue {
            value_type: "number".into(),
            value: Some(v),
            min: Some(0.0),
            max: Some(100.0),
            options: vec![],
            display: Some("血".into()),
            update_instruction: Some("受击时下降".into()),
            updated_by: String::new(),
            updated_turn: 0,
            updated_reason: String::new(),
        }
    }

    fn trait_inst(pool: &str, id: &str, name: &str) -> ActorTraitInstance {
        ActorTraitInstance {
            pool_id: pool.into(),
            pool_name: None,
            trait_id: id.into(),
            name: name.into(),
            summary: Some("简述".into()),
            source_turn_id: None,
        }
    }

    #[test]
    fn build_context_text_empty_is_empty_string() {
        let sys = ActorStateSystem::default();
        assert!(sys.build_context_text().is_empty());
    }

    #[test]
    fn build_context_text_includes_character_and_fields() {
        let mut sys = ActorStateSystem::default();
        sys.actors.insert(
            "hero".into(),
            ActorStateEntry {
                template_id: "t1".into(),
                fields: [("hp".into(), field(json!(80)))]
                    .into_iter()
                    .collect(),
                traits: vec![trait_inst("p1", "tr1", "热血")],
            },
        );
        let text = sys.build_context_text();
        assert!(text.contains("hero"));
        assert!(text.contains("hp"));
        assert!(text.contains("80"));
        assert!(text.contains("热血"));
        // update_instruction 应出现在备注里
        assert!(text.contains("受击时下降"));
    }

    #[test]
    fn build_context_text_injects_compass_block() {
        let mut sys = ActorStateSystem::default();
        sys.mount_compass(Compass::new("主角必须活到最后", "本周写完第三章"));
        let text = sys.build_context_text();
        assert!(text.contains("## 创作罗盘"));
        assert!(text.contains("【全书承诺】主角必须活到最后"));
        assert!(text.contains("【近期目标】本周写完第三章"));
        // 无角色时仅输出罗盘段。
        assert!(text.starts_with("## 创作罗盘\n"));
    }

    #[test]
    fn build_context_text_compass_before_characters() {
        let mut sys = ActorStateSystem::default();
        sys.mount_compass(Compass::new("全书承诺 A", ""));
        sys.actors.insert(
            "hero".into(),
            ActorStateEntry {
                template_id: "t1".into(),
                fields: [("hp".into(), field(json!(80)))]
                    .into_iter()
                    .collect(),
                traits: vec![trait_inst("p1", "tr1", "热血")],
            },
        );
        let text = sys.build_context_text();
        // 罗盘段置顶，角色信息在其后。
        assert!(text.starts_with("## 创作罗盘\n"));
        let compass_end = text.find("【全书承诺】").unwrap();
        let hero_pos = text.find("# hero").unwrap();
        assert!(compass_end < hero_pos, "compass must appear before characters");
        // 空字段（current_focus）不输出。
        assert!(!text.contains("【近期目标】"));
        assert!(text.contains("hp"));
    }

    #[test]
    fn build_context_text_empty_compass_not_injected() {
        let mut sys = ActorStateSystem::default();
        sys.actors.insert(
            "hero".into(),
            ActorStateEntry {
                template_id: "t1".into(),
                fields: Default::default(),
                traits: vec![],
            },
        );
        let text = sys.build_context_text();
        assert!(!text.contains("创作罗盘"));
        assert!(!text.contains("【全书承诺】"));
        assert!(!text.contains("【近期目标】"));
        assert!(text.contains("hero"));
    }

    #[test]
    fn actor_system_compass_survives_json_roundtrip() {
        let mut sys = ActorStateSystem::default();
        sys.mount_compass(Compass::new("承诺", "目标"));
        let raw = serde_json::to_string(&sys).unwrap();
        let back: ActorStateSystem = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.compass.author_intent, "承诺");
        assert_eq!(back.compass.current_focus, "目标");

        // 旧存档无 compass 字段 → 反序列化为空罗盘（零回归）。
        let legacy = r#"{"schemaVersion":3,"initialActors":[],"actors":{},"traitPools":[],"archive":[]}"#;
        let legacy_back: ActorStateSystem = serde_json::from_str(legacy).unwrap();
        assert!(legacy_back.compass.is_empty());
    }

    #[test]
    fn apply_updates_upserts_new_character() {
        let mut sys = ActorStateSystem::default();
        let updates = vec![ActorStateUpdate {
            character_id: "villain".into(),
            fields: [("hp".into(), json!(50))].into_iter().collect(),
            add_traits: vec![],
            remove_traits: vec![],
        }];
        let n = sys.apply_updates(&updates);
        assert_eq!(n, 1);
        let entry = sys.actors.get("villain").expect("upserted");
        assert_eq!(entry.fields["hp"].value.as_ref(), Some(&json!(50)));
    }

    #[test]
    fn apply_updates_overwrites_existing_fields() {
        let mut sys = ActorStateSystem::default();
        sys.actors.insert(
            "hero".into(),
            ActorStateEntry {
                template_id: "t1".into(),
                fields: [("hp".into(), field(json!(100)))]
                    .into_iter()
                    .collect(),
                traits: vec![],
            },
        );
        let updates = vec![ActorStateUpdate {
            character_id: "hero".into(),
            fields: [("hp".into(), json!(30))].into_iter().collect(),
            add_traits: vec![],
            remove_traits: vec![],
        }];
        let n = sys.apply_updates(&updates);
        assert_eq!(n, 1);
        assert_eq!(sys.actors["hero"].fields["hp"].value.as_ref(), Some(&json!(30)));
    }

    #[test]
    fn apply_updates_add_traits_ignores_duplicate() {
        let mut sys = ActorStateSystem::default();
        sys.actors.insert(
            "hero".into(),
            ActorStateEntry {
                template_id: "t1".into(),
                fields: [("hp".into(), field(json!(100)))].into_iter().collect(),
                traits: vec![trait_inst("p1", "tr1", "热血")],
            },
        );
        let updates = vec![ActorStateUpdate {
            character_id: "hero".into(),
            fields: [("hp".into(), json!(10))].into_iter().collect(),
            add_traits: vec![trait_inst("p1", "tr1", "热血"), trait_inst("p2", "tr2", "冷静")],
            remove_traits: vec![],
        }];
        let n = sys.apply_updates(&updates);
        assert_eq!(n, 1);
        let traits = &sys.actors["hero"].traits;
        // 已有 tr1 不重复追加；新增 tr2
        assert_eq!(traits.iter().filter(|t| t.trait_id == "tr1").count(), 1);
        assert!(traits.iter().any(|t| t.trait_id == "tr2"));
        assert_eq!(traits.len(), 2);
    }

    #[test]
    fn apply_updates_remove_traits() {
        let mut sys = ActorStateSystem::default();
        sys.actors.insert(
            "hero".into(),
            ActorStateEntry {
                template_id: "t1".into(),
                fields: [("hp".into(), field(json!(100)))].into_iter().collect(),
                traits: vec![trait_inst("p1", "tr1", "热血"), trait_inst("p2", "tr2", "冷静")],
            },
        );
        let updates = vec![ActorStateUpdate {
            character_id: "hero".into(),
            fields: [("hp".into(), json!(10))].into_iter().collect(),
            add_traits: vec![],
            remove_traits: vec!["tr1".into()],
        }];
        let n = sys.apply_updates(&updates);
        assert_eq!(n, 1);
        let traits = &sys.actors["hero"].traits;
        assert!(!traits.iter().any(|t| t.trait_id == "tr1"));
        assert!(traits.iter().any(|t| t.trait_id == "tr2"));
        assert_eq!(traits.len(), 1);
    }

    #[test]
    fn apply_updates_returns_field_count() {
        let mut sys = ActorStateSystem::default();
        let updates = vec![
            ActorStateUpdate {
                character_id: "a".into(),
                fields: [("hp".into(), json!(1)), ("mp".into(), json!(2))]
                    .into_iter()
                    .collect(),
                add_traits: vec![],
                remove_traits: vec![],
            },
            ActorStateUpdate {
                character_id: "a".into(),
                fields: [("hp".into(), json!(9))].into_iter().collect(),
                add_traits: vec![],
                remove_traits: vec![],
            },
            ActorStateUpdate {
                character_id: "b".into(),
                fields: [("sp".into(), json!(3))].into_iter().collect(),
                add_traits: vec![],
                remove_traits: vec![],
            },
        ];
        let n = sys.apply_updates(&updates);
        assert_eq!(n, 4);
        assert_eq!(sys.actors.len(), 2);
    }

    #[test]
    fn apply_updates_skips_empty_character_id() {
        let mut sys = ActorStateSystem::default();
        let updates = vec![ActorStateUpdate {
            character_id: String::new(),
            fields: [("hp".into(), json!(5))].into_iter().collect(),
            add_traits: vec![],
            remove_traits: vec![],
        }];
        let n = sys.apply_updates(&updates);
        assert_eq!(n, 0);
        assert!(sys.actors.is_empty());
    }
}

