//! st_world_info 测试（R6③ 机械迁入，零改动）。
#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::st_world_info::types::*;

    fn ent(uid: &str, keys: &[&str], content: &str, constant: bool, order: i32) -> WiEntry {
        WiEntry {
            uid: uid.into(),
            world: "test".into(),
            keys: keys.iter().map(|s| s.to_string()).collect(),
            keysecondary: vec![],
            content: content.into(),
            comment: uid.into(),
            constant,
            disable: false,
            selective: false,
            selective_logic: SelectiveLogic::AndAny,
            order,
            position: WiPosition::Before,
            depth: 4,
            probability: 100.0,
            use_probability: false,
            scan_depth: None,
            case_sensitive: None,
            match_whole_words: None,
            group: String::new(),
            group_override: false,
            group_weight: 100.0,
            use_group_scoring: None,
            exclude_recursion: false,
            prevent_recursion: false,
            delay_until_recursion: false,
            ignore_budget: false,
            sticky: None,
            cooldown: None,
            delay: None,
            decorators: Vec::new(),
            outlet_name: String::new(),
            character_filter: None,
            triggers: Vec::new(),
            vectorized: false,
            automation_id: String::new(),
            role: 0,
        }
    }

    #[test]
    fn constant_always_on() {
        let entries = vec![ent("c", &[], "CONST_LORE", true, 10)];
        let chat = vec!["hello".into()];
        let r = check_world_info(&entries, &chat, 4096, &WiSettings::default());
        assert!(r.world_info_before.contains("CONST_LORE"));
    }

    #[test]
    fn key_match_activates() {
        let entries = vec![ent("k", &["storm"], "STORM_LORE", false, 50)];
        let chat = vec!["The storm rolls in.".into()];
        let r = check_world_info(&entries, &chat, 4096, &WiSettings::default());
        assert!(r.world_info_before.contains("STORM_LORE") || r.world_info_after.contains("STORM_LORE") || r.activated.iter().any(|a| a.content.contains("STORM_LORE")));
        // position Before → before
        assert!(format_wi_for_system(&r).contains("STORM_LORE"));
    }

    #[test]
    fn key_miss_skips() {
        let entries = vec![ent("k", &["storm"], "STORM_LORE", false, 50)];
        let chat = vec!["sunny day".into()];
        let r = check_world_info(&entries, &chat, 4096, &WiSettings::default());
        assert!(r.activated.is_empty());
    }

    #[test]
    fn regex_key() {
        let mut e = ent("r", &[r"/\bstorm\b/i"], "RE_LORE", false, 1);
        e.keys = vec![r"/\bstorm\b/i".into()];
        let chat = vec!["Storm ahead".into()];
        let r = check_world_info(&[e], &chat, 4096, &WiSettings::default());
        assert_eq!(r.activated.len(), 1);
    }

    #[test]
    fn secondary_and_any() {
        let mut e = ent("s", &["hero"], "SEC", false, 1);
        e.selective = true;
        e.keysecondary = vec!["sword".into(), "blade".into()];
        e.selective_logic = SelectiveLogic::AndAny;
        let chat = vec!["the hero draws a sword".into()];
        let r = check_world_info(&[e], &chat, 4096, &WiSettings::default());
        assert_eq!(r.activated.len(), 1);
    }

    #[test]
    fn parse_character_book_entries() {
        let book = json!({
            "stBookRaw": {
                "name": "B",
                "entries": [
                    {"keys":["a"],"content":"A","enabled":true,"constant":false,"insertion_order":5},
                    {"keys":["b"],"content":"B","disable":true}
                ]
            }
        });
        let ents = entries_from_world_book("W", Some(&book), "");
        assert_eq!(ents.len(), 2);
        assert!(ents.iter().any(|e| e.disable));
        assert!(ents.iter().any(|e| e.content == "A" && !e.disable));
    }

    #[test]
    fn import_card_world_book_filters_disabled() {
        let card = crate::parse_st_character_card_json(
            r#"{
              "spec": "chara_card_v2",
              "data": {
                "name": "WB Card",
                "description": "d",
                "world_book": {
                  "name": "CardWorldBook",
                  "entries": [
                    {"keys": ["storm", "rain"], "content": "Coast never dries.", "enabled": true, "constant": true, "comment": "weather"},
                    {"keys": ["skip"], "content": "disabled by enabled=false", "enabled": false},
                    {"keys": ["skip2"], "content": "disabled by disable=true", "disable": true},
                    {"keys": ["ok"], "content": "enabled by default"}
                  ]
                }
              }
            }"#,
        )
        .unwrap();
        let ents = import_card_world_book(&card, "W");
        assert_eq!(ents.len(), 2);
        let storm = ents.iter().find(|e| e.keys.contains(&"storm".to_string())).unwrap();
        assert_eq!(storm.content, "Coast never dries.");
        assert!(storm.constant);
        assert!(!storm.disable);
        let ok = ents.iter().find(|e| e.keys.contains(&"ok".to_string())).unwrap();
        assert_eq!(ok.content, "enabled by default");
    }

    #[test]
    fn import_card_world_book_empty_card() {
        let card = crate::parse_st_character_card_json(
            r#"{"spec":"chara_card_v2","data":{"name":"NoBook","description":"d","world_book":{"name":"Empty","entries":[]}}}"#,
        )
        .unwrap();
        assert!(import_card_world_book(&card, "W").is_empty());

        let no_book = crate::parse_st_character_card_json(
            r#"{"spec":"chara_card_v2","data":{"name":"NoBook2","description":"d"}}"#,
        )
        .unwrap();
        assert!(import_card_world_book(&no_book, "W").is_empty());
    }
}
