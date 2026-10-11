//! convert 测试（R5 自 convert.rs 迁入，内容零改动）
#![allow(unused_imports)]
use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    /// 角色名合法性过滤：历史乱码名全部拦截，真名/称呼全部放行。
    /// 注：「湮风通」「人里克斯」这类无虚词罕见组合由第二层兜底拦截
    /// （auto cast 正文存在性 / 角色谱证据检索），纯规则层不拦。
    #[test]
    fn plausible_name_rejects_garbage_fragments() {
        let garbage = [
            "冲进下水", "过电车轨", "向越过街", "里有一种", "像是想", "下拉出一", "那种味",
            "那就", "有自己知", "示意我别", "尽管我知", "另一个", "纷纷询", "粮食", "留下一",
            "听早早", "身后", "突然", "颜色",
            "",
            "abcdefghijklm", // 13 字超长
            "林小宇、苏婉", // 带标点
        ];
        for g in garbage {
            assert!(!is_plausible_character_name(g), "应拦截乱码名: {g}");
        }
    }

    #[test]
    fn plausible_name_keeps_real_names() {
        let real = [
            "林小宇", "苏婉", "林远", "林逸", "陆清韵", "林父", "光头男", "沈棠知", "苏早",
            "外婆", "弗雷德", "王一博", "李进", "妈妈", "爸爸", "老板", "小美", "老王",
            "向明初", "莫旺财", "山楂", "冯婷", "浓妆女人",
        ];
        for r in real {
            assert!(is_plausible_character_name(r), "应放行真名: {r}");
        }
    }

    /// 余弦相似度：同向高分、垂直为 0、维度不一致为 0。
    #[test]
    fn cosine_sim_basic() {
        assert_eq!(cosine_sim(&[], &[]), 0.0);
        assert_eq!(cosine_sim(&[1.0], &[1.0, 2.0]), 0.0);
        assert!((cosine_sim(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!((cosine_sim(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
        assert!((cosine_sim(&[1.0, 0.0], &[0.0, 1.0])).abs() < 1e-6);
    }

    /// card_to_ref 能解析女娲/仓颉三维认知字段（mental_models / decision_heuristics / beliefs），
    /// 且缺失时安全回退为空数组（向后兼容旧 LLM 输出）。
    #[test]
    fn card_to_ref_parses_nuwa_cangjie_fields() {
        let card = serde_json::json!({
            "name": "沈棠",
            "identity": "主角",
            "personality": "谨慎嘴硬心软",
            "speech_style": "短句",
            "evidence_refs": ["ch01:block3"],
            "mental_models": ["棋手:把人心当棋盘（ch02）"],
            "decision_heuristics": ["先找退路:遇险先保后路再谈成败（ch04）"],
            "beliefs": ["规则大于人情:因师门教训形成（ch03）"],
            "opening_scene": "暮色书斋，烛火明灭，窗外的雨敲在青石板上。",
            "opening_lines": "「沈棠在这里候你多时。」她合上手中的卷册，眉眼抬向你。",
            "nsfw_profile": "露骨→禁止；接吻以上→nsfw；日常对白→non",
        });
        let c = card_to_ref(0, "沈棠", "high", &card);
        assert_eq!(c.mental_models, vec!["棋手:把人心当棋盘（ch02）".to_string()]);
        assert_eq!(c.decision_heuristics, vec!["先找退路:遇险先保后路再谈成败（ch04）".to_string()]);
        assert_eq!(c.beliefs, vec!["规则大于人情:因师门教训形成（ch03）".to_string()]);
        assert_eq!(c.role, "主角");
        assert_eq!(c.opening_scene, "暮色书斋，烛火明灭，窗外的雨敲在青石板上。");
        assert!(c.opening_lines.contains("沈棠在这里候你多时"), "first_mes 解析, got: {}", c.opening_lines);
        assert_eq!(c.nsfw_profile, "露骨→禁止；接吻以上→nsfw；日常对白→non");

        // 缺省时安全回退
        let bare = serde_json::json!({ "name": "无名", "identity": "" });
        let b = card_to_ref(1, "无名", "low", &bare);
        assert!(b.mental_models.is_empty());
        assert!(b.decision_heuristics.is_empty());
        assert!(b.beliefs.is_empty());
        assert!(b.nsfw_profile.is_empty(), "缺省回退为空串");
    }

    /// top-k 排序正确：相似度最高的排最前，并按 index 对齐。
    #[test]
    fn cosine_top_k_ranks_desc() {
        let blocks = vec!["苹果 苹果 苹果", "香蕉 香蕉", "苹果", "橘子 橘子 橘子 橘子"]
            .into_iter()
            .map(|t| EvidenceBlock { chapter: "ch01".into(), text: t.into() })
            .collect::<Vec<_>>();
        let query = block_signature("苹果");
        let hits = cosine_top_k(&query, &blocks, 4);
        assert_eq!(hits.len(), 4);
        // 严格降序
        for w in hits.windows(2) {
            assert!(w[0].1 >= w[1].1);
        }
        // 与查询完全同构的块（单次"苹果"）相似度最高 → 应排第一
        assert_eq!(hits[0].0, 2);
        // 纯相关（多次"苹果"+空格稀释方向）仍高于完全不相关块
        let idx0 = hits.iter().position(|(i, _)| *i == 0).unwrap();
        let idx3 = hits.iter().position(|(i, _)| *i == 3).unwrap();
        assert!(idx0 < idx3);
    }

    /// top-k 截断：只返回前 k 个。
    #[test]
    fn cosine_top_k_truncates() {
        let blocks = vec![
            EvidenceBlock { chapter: "ch01".into(), text: "a b c d e f g h".into() },
            EvidenceBlock { chapter: "ch02".into(), text: "1 2 3 4 5 6 7 8".into() },
            EvidenceBlock { chapter: "ch03".into(), text: "! @ # $ % ^ & *".into() },
            EvidenceBlock { chapter: "ch04".into(), text: "q w e r t y u i".into() },
        ];
        let query = block_signature("a b c d e f g h");
        let hits = cosine_top_k(&query, &blocks, 2);
        assert_eq!(hits.len(), 2);
        // k 超过块数 → 全部返回
        let all = cosine_top_k(&query, &blocks, 99);
        assert_eq!(all.len(), 4);
    }

    /// 分块边界：~800 字重叠 100 字的切分行为。
    #[test]
    fn split_blocks_boundaries() {
        // 长度 < block size → 单块（整段）
        let short = "短".repeat(50);
        let blocks = split_into_blocks(&short, BLOCK_SIZE, BLOCK_OVERLAP);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].chars().count(), 50);

        // 长度 2000 字 → 应有约 3 块（步长 700）
        let long = "长".repeat(2000);
        let blocks = split_into_blocks(&long, BLOCK_SIZE, BLOCK_OVERLAP);
        assert!(blocks.len() >= 2);
        // 相邻块应当重叠（相邻块最多差 block_size-overlap 个新增字符）
        for b in &blocks {
            assert!(!b.is_empty());
            assert!(b.chars().count() <= BLOCK_SIZE);
        }
        // 空输入
        assert!(split_into_blocks("", BLOCK_SIZE, BLOCK_OVERLAP).is_empty());
    }

    /// 从召回证据字符串提取章节代号。
    #[test]
    fn extract_evidence_chapters_dedup() {
        let ev = "【ch01】第一段\n\n【ch02】第二段\n【ch01】更多\n";
        let tags = extract_evidence_chapters(ev);
        assert_eq!(tags, vec!["ch01".to_string(), "ch02".to_string()]);
    }

    /// 角色卡重试：mock LLM 首次返回空、第二次返回合法 JSON，应产出该角色卡。
    #[tokio::test]
    async fn retry_character_card_yields_on_second_response() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let card_v = distill_one_character_card("沈棠", "medium", {
            let calls = std::rc::Rc::clone(&calls);
            move || {
                let calls = std::rc::Rc::clone(&calls);
                async move {
                    let n = calls.get();
                    calls.set(n + 1);
                    if n == 0 {
                        Err("extraction empty response".into())
                    } else {
                        Ok("{\"name\":\"沈棠\",\"personality\":\"谨慎嘴硬心软\"}".into())
                    }
                }
            }
        })
        .await;
        let card_v = card_v.expect("首次空响应后重试应产出合法 JSON 卡");
        assert_eq!(calls.get(), 2);
        assert_eq!(card_v["name"], "沈棠");
        assert_eq!(card_v["personality"], "谨慎嘴硬心软");
    }

    // 角色卡重试：mock LLM 恒空，重试 3 次后应返回 None 且不 panic（单角色被跳过）。
    #[tokio::test]
    async fn retry_character_card_skips_on_persistent_empty() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let card_v = distill_one_character_card("路人甲", "low", {
            let calls = std::rc::Rc::clone(&calls);
            move || {
                let calls = std::rc::Rc::clone(&calls);
                async move {
                    calls.set(calls.get() + 1);
                    Ok(String::new())
                }
            }
        })
        .await;
        assert!(card_v.is_none(), "恒空响应下应跳过该角色");
        assert_eq!(calls.get(), 4, "low 角色 fffd 配额 4 次（83746baf 3→4），应恰好在 4 次后放弃");
    }

    // 角色卡重试：mock LLM 首次返回含 U+FFFD 损坏字的 JSON、第二次返回干净 JSON，
    // 应丢弃损坏卡重试并产出干净卡（修复上游偶发把单字采样成 FFFD）。
    #[tokio::test]
    async fn retry_character_card_skips_fffd_and_yields_clean() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let card_v = distill_one_character_card("沈棠", "high", {
            let calls = std::rc::Rc::clone(&calls);
            move || {
                let calls = std::rc::Rc::clone(&calls);
                async move {
                    let n = calls.get();
                    calls.set(n + 1);
                    if n == 0 {
                        Ok("{\"name\":\"沈棠\",\"personality\":\"对弗\u{FFFD}\u{FFFD}德的信任\"}"
                            .into())
                    } else {
                        Ok("{\"name\":\"沈棠\",\"personality\":\"对弗雷德的信任\"}".into())
                    }
                }
            }
        })
        .await;
        let card_v = card_v.expect("含 FFFD 损坏字重试后应产出干净 JSON 卡");
        assert_eq!(calls.get(), 2, "首次损坏应触发重试");
        assert_eq!(card_v["personality"], "对弗雷德的信任");
    }

    // ─── Stage 3 纯函数测试 ─────────────────────────────────────────────────

    /// 世界树条目缺字段兜底：permanent/tags/importance/type/confidence 均有默认值。
    #[test]
    fn normalize_world_lore_defaults() {
        let v = json!({"title": "青云宗", "text": "以剑修为主的宗门。"});
        let e = normalize_world_lore_entry(&v).expect("title+text 应有条目");
        assert_eq!(e["title"], "青云宗");
        assert_eq!(e["text"], "以剑修为主的宗门。");
        assert_eq!(e["tags"].as_array().map(|a| a.len()), Some(0));
        assert_eq!(e["permanent"], true);
        assert_eq!(e["importance"], "low");
        assert_eq!(e["type"], "setting");
        assert_eq!(e["confidence"].as_f64().unwrap(), 0.5);
    }

    /// 世界树条目缺 title/text 不落库。
    #[test]
    fn normalize_world_lore_rejects_missing_core() {
        assert!(normalize_world_lore_entry(&json!({"title": "无文本"})).is_none());
        assert!(normalize_world_lore_entry(&json!({"text": "无标题"})).is_none());
        assert!(normalize_world_lore_entry(&json!({"title": "  ", "text": "  "})).is_none());
    }

    /// confidence 越界夹紧、snake_case 字段可读。
    #[test]
    fn normalize_world_lore_clamps_confidence() {
        let v = json!({
            "title": "青云宗",
            "text": "以剑修为主的宗门。",
            "permanent": false,
            "tags": ["宗门"],
            "importance": "high",
            "type": "faction",
            "confidence": 1.7,
            "evidence_refs": ["ch01:block2"],
        });
        let e = normalize_world_lore_entry(&v).unwrap();
        assert_eq!(e["permanent"], false);
        assert_eq!(e["importance"], "high");
        assert_eq!(e["type"], "faction");
        assert_eq!(e["confidence"].as_f64().unwrap(), 1.0);
        assert_eq!(e["evidence_refs"][0], "ch01:block2");
    }

    /// 节拍数组解析：截断长度与条数、过滤空串。
    #[test]
    fn beats_parse_truncates() {
        let v = json!(["节拍一", "节拍二", "", "长".repeat(60), "节拍三", "节拍四"]);
        let beats = parse_beats(&v, 3, 40);
        assert_eq!(beats.len(), 3);
        for b in &beats {
            assert!(b.chars().count() <= 40);
        }
        // 对象包装 / 字符串包装容错
        let empty: Vec<String> = vec![];
        assert_eq!(parse_beats(&json!({"k": "v"}), 3, 40), empty);
        assert_eq!(parse_beats(&json!("单条"), 3, 40), vec!["单条".to_string()]);
    }

    /// 出口候选解析：when 非空、hint 可空、上限 3。
    #[test]
    fn exit_candidates_parse() {
        let v = json!([
            {"when": "决定追上去", "hint": "看看那人去了哪"},
            {"when": "", "hint": "空 when 应被过滤"},
            {"when": "留在原地", "hint": ""},
            {"when": "第四项"},
            {"when": "第五项"},
        ]);
        let c = parse_exit_candidates(&v);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0], ("决定追上去".to_string(), "看看那人去了哪".to_string()));
        assert_eq!(c[1], ("留在原地".to_string(), String::new()));
    }

    /// 世界线 JSON 解析容错：event 非空保留、其余丢弃，非数组包装后过滤。
    #[test]
    fn worldline_parse_tolerant() {
        let v = json!([
            {"event": "宗门大战", "time_point": "卷一", "characters": ["叶辰"], "importance": "high", "chapter": "ch01"},
            {"time_point": "无事件被丢弃", "characters": []},
            "字符串项被丢弃",
        ]);
        let list = parse_worldline(&v);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["event"], "宗门大战");
        // 对象（非数组）包装后过滤
        let obj = parse_worldline(&json!({"event": "单事件"}));
        assert_eq!(obj.len(), 1);
        assert_eq!(obj[0]["event"], "单事件");
        let empty = parse_worldline(&json!([]));
        assert_eq!(empty.len(), 0);
    }

    // ─── 素材库蒸馏解析测试（纯函数，不依赖 LLM）───────────────────────────────

    /// 事件包解析：合法样例产出结构化包与卡，缺字段兜底生效。
    #[test]
    fn event_packages_parse_valid() {
        let v = json!([
            {
                "id": "pkg-adventure",
                "name": "奇遇包",
                "description": "主角在山野间的奇遇",
                "enabled": true,
                "cards": [
                    {"id": "card-1", "title": "悬崖奇遇", "prompt": "江景离在青云宗后山悬崖发现遗迹", "weight": 3, "enabled": true, "oncePerSession": true},
                    {"id": "card-2", "title": "集市偶遇", "prompt": "主角在临安城集市遇到神秘摊主"},
                    {"id": "card-3", "title": "灵兽拦路", "prompt": "一只灵兽拦住去路", "weight": 1, "enabled": true, "once_per_session": false}
                ]
            }
        ]);
        let pkgs = parse_event_packages(&v);
        assert_eq!(pkgs.len(), 1);
        let p = &pkgs[0];
        assert_eq!(p.id, "pkg-adventure");
        assert_eq!(p.name, "奇遇包");
        assert!(p.enabled);
        assert_eq!(p.cards.len(), 3);
        // weight 缺省兜底 1；oncePerSession 缺省 false
        assert_eq!(p.cards[1].weight, 1);
        assert!(!p.cards[1].once_per_session);
        assert!(!p.cards[1].used_in_session);
        // 兼容 snake_case once_per_session
        assert!(!p.cards[2].once_per_session);
        // 事件包能被 pick_event_card 抽出（模块语义：启用包 + enabled 卡）
        let pkg_value = serde_json::to_value(&pkgs).unwrap();
        assert!(pkg_value.as_array().is_some());
    }

    /// 事件包解析：非法卡（缺 id/title/prompt）跳过；整包卡全非法则跳过该包。
    #[test]
    fn event_packages_parse_skips_invalid() {
        let v = json!([
            {
                "id": "pkg-bad",
                "name": "坏包",
                "cards": [
                    {"id": "", "title": "无 id", "prompt": "x"},
                    {"id": "c", "title": "", "prompt": "x"},
                    {"id": "c", "title": "无 prompt"}
                ]
            },
            {
                "id": "pkg-ok",
                "name": "好包",
                "cards": [
                    {"id": "c1", "title": "正常卡", "prompt": "有效提示", "weight": 5}
                ]
            }
        ]);
        let pkgs = parse_event_packages(&v);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].id, "pkg-ok");
        assert_eq!(pkgs[0].cards.len(), 1);
        // 缺 id/name 的包被跳过
        let bad_pkg = json!([{"name": "无 id 包", "cards": [{"id": "c", "title": "t", "prompt": "p"}]}]);
        assert!(parse_event_packages(&bad_pkg).is_empty());
        // 非数组包装容错
        let wrapped = json!({"id": "pkg-w", "name": "w", "cards": [{"id": "c", "title": "t", "prompt": "p"}]});
        let pkgs2 = parse_event_packages(&wrapped);
        assert_eq!(pkgs2.len(), 1);
    }

    /// 事件包解析：包数与卡数有上限。
    #[test]
    fn event_packages_parse_caps() {
        let pkgs: Vec<Value> = (0..8)
            .map(|i| {
                let cards: Vec<Value> = (0..8)
                    .map(|j| json!({"id": format!("c{i}-{j}"), "title": "t", "prompt": "p", "weight": 1}))
                    .collect();
                json!({"id": format!("pkg-{i}"), "name": format!("包{i}"), "cards": cards})
            })
            .collect();
        let out = parse_event_packages(&json!(pkgs));
        assert_eq!(out.len(), MAX_EVENT_PACKAGES);
        for p in &out {
            assert!(p.cards.len() <= MAX_EVENT_CARDS);
        }
        assert_eq!(out[0].cards.len(), MAX_EVENT_CARDS);
    }

    /// 事件包解析 G7：camelCase 新字段（typeName/category/tags/intensity/cooldownTurns）落盘。
    #[test]
    fn event_packages_parse_new_fields_camel() {
        let v = json!([
            {
                "id": "pkg-adventure",
                "name": "奇遇包",
                "cards": [
                    {
                        "id": "card-1",
                        "title": "牌坊考核",
                        "prompt": "外门考核开始",
                        "typeName": "外门考核打脸",
                        "category": "打脸",
                        "tags": ["门派", "考核", "打脸"],
                        "intensity": "medium",
                        "cooldownTurns": 2
                    }
                ]
            }
        ]);
        let pkgs = parse_event_packages(&v);
        assert_eq!(pkgs.len(), 1);
        let c = &pkgs[0].cards[0];
        assert_eq!(c.type_name, "外门考核打脸");
        assert_eq!(c.category, "打脸");
        assert_eq!(c.tags, vec!["门派".to_string(), "考核".to_string(), "打脸".to_string()]);
        assert_eq!(c.intensity, "medium");
        assert_eq!(c.cooldown_turns, 2);
    }

    /// 事件包解析 G7：snake_case 新字段（type_name/category/tags/intensity/cooldown_turns）兼容落盘。
    #[test]
    fn event_packages_parse_new_fields_snake() {
        let v = json!([
            {
                "id": "pkg-romance",
                "name": "恋爱包",
                "cards": [
                    {
                        "id": "card-2",
                        "title": "误入香闺",
                        "prompt": "雨夜误入沈棠闺房",
                        "type_name": "误入香闺",
                        "category": "恋爱",
                        "tags": ["闺房", "幽会"],
                        "intensity": "low",
                        "cooldown_turns": 3
                    }
                ]
            }
        ]);
        let pkgs = parse_event_packages(&v);
        assert_eq!(pkgs.len(), 1);
        let c = &pkgs[0].cards[0];
        assert_eq!(c.type_name, "误入香闺");
        assert_eq!(c.category, "恋爱");
        assert_eq!(c.tags, vec!["闺房".to_string(), "幽会".to_string()]);
        assert_eq!(c.intensity, "low");
        assert_eq!(c.cooldown_turns, 3);
    }

    /// 事件包解析 G7：缺新字段不跳卡（默认 type_name/category/intensity 空、tags 空、cooldown_turns 0）。
    #[test]
    fn event_packages_parse_missing_new_fields_no_skip() {
        let v = json!([
            {
                "id": "pkg-legacy",
                "name": "旧包",
                "cards": [
                    {"id": "card-9", "title": "旧卡", "prompt": "旧数据无新字段"}
                ]
            }
        ]);
        let pkgs = parse_event_packages(&v);
        assert_eq!(pkgs.len(), 1, "缺新字段不得跳卡");
        let c = &pkgs[0].cards[0];
        assert_eq!(c.type_name, "");
        assert_eq!(c.category, "");
        assert!(c.tags.is_empty());
        assert_eq!(c.intensity, "");
        assert_eq!(c.cooldown_turns, 0);
        // 旧字段照常
        assert_eq!(c.id, "card-9");
        assert_eq!(c.title, "旧卡");
        assert!(c.enabled);
    }

    /// 演员状态解析：合法样例 → templates + initial_actors 结构正确，schema_version 归 1。
    #[test]
    fn actor_state_parse_valid() {
        let v = json!({
            "schemaVersion": 1,
            "initialActors": [
                {"characterId": "江景离", "templateId": "tpl-protagonist"},
                {"characterId": "沈棠", "templateId": "tpl-supporting"}
            ],
            "templates": {
                "tpl-protagonist": {
                    "fields": {
                        "修为": {"valueType": "number", "value": 10, "min": 0, "max": 100, "display": "修为", "updateInstruction": "修炼/战斗后变化"},
                        "好感": {"valueType": "number", "value": 50, "min": 0, "max": 100, "display": "好感"}
                    },
                    "traitPools": [{"id": "pool-1", "name": "灵根", "traits": [{"id": "t1", "name": "单灵根", "weight": 1}]}]
                },
                "tpl-supporting": {
                    "fields": {"伤势": {"valueType": "number", "value": 0, "min": 0, "max": 10, "display": "伤势"}},
                    "traitPools": []
                }
            }
        });
        let cfg = parse_actor_state_config(&v, None);
        assert_eq!(cfg.schema_version, 1);
        assert_eq!(cfg.templates.len(), 2);
        assert_eq!(cfg.initial_actors.len(), 2);
        let tpl = &cfg.templates["tpl-protagonist"];
        assert_eq!(tpl.fields.len(), 2);
        assert_eq!(tpl.fields["修为"].value_type, "number");
        assert_eq!(tpl.fields["修为"].min, Some(0.0));
        assert_eq!(tpl.fields["修为"].max, Some(100.0));
        assert_eq!(tpl.trait_pools.len(), 1);
        assert_eq!(cfg.initial_actors[0].character_id, "江景离");
        assert_eq!(cfg.initial_actors[0].template_id, "tpl-protagonist");
        // to_system 能把配置物化为 ActorStateSystem
        let sys = cfg.to_system();
        assert!(sys.actors.contains_key("江景离"));
        assert!(sys.actors.contains_key("沈棠"));
    }

    /// 演员状态解析：非法模板/悬空引用被裁剪，解析失败返回默认空配置。
    #[test]
    fn actor_state_parse_prunes_invalid() {
        let v = json!({
            "initialActors": [
                {"characterId": "悬空", "templateId": "tpl-missing"},
                {"characterId": "好角色", "templateId": "tpl-ok"}
            ],
            "templates": {
                "tpl-missing": {"fields": {}, "traitPools": []},
                "tpl-ok": {"fields": {"声望": {"valueType": "number", "value": 5, "min": 0, "max": 10}}},
                "": {"fields": {"x": {"valueType": "number"}}}
            }
        });
        let cfg = parse_actor_state_config(&v, None);
        assert_eq!(cfg.templates.len(), 1);
        assert!(cfg.templates.contains_key("tpl-ok"));
        assert_eq!(cfg.initial_actors.len(), 1);
        assert_eq!(cfg.initial_actors[0].character_id, "好角色");
        // 白名单过滤：LLM 自创的「舍友」不在角色卡名单 → 丢弃该 actor + 孤立模板
        let whitelist: std::collections::HashSet<String> =
            ["江景离", "莫旺财"].iter().map(|s| s.to_string()).collect();
        let v2 = json!({
            "initialActors": [
                {"characterId": "莫旺财", "templateId": "tpl-mwc"},
                {"characterId": "舍友", "templateId": "tpl-sheyou"}
            ],
            "templates": {
                "tpl-mwc": {"fields": {"声望": {"valueType": "number", "value": 5, "min": 0, "max": 10}}},
                "tpl-sheyou": {"fields": {"地位": {"valueType": "string", "value": "舍友"}}}
            }
        });
        let cfg2 = parse_actor_state_config(&v2, Some(&whitelist));
        assert_eq!(cfg2.initial_actors.len(), 1);
        assert_eq!(cfg2.initial_actors[0].character_id, "莫旺财");
        assert!(cfg2.templates.contains_key("tpl-mwc"));
        assert!(!cfg2.templates.contains_key("tpl-sheyou"), "孤立模板应被清理");
        // 非对象 → 默认空配置
        let empty = parse_actor_state_config(&json!([1, 2, 3]), None);
        assert!(empty.templates.is_empty());
        assert!(empty.initial_actors.is_empty());
        // 完全非 JSON 结构 → 默认空配置
        let bad = parse_actor_state_config(&json!("not an object"), None);
        assert!(bad.templates.is_empty());
    }

    /// 文风归一化：字段齐整、缺省补空、非对象返回空对象。
    #[test]
    fn narrative_style_normalize() {
        let v = json!({
            "style": "第三人称全知",
            "tone": "热血",
            "pacing": "快节奏爽文",
            "scene_focus": "战斗场面",
            "prose_guidance": "多用短句，战斗段落镜头感强。"
        });
        let s = normalize_narrative_style(&v);
        assert_eq!(s["style"], "第三人称全知");
        assert_eq!(s["tone"], "热血");
        assert_eq!(s["prose_guidance"], "多用短句，战斗段落镜头感强。");
        // 缺省字段补空串
        let s2 = normalize_narrative_style(&json!({"style": "第一人称"}));
        assert_eq!(s2["tone"], "");
        // 非对象 → 空对象
        let s3 = normalize_narrative_style(&json!([1]));
        assert_eq!(s3.as_object().map(|o| o.len()), Some(0));
    }

    /// 规则检定解析：合法样例全部保留，能反序列化为 RuleSystem。
    #[test]
    fn rule_system_parse_valid() {
        let v = json!({
            "checks": [
                {"id": "breakthrough", "label": "修炼突破", "dice": "d100", "modifier": 0, "trigger": "尝试突破境界", "mustCheckExamples": ["冲击筑基"], "skipCheckExamples": ["日常吐纳"], "successHint": "突破成功", "failureHint": "突破失败，走火入魔"},
                {"id": "combat", "label": "战斗", "dice": "d20", "modifier": 2, "trigger": "与敌交手", "mustCheckExamples": ["与妖兽搏斗"], "failurePolicy": "block"},
                {"id": "persuade", "label": "交涉", "dice": "d20", "modifier": -1, "trigger": "游说他人", "mustCheckExamples": ["说服掌门"]},
                {"id": "stealth", "label": "潜行", "dice": "d20", "modifier": 0, "trigger": "潜入禁地", "mustCheckExamples": ["夜探藏经阁"]},
                {"id": "perception", "label": "察觉", "dice": "d20", "modifier": 1, "trigger": "观察四周", "mustCheckExamples": ["发现埋伏"]}
            ]
        });
        let r = normalize_rule_system(&v);
        let checks = r["checks"].as_array().unwrap();
        assert_eq!(checks.len(), 5);
        let sys = RuleSystem::from_value(&r).expect("应能反序列化为 RuleSystem");
        assert_eq!(sys.checks.len(), 5);
        assert_eq!(sys.checks[0].id, "breakthrough");
        assert_eq!(sys.checks[0].dice, "d100");
        assert_eq!(sys.checks[1].modifier, 2.0);
    }

    /// 规则检定解析：id/label/dice 空的条被过滤；非 checks 结构返回空。
    #[test]
    fn rule_system_parse_skips_invalid() {
        let v = json!({
            "checks": [
                {"id": "", "label": "无 id", "dice": "d20"},
                {"id": "a", "label": "", "dice": "d20"},
                {"id": "b", "label": "无骰", "dice": ""},
                {"id": "ok", "label": "合格", "dice": "d20", "trigger": "触发"}
            ]
        });
        let r = normalize_rule_system(&v);
        let checks = r["checks"].as_array().unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0]["id"], "ok");
        // 无 checks 键 → 空
        let r2 = normalize_rule_system(&json!({"foo": "bar"}));
        assert_eq!(r2["checks"].as_array().map(|a| a.len()), Some(0));
        // checks 为字符串 → 解析失败返回空
        let r3 = normalize_rule_system(&json!({"checks": "oops"}));
        assert_eq!(r3["checks"].as_array().map(|a| a.len()), Some(0));
    }

    // ─── B4: 网络错误不消耗 fffd 配额 ─────────────────────────────────────────

    /// 模拟桩：4 次 fffd + 2 次网络错误，应能救回（网络错误重试成功→出卡），而不是全耗完跳过。
    #[tokio::test]
    async fn retry_character_card_network_errors_separate_from_fffd() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let card_v = distill_one_character_card("庄眉", "high", {
            let calls = std::rc::Rc::clone(&calls);
            move || {
                let calls = std::rc::Rc::clone(&calls);
                async move {
                    let n = calls.get();
                    calls.set(n + 1);
                    match n {
                        0 => Err("error sending request for url ...9527".into()),
                        1 => Ok("{\"name\":\"庄眉\",\"personality\":\"对弗\u{FFFD}\u{FFFD}德的信任\"}".into()),
                        2 => Err("error sending request for url ...9527".into()),
                        3 => Ok("{\"name\":\"庄眉\",\"personality\":\"对弗\u{FFFD}德的信任\"}".into()),
                        4 => Ok("{\"name\":\"庄眉\",\"personality\":\"对弗雷德的信任\"}".into()),
                        _ => Ok("{\"name\":\"庄眉\",\"personality\":\"对弗雷德的信任\"}".into()),
                    }
                }
            }
        })
        .await;
        let card_v = card_v.expect("4次 fffd + 2次网络错误后应产出合法 JSON 卡");
        assert_eq!(card_v["name"], "庄眉");
        assert_eq!(card_v["personality"], "对弗雷德的信任");
        // 检查调用次数：2次网络错误 + 2次 fffd + 1次成功 = 5 次
        assert_eq!(calls.get(), 5, "应恰好在第 5 次调用时成功（2次网络+2次fffd+1次成功）");
    }

    // ─── B5: high 角色保底兜底 ─────────────────────────────────────────────────

    /// 模拟桩：high 角色 fffd 耗尽后降级保底应产出最小卡。
    #[tokio::test]
    async fn retry_character_card_high_fallback_yields_minimal_card() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let card_v = distill_one_character_card("主角", "high", {
            let calls = std::rc::Rc::clone(&calls);
            move || {
                let calls = std::rc::Rc::clone(&calls);
                async move {
                    let n = calls.get();
                    calls.set(n + 1);
                    // 前 6 轮全 fffd → fffd 配额耗尽
                    if n < 6 {
                        Ok("{\"name\":\"主角\",\"personality\":\"对弗\u{FFFD}的信任\"}".into())
                    } else if n == 6 {
                        // 降级保底第 1 轮
                        Ok("{\"name\":\"主角\",\"personality\":\"勇敢坚毅\"}".into())
                    } else {
                        Ok("{\"name\":\"主角\",\"personality\":\"勇敢坚毅\"}".into())
                    }
                }
            }
        })
        .await;
        let card_v = card_v.expect("high 角色降级保底应产出最小卡");
        assert_eq!(card_v["name"], "主角");
        // 至少调用了 7 次（6次 fffd + 1次保底）
        assert!(calls.get() >= 7, "应至少调用 7 次（6次 fffd + 1次保底）");
    }

    /// 模拟桩：非 high 角色 fffd 耗尽后不应触发保底。
    #[tokio::test]
    async fn retry_character_card_non_high_no_fallback() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let card_v = distill_one_character_card("路人", "low", {
            let calls = std::rc::Rc::clone(&calls);
            move || {
                let calls = std::rc::Rc::clone(&calls);
                async move {
                    calls.set(calls.get() + 1);
                    Ok("{\"name\":\"路人\",\"personality\":\"\u{FFFD}信任\"}".into())
                }
            }
        })
        .await;
        assert!(card_v.is_none(), "非 high 角色 fffd 耗尽后不应触发保底");
        assert_eq!(calls.get(), 4, "low 角色 fffd 配额 4 次，应恰好在 4 次后放弃");
    }

    // ─── B4: 网络错误独立配额测试 ─────────────────────────────────────────────

    /// 模拟桩：低角色连续网络错误 → conn_err 上限 2 快速放弃（83746baf 优化），
    /// fffd 配额(3)未动——验证网络错误独立于坏字配额。
    #[tokio::test]
    async fn retry_character_card_low_conn_errors_separate() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let card_v = distill_one_character_card("路人", "low", {
            let calls = std::rc::Rc::clone(&calls);
            move || {
                let calls = std::rc::Rc::clone(&calls);
                async move {
                    let n = calls.get();
                    calls.set(n + 1);
                    Err("网络超时".into())
                }
            }
        })
        .await;
        assert!(card_v.is_none(), "low 角色网络错误快速放弃，不产出卡");
        assert_eq!(calls.get(), 2, "low 角色 2 次网络错误后放弃（conn_err 上限 2）");
    }

    // ─── D1: high 角色证据空时词频兜底 ──────────────────────────────────────
    #[test]
    fn fallback_evidence_scan_recalls_high_frequency_character() {
        let chapters = vec![
            ("ch01".to_string(), "庄眉走进派出所带走向明初，作为母亲她异常冷静。".to_string()),
            ("ch02".to_string(), "向明初在宿舍发呆，完全没有庄眉的消息。".to_string()),
            ("ch03".to_string(), "路边卖糖水的老王吆喝着生意。".to_string()),
        ];
        // 庄眉：别名含"眉姐"
        let ev = fallback_evidence_scan(&chapters, "庄眉", &["眉姐".to_string()], 8);
        assert!(!ev.is_empty(), "高频角色应能按名字召回证据");
        assert!(ev.contains("庄眉"), "兜底证据应包含角色名原文");
        assert!(ev.contains("ch01"), "应标注来源章节");
        assert!(ev.contains("ch02"), "应跨章召回(另一方无助时仍提及庄眉)");
    }

    #[test]
    fn fallback_evidence_scan_skips_absent_character() {
        let chapters = vec![
            ("ch01".to_string(), "老王在街角卖糖水，招呼着过路的学子。".to_string()),
            ("ch02".to_string(), "宿舍阿姨锁好门,叮嘱大家早点休息。".to_string()),
        ];
        // 全本不出现的角色 → 应返回空(继续走跳过分支)
        let ev = fallback_evidence_scan(&chapters, "庄眉", &[], 8);
        assert!(ev.is_empty(), "原文未出现的角色兜底也应为空");
    }

    #[test]
    fn st26_late_hotel_detected() {
        // 度蜜月实测违规样本：蜜月套房 → 触发
        assert!(opening_scene_violates_st26(
            "清晨，蜜月套房内，木质地板微凉，海浪声从推拉门方向渗入。"
        ));
    }

    #[test]
    fn st26_spoiler_suffix_detected() {
        // 剧透后缀「另：」 → 触发
        assert!(opening_scene_violates_st26(
            "海边度假房间，海风灌入，纱帘鼓动。另：深夜接沈雨棠电话时"
        ));
    }

    #[test]
    fn st26_daily_scene_allowed() {
        // 学校/家中日常场景 → 放行
        assert!(!opening_scene_violates_st26(
            "学校走廊，午后阳光穿过樟树枝叶，学生涌出教室。林婉清背着包往外走。"
        ));
    }

    #[test]
    fn st26_empty_allowed() {
        assert!(!opening_scene_violates_st26("证据不足，无实际出场场景"));
    }
}