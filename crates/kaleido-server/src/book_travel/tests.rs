//! book_travel 测试（R6④a 机械迁入，零改动）。
#[cfg(test)]
mod tests {
    use super::super::*;
    // open-session 产物提取：stages → L3/milestones/promises/present/focus 的映射口径
    fn fake_pipeline_result() -> Value {
        json!({
            "title": "雨夜",
            "stages": [
                {"step": "assemble", "result": {"plan": "Entrance 进书；Exit 离书"}},
                {"step": "plan_scene", "result": {"scenePlan": {"activeCharacters": ["沈棠", "林晚"]}}},
                {"step": "summarize_memory", "result": {
                    "summary": "雨夜茶馆初见",
                    "keyChoices": ["收下银锁"],
                    "unresolvedConflicts": ["白梅死因未明"]
                }}
            ]
        })
    }
    #[test]
    fn open_session_extracts_stages() {
        let result = fake_pipeline_result();
        let stages = result.get("stages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let sr = |step: &str| -> Value {
            stages.iter().find(|s| s.get("step").and_then(|v| v.as_str()) == Some(step))
                .and_then(|s| s.get("result")).cloned().unwrap_or(json!({}))
        };
        let mem = sr("summarize_memory");
        assert_eq!(mem.get("summary").and_then(|v| v.as_str()).unwrap(), "雨夜茶馆初见");
        assert_eq!(mem.get("keyChoices").and_then(|v| v.as_array()).unwrap().len(), 1);
        assert_eq!(mem.get("unresolvedConflicts").and_then(|v| v.as_array()).unwrap().len(), 1);
        let plan = sr("plan_scene");
        assert_eq!(plan.get("scenePlan").and_then(|v| v.get("activeCharacters"))
            .and_then(|v| v.as_array()).unwrap().len(), 2);
        let asm = sr("assemble");
        assert!(asm.get("plan").and_then(|v| v.as_str()).unwrap().contains("Entrance"));
    }
    #[test]
    fn open_session_empty_result_defaults() {
        let result = json!({"title": "空"});
        assert!(result.get("stages").and_then(|v| v.as_array()).is_none());
    }
}
