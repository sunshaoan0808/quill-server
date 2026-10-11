//! p009 tests · u11（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    // ─── U11: resume / epoch / 成本记账 纯函数单测 ───────────────────────────

    #[test]
    pub(crate) fn u11_should_epoch_compress_char_threshold() {
        // 字符阈值：超 TURN_EPOCH_HARD_CHARS 触发
        let mut sess = test_session_minimal();
        sess.messages = vec![
            TavernMessage {
                id: "m1".into(),
                role: "user".into(),
                content: "啊".repeat(TURN_EPOCH_HARD_CHARS + 100),
                created_at: String::new(),
                options: vec![],
            swipes: vec![],
            swipe_index: 0,
                engine_tag: None,
                program: None,
                reasoning: None,
            tokens: 0,
            },
        ];
        let ctx = estimate_turn_ctx_chars(&sess, "sys", "u", "d");
        assert!(
            should_epoch_compress(&sess, ctx),
            "ctx_chars={ctx} 应触发 epoch 压缩"
        );
    }

    #[test]
    pub(crate) fn u11_should_epoch_compress_message_threshold() {
        let mut sess = test_session_minimal();
        for i in 0..TURN_EPOCH_HARD_MESSAGES {
            sess.messages.push(TavernMessage {
                id: format!("m{i}"),
                role: (if i % 2 == 0 { "user" } else { "assistant" }).into(),
                content: "短消息".into(),
                created_at: String::new(),
                options: vec![],
                engine_tag: None,
                program: None,
                reasoning: None,
            swipes: vec![],
            swipe_index: 0,
            tokens: 0,
            });
        }
        assert!(
            should_epoch_compress(&sess, 100),
            "消息数达 TURN_EPOCH_HARD_MESSAGES 应触发"
        );
        assert!(
            !should_epoch_compress(&test_session_minimal(), 100),
            "空会话不触发"
        );
    }

    #[test]
    pub(crate) fn u11_refine_llm_calls_per_quality() {
        assert_eq!(refine_llm_calls(TurnQuality::Lite), 0);
        assert_eq!(refine_llm_calls(TurnQuality::Standard), 2);
        assert_eq!(refine_llm_calls(TurnQuality::Heavy), 5 + QUALITY_MAX_FIX_ROUNDS as u32 + 1);
    }

    #[test]
    pub(crate) fn u11_turn_cost_estimate_free_model_zero() {
        let est = turn_cost_estimate("deepseek-v4-flash-free", TurnQuality::Lite, "sys", "user", "draft", 0, false);
        assert_eq!(est.est_cost_usd, 0.0);
        assert_eq!(est.llm_calls, 1);
        assert!(est.est_in_tokens > 0, "非空输入 token 估算至少为 1");
    }

    #[test]
    pub(crate) fn u11_turn_cost_estimate_paid_positive() {
        let sys = "你是一个故事主持人（DM/GM）。".repeat(20);
        let user = "玩家输入：向森林深处走去，寻找传说中的古井。";
        let draft = "夜色如墨，林间小径在月光下泛着银光。".repeat(30);
        let est = turn_cost_estimate("deepseek-chat", TurnQuality::Standard, &sys, user, &draft, 1, true);
        // 主流(1) + 回退(1) + standard 审稿修订(2) + 非阻塞(1) = 5
        assert_eq!(est.llm_calls, 5);
        assert!(est.est_in_tokens > 0);
        assert!(est.est_out_tokens > 0);
        assert!(est.est_cost_usd > 0.0);
    }

    #[test]
    pub(crate) fn u11_accounting_json_shape() {
        let v = u11_accounting_json("m", TurnQuality::Lite, "s", "u", "d", 0, false, 1234, true, 2, Some(9), Some("boom"));
        let u = &v["u11"];
        assert_eq!(u["durationMs"], json!(1234));
        assert_eq!(u["resumed"], json!(true));
        assert_eq!(u["epoch"], json!(2));
        assert_eq!(u["turn"], json!(9));
        assert_eq!(u["error"], json!("boom"));
        assert!(u["llmCalls"].as_u64().unwrap() >= 1);
    }

    #[test]
    pub(crate) fn u11_turn_over_budget_time_check() {
        // 纯函数看门狗：预算 1s，起点 5s 前 → 超时；预算 100s → 未超时
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(turn_over_budget(now_ms - 5_000, 1_000));
        assert!(!turn_over_budget(now_ms, 100_000));
    }

    // ---- U11 阈值参数化单测 (2026-08-27) ----

    #[test]
    pub(crate) fn u11_tuning_default_matches_consts() {
        let t = U11Tuning::default();
        assert_eq!(t.epoch_hard_chars, TURN_EPOCH_HARD_CHARS);
        assert_eq!(t.epoch_hard_messages, TURN_EPOCH_HARD_MESSAGES);
        assert_eq!(t.hard_timeout_secs, TURN_HARD_TIMEOUT_SECS);
        assert_eq!(t.max_fix_rounds, QUALITY_MAX_FIX_ROUNDS);
    }

    #[test]
    pub(crate) fn u11_should_epoch_compress_with_explicit_thresholds() {
        let mut sess = test_session_minimal();
        sess.messages.push(TavernMessage {
            id: "m1".into(),
            role: "user".into(),
            content: "啊啊啊啊啊啊啊啊".into(),
            created_at: String::new(),
            options: vec![],
            swipes: vec![],
            swipe_index: 0,
            engine_tag: None,
            program: None,
            reasoning: None,
            tokens: 0,
        });
        // 调低字符阈值为 5 → 触发；调高消息阈值为 999 → 仅字符侧触发
        assert!(should_epoch_compress_with(&sess, 10, 5, 999));
        // 字符未达阈值、消息 1 < 2 → 不触发
        assert!(!should_epoch_compress_with(&sess, 10, 100, 2));
    }
