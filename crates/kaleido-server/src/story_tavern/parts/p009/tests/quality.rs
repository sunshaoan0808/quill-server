//! p009 tests · quality（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;
use crate::story_tavern::parts::*;


    // ─── P0 写作三档（quality）单元测试 ───────────────────────────────────

    // 本地 mock LLM：预置响应序列，记录调用阶段（review/fix/plan/write/gate）。
    pub(crate) struct MockLlm {
        responses: std::cell::RefCell<std::collections::VecDeque<String>>,
        log: std::cell::RefCell<Vec<String>>,
    }

    impl MockLlm {
        fn new(responses: Vec<&str>) -> Self {
            MockLlm {
                responses: std::cell::RefCell::new(
                    responses.into_iter().map(String::from).collect(),
                ),
                log: std::cell::RefCell::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<String> {
            self.log.borrow().clone()
        }
    }

    impl QualityLlm for MockLlm {
        fn call(
            &self,
            system: &str,
            _user: &str,
        ) -> std::pin::Pin<Box<dyn futures_util::Future<Output = Result<String, String>> + Send + '_>>
        {
            let phase = if system.contains("审稿人") {
                "review"
            } else if system.contains("内容负责人") {
                "fix"
            } else if system.contains("剧情策划") {
                "plan"
            } else if system.contains("终检评审") {
                "gate"
            } else if system.contains("正文作者") {
                "write"
            } else if system.contains("状态补丁") {
                "memory"
            } else {
                "other"
            };
            self.log.borrow_mut().push(phase.to_string());
            let resp = self
                .responses
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| "{\"pass\":true}".to_string());
            Box::pin(async move { Ok(resp) })
        }
    }

    #[test]
    pub(crate) fn test_quality_default_is_lite() {
        assert_eq!(resolve_turn_quality(None), TurnQuality::Lite);
        assert_eq!(
            resolve_turn_quality(Some(TurnQuality::Standard)),
            TurnQuality::Standard
        );
        assert_eq!(
            resolve_turn_quality(Some(TurnQuality::Heavy)),
            TurnQuality::Heavy
        );
    }

    #[test]
    pub(crate) fn test_quality_old_request_body_compat() {
        // 旧请求体无 quality 字段 → 兼容解析为 None → lite
        let req: TurnStartRequest = serde_json::from_value(json!({"message": "你好"}))
            .expect("old body without quality must parse");
        assert!(req.quality.is_none());
        assert_eq!(resolve_turn_quality(req.quality), TurnQuality::Lite);
        // 新请求体显式 standard/heavy/非法值
        let req2: TurnStartRequest = serde_json::from_value(
            json!({"message": "x", "quality": "standard"}),
        )
        .expect("standard quality must parse");
        assert_eq!(req2.quality, Some(TurnQuality::Standard));
        let req3: TurnStartRequest =
            serde_json::from_value(json!({"message": "x", "quality": "heavy"}))
                .expect("heavy quality must parse");
        assert_eq!(req3.quality, Some(TurnQuality::Heavy));
        let req4: TurnStartRequest =
            serde_json::from_value(json!({"message": "x", "quality": "weird"}))
                .expect("unknown quality must fall back to lite");
        assert_eq!(resolve_turn_quality(req4.quality), TurnQuality::Lite);
    }

    #[test]
    pub(crate) fn test_plan_quality_stages_semantics() {
        // lite = 单次直出，不启动审稿/修稿子流程
        assert_eq!(plan_quality_stages(TurnQuality::Lite), vec![QualityStage::Write]);
        // standard = 初稿 → 审稿 → 修订
        let std_stages = plan_quality_stages(TurnQuality::Standard);
        assert!(std_stages.contains(&QualityStage::Write));
        assert!(std_stages.contains(&QualityStage::Review));
        assert!(std_stages.contains(&QualityStage::Fix));
        assert!(!std_stages.contains(&QualityStage::FinalGate));
        assert_eq!(std_stages.len(), 3);
        // heavy = 全管道（round 内执行；P4 起 MemoryPatch 归入管道末尾）
        let heavy_stages = plan_quality_stages(TurnQuality::Heavy);
        assert!(heavy_stages.contains(&QualityStage::ContextPlan));
        assert!(heavy_stages.contains(&QualityStage::Write));
        assert!(heavy_stages.contains(&QualityStage::Review));
        assert!(heavy_stages.contains(&QualityStage::Fix));
        assert!(heavy_stages.contains(&QualityStage::FinalGate));
        assert!(
            heavy_stages.contains(&QualityStage::MemoryPatch),
            "P4: heavy 管道末尾含 MemoryPatch（产出可应用 patch）"
        );
        // 上限防死循环
        assert_eq!(QUALITY_MAX_FIX_ROUNDS, 2);
    }

    #[tokio::test]
    pub(crate) async fn test_quality_pipeline_lite_keeps_single_path() {
        let mock = MockLlm::new(vec![]);
        let (out, memory_patch) = run_quality_refine(
            TurnQuality::Lite,
            &mock,
            "sys",
            "user",
            "初稿正文",
            None,
            &mut String::new(),
        )
        .await
        .expect("lite path must succeed");
        assert_eq!(out, "初稿正文");
        assert!(
            memory_patch.is_none(),
            "lite 不产出 memory patch"
        );
        assert!(
            mock.calls().is_empty(),
            "lite 不得发起任何额外 LLM 调用，实际：{:?}",
            mock.calls()
        );
    }

    #[test]
    pub(crate) fn strip_lite_reasoning_leak_keeps_normal_narrative() {
        // 正常结构化回合（含 <场景 标签）与干净叙事不得误伤。
        let tagged = "<场景：雨巷旧茶馆｜深夜>\n\n雨声在门外绵密地落着。";
        assert_eq!(strip_lite_reasoning_leak(tagged.to_string()), tagged);
        let clean = "雨声像一件没缝完的衣裳，继续在屋檐上落着。\n\n林晚松开了你的衣襟。";
        assert_eq!(strip_lite_reasoning_leak(clean.to_string()), clean);
    }

    #[test]
    pub(crate) fn strip_lite_reasoning_leak_strips_now_need_variant() {
        // [fix 2026-08-16] flash-free「现在我需要/XU需要」开头思考：reasoning_content
        // 只装第一段，后续规划段混进 content——旧表缺这些前缀 → 不剥离。现在应剥掉
        // 思考段，保留真实叙事段。
        let leaked = "现在我需要自然地融入这个事件卡，但要注意它只是背景素材。\n\n\
            我环顾四周，观察了这个蜜月套房的环境——木质地板、海风、大床。\n\n\
            妈妈从卫生间出来，我们简短地交谈了几句。\n\n\
            我需要继续推进这个场景，保持暧昧氛围。";
        let out = strip_lite_reasoning_leak(leaked.to_string());
        assert!(out.starts_with("我环顾四周"), "应保留叙事段，实际: {:?}", &out[..30.min(out.len())]);
        assert!(!out.contains("现在我需要"), "思考段应被剥离");
        assert!(!out.contains("我需要继续"), "尾部思考应被剥离");
    }

    #[test]
    pub(crate) fn strip_trailing_metadiscourse_strips_now_write_variant() {
        // [fix 2026-08-16 实踩]「嗯，这样应该可以。现在让我把完整的叙事写出来。」
        // +「还要注意：- 正文末尾需要角色清单」——正文后的自检尾巴。
        let leaked = "替她拨开，指尖却故意在她耳垂上多停了一秒，看她会不会脸红。\n\n\
            嗯，这样应该可以。现在让我把完整的叙事写出来。\n\n\
            还要注意：\n- 正文末尾需要角色清单";
        let out = strip_trailing_metadiscourse(leaked.to_string());
        assert!(out.contains("看她会不会脸红"), "正文应保留");
        assert!(!out.contains("这样应该可以"), "尾部元话语应剥离");
        assert!(!out.contains("还要注意"), "清单段应剥离");
    }

    #[test]
    pub(crate) fn strip_lite_reasoning_leak_keeps_short_sentence_narrative() {
        // [fix 2026-08-16 剥离误杀] 中文正文短句段（「走过去。」4字、「没有说话。」
        // 5字）不应被当思考剥掉。旧阈值 len>=10 把整篇短句正文剥光只剩思考尾
        // （实踩 21:05 回合 content=42 字）。阈值降至 2 后应保留正文。
        let leaked = "让我理清一下当前状态。\n\n我走过去。\n\n从背后轻轻抱住她。\n\n她没有说话。\n\n海风把她的发梢吹到我的指间。\n\n最后，让我确认一下【节点推进】标记。";
        let out = strip_lite_reasoning_leak(leaked.to_string());
        assert!(out.contains("我走过去"), "短句正文应保留，实际: {:?}", &out[..40.min(out.len())]);
        assert!(out.contains("轻轻抱住她"), "正文应保留");
        assert!(out.contains("发梢吹到"), "正文应保留");
        assert!(!out.starts_with("让我理清"), "开头思考应被剥离");
        assert!(!out.contains("让我确认一下"), "尾部思考应被剥离");
    }

    #[test]
    pub(crate) fn strip_lite_reasoning_leak_extracts_clean_body() {
        // 无标签思维链泄漏：推理段包裹正文且重复多次，取最长连续叙事段组。
        let leaked = "根据章节大纲，这一章是“雨巷来客”。\n\n让我重新阅读一下提示。\n\n\
            雨声像一件没缝完的衣裳，继续在屋檐上落着。\n\n林晚松开了你的衣襟，手指没有完全离开。\n\n\
            「天快亮了。」她说，声音有些哑。\n\n可她没有动，也没有让你去拿伞。\n\n\
            然后提供选项。角色清单：沈棠、林晚。";
        let out = strip_lite_reasoning_leak(leaked.to_string());
        // 最长连续叙事组 = 雨声→可她没有动（4 段），尾部元话语段被剔除。
        assert!(out.contains("雨声像一件没缝完的衣裳"), "应保留正文：{out}");
        assert!(out.contains("可她没有动"), "应保留正文尾部：{out}");
        assert!(!out.contains("根据章节大纲"), "推理段应剔除：{out}");
        assert!(!out.contains("然后提供选项"), "尾部元话语应剔除：{out}");
        assert!(!out.contains("角色清单"), "角色清单应剔除：{out}");
        assert!(!out.contains("让我重新阅读"), "思维段应剔除：{out}");
    }

    #[test]
    pub(crate) fn strip_lite_reasoning_leak_too_short_keeps_original() {
        // 无可剥离正文（叙事段不足）→ 保守返回原文。
        let only_meta = "让我重新阅读一下提示。\n\n让我再检查一次。";
        assert_eq!(strip_lite_reasoning_leak(only_meta.to_string()), only_meta);
    }

    #[tokio::test]
    pub(crate) async fn test_quality_pipeline_standard_triggers_review_and_fix() {
        let mock = MockLlm::new(vec!["审稿：人物声线不稳", "修订后的完整正文"]);
        let (out, _patch) = run_quality_refine(
            TurnQuality::Standard,
            &mock,
            "sys",
            "user",
            "初稿正文",
            None,
            &mut String::new(),
        )
        .await
        .expect("standard path must succeed");
        assert_eq!(out, "修订后的完整正文");
        assert_eq!(mock.calls(), vec!["review".to_string(), "fix".to_string()]);
    }

    #[tokio::test]
    pub(crate) async fn test_quality_pipeline_standard_strips_fix_thinking() {
        // fix 输出带 <thinking>/<story> 结构 → 正文纯净化，思维汇入 thinking_out 折叠。
        let mock = MockLlm::new(vec![
            "审稿：人物声线不稳",
            "<thinking>我要根据审稿意见逐条修订，先分析声线问题…</thinking>\n\n<story>修订后的完整正文</story>",
        ]);
        let mut thinking_out = String::new();
        let (out, _patch) = run_quality_refine(
            TurnQuality::Standard,
            &mock,
            "sys",
            "user",
            "初稿正文",
            None,
            &mut thinking_out,
        )
        .await
        .expect("standard path must succeed");
        assert_eq!(out, "修订后的完整正文");
        assert!(thinking_out.contains("逐条修订"), "思维段应汇入折叠区");
    }

    #[tokio::test]
    pub(crate) async fn test_quality_pipeline_heavy_rounds_capped() {
        // gate 永远不通过 → 循环必须在上限 QUALITY_MAX_FIX_ROUNDS=2 处截断，防死循环。
        // [fix 2026-08-15] memory 阶段响应改为合法 patch JSON（原数据误用 gate verdict
        // {"pass":false,...} → 触发 P4.1 repair 修复循环 → 多一次 "other" 调用 → 断言红；
        // 该失败为既有（HEAD 8376ed55 上即红），随本批顺手根治）。
        let mock = MockLlm::new(vec![
            "计划", "正文", "审稿意见", "第一版修订",
            "{\"pass\":false,\"problems\":\"连续性差\"}",
            "第二版修订",
            "{\"pass\":false,\"problems\":\"仍不稳\"}",
            "第三版修订",
            "{\"progress\":\"推进主线\",\"character_state\":{},\"world_state\":\"已进入矿区\",\"foreshadowing\":\"矿洞深处有异常\"}",
        ]);
        let (out, memory_patch) = run_quality_refine(
            TurnQuality::Heavy,
            &mock,
            "sys",
            "user",
            "初稿正文",
            None,
            &mut String::new(),
        )
        .await
        .expect("heavy path must succeed");
        assert_eq!(out, "第三版修订");
        let calls = mock.calls();
        let gate_calls = calls.iter().filter(|c| *c == "gate").count();
        let fix_calls = calls.iter().filter(|c| *c == "fix").count();
        assert_eq!(
            gate_calls, 2,
            "gate 调用必须被上限截断（≤2），实际：{calls:?}"
        );
        assert_eq!(fix_calls, 3, "初始修订+2次重修，实际：{calls:?}");
        // 阶段顺序：plan → write → review → fix → gate (→ fix → gate) → memory
        assert_eq!(calls[0], "plan");
        assert_eq!(calls[1], "write");
        assert_eq!(calls[2], "review");
        assert_eq!(
            calls.last().map(|s| s.as_str()),
            Some("memory"),
            "heavy 管道末尾须执行 MemoryPatch：{calls:?}"
        );
        assert_eq!(
            memory_patch,
            Some("{\"progress\":\"推进主线\",\"character_state\":{},\"world_state\":\"已进入矿区\",\"foreshadowing\":\"矿洞深处有异常\"}".to_string())
        );
    }

    #[tokio::test]
    pub(crate) async fn test_quality_pipeline_heavy_gate_passes_stops_early() {
        let mock = MockLlm::new(vec![
            "计划",
            "正文",
            "审稿意见",
            "达标修订",
            "{\"pass\":true}",
            "{\"progress\":\"...\",\"character_state\":{},\"world_state\":\"...\",\"foreshadowing\":\"...\"}",
        ]);
        let (out, memory_patch) = run_quality_refine(
            TurnQuality::Heavy,
            &mock,
            "sys",
            "user",
            "初稿正文",
            None,
            &mut String::new(),
        )
        .await
        .expect("heavy path must succeed");
        assert_eq!(out, "达标修订");
        let calls = mock.calls();
        assert_eq!(calls.iter().filter(|c| *c == "gate").count(), 1);
        assert_eq!(calls.iter().filter(|c| *c == "fix").count(), 1);
        assert!(memory_patch.is_some(), "heavy 达标后仍产出 memory patch");
    }

    #[tokio::test]
    pub(crate) async fn test_quality_pipeline_heavy_memory_patch_failure_keeps_text() {
        // MemoryPatch LLM 失败 → 只记日志，正文不丢。
        let mock = MockLlm::new(vec![
            "计划",
            "正文",
            "审稿意见",
            "达标修订",
            "{\"pass\":true}",
        ]);
        let (out, memory_patch) = run_quality_refine(
            TurnQuality::Heavy,
            &mock,
            "sys",
            "user",
            "初稿正文",
            None,
            &mut String::new(),
        )
        .await
        .expect("heavy path must succeed");
        // 响应耗尽后 MockLlm pop_front 返回 None → memory 阶段失败（模拟上游超时）
        assert_eq!(out, "达标修订");
        assert!(memory_patch.is_none() || memory_patch.as_deref() == Some("{\"pass\":true}"));
    }

    #[test]
    pub(crate) fn test_quality_gate_passes_parse() {
        assert!(gate_passes("{\"pass\":true}"));
        assert!(!gate_passes("{\"pass\":false,\"problems\":\"连续性差\"}"));
        assert!(gate_passes("{\"pass\": false}") == false);
        assert!(!gate_passes("some non-json"), "无法解析时 fail-closed");
        assert!(gate_passes("```json\n{\"pass\":true}\n```"), "围栏包裹的 JSON 应可解析");
        assert!(gate_passes("结论：{\"pass\":true}。以上。"), "前后缀文本容忍");
        assert!(!gate_passes("{\"pass\":null}"), "pass 非布尔视为未通过");
        assert!(!gate_passes("{\"problems\":\"缺 pass 字段\"}"), "缺 pass 字段视为未通过");
    }
