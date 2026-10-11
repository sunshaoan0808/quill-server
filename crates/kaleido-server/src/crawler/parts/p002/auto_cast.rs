//! crawler/parts/p002/auto_cast —— 自动抽取角色谱后台任务（spawn_auto_cast_extraction）。（R6-3① 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn spawn_auto_cast_extraction(state: AppState, pack_id: String) {
    tokio::spawn(async move {
        tracing::info!(pack_id, "auto cast extraction started");
        let pack = match state.packs.get(&pack_id) {
            Ok(p) => p,
            Err(_) => return,
        };
        let real: Vec<String> = pack
            .characters
            .iter()
            .filter(|c| c.role != "narrator" && c.role != "player")
            .map(|c| c.name.clone())
            .collect();
        // [fix 2026-08-15 阈值相对判断] 原来 `>=3 就跳过` 导致长篇小说（331 章智取美母等）
        // 启发式抽到 3-4 个后 LLM 抽取永不触发，叙述中出现的大量角色全漏。
        // 改为：已有角色数少于「章节数/20 且至少 4 个」才补抽；existing.contains 去重保证重复补抽安全。
        let chapter_count = pack.chapters.len().max(1);
        let min_expected = (chapter_count / 20).max(4);
        if real.len() >= min_expected {
            return;
        }
        // [fix 2026-08-15 分层抽样] 原来只读前 5 章 → 长篇中后段角色永远抽不到（温床/宿醉等
        // 短篇 LLM 每轮抽到相同前部角色 added=0 静默）。改为按章节数均匀取 5 个采样点：
        // 开头/1/4/1/2/3/4/结尾，覆盖全书角色分布。
        let mut sample = String::new();
        let mut chapters_read = 0usize;
        let mut sampled: Vec<usize> = Vec::new();
        if chapter_count <= 5 {
            sampled = (0..chapter_count).collect();
        } else {
            for frac in [0usize, 1, 2, 3, 4] {
                sampled.push((chapter_count - 1) * frac / 4);
            }
            sampled.dedup();
        }
        for idx in sampled {
            let ch = &pack.chapters[idx];
            let body = if !ch.body_path.trim().is_empty() {
                state
                    .packs
                    .read_chapter_body(&pack_id, &ch.body_path)
                    .ok()
            } else {
                None
            };
            let body = body.or_else(|| {
                // [fix 2026-08-15] 珞白 pack：chapters 登记了 body_path=None 但正文在 chapters/{id}.md
                state
                    .packs
                    .read_chapter_body(&pack_id, &format!("chapters/{}.md", ch.id))
                    .ok()
            });
            if let Some(f) = body {
                // [fix 2026-08-15] 目录页污染样本：安全屋 pack 的 ch01-03.md 是「- 第N章：标题」目录列表
                // 不是正文（导入时抓目录当章节存了）。目录页特征 = 短文件 + 大量「- 第N章」行。
                // 跳过目录页，避免 LLM 从章节标题里瞎抽"角色"。
                if is_chapter_toc(&f) {
                    tracing::warn!(pack_id, title = %ch.title, "auto cast: 跳过章节目录页");
                    continue;
                }
                sample.push_str(&format!("=== {} ===\n", ch.title));
                sample.push_str(&f.chars().take(2200).collect::<String>());
                sample.push('\n');
                chapters_read += 1;
            }
        }
        // 章节索引全断链时回退扫 chapters/ 目录实际文件（前 5 个按文件名排序）
        if chapters_read == 0 {
            if let Ok(dir) = state.packs.pack_dir(&pack_id) {
                let chdir = dir.join("chapters");
                if let Ok(entries) = std::fs::read_dir(&chdir) {
                    let mut files: Vec<_> = entries.filter_map(|e| e.ok()).collect();
                    files.sort_by_key(|e| e.file_name());
                    for ent in files.into_iter().take(5) {
                        if let Ok(f) = std::fs::read_to_string(ent.path()) {
                            if is_chapter_toc(&f) {
                                continue;
                            }
                            sample.push_str(&format!(
                                "=== {} ===\n",
                                ent.file_name().to_string_lossy()
                            ));
                            sample.push_str(&f.chars().take(2200).collect::<String>());
                            sample.push('\n');
                            chapters_read += 1;
                        }
                    }
                }
            }
        }
        // [fix 2026-08-15] 正文文件全缺时从 nodes[].summary 拼样本（智取美母实证：
        // chapters/ 目录空、bodyPath 全断链，但 331 个 node 的 summary 各含 400 字蒸馏正文，
        // 足够 LLM 抽取角色）。summary 都是正文浓缩，不会误触目录页/会话另存检测。
        if chapters_read == 0 {
            for n in pack.nodes.iter().take(10) {
                if !n.summary.trim().is_empty() {
                    sample.push_str(&format!("=== {} ===\n", n.title));
                    sample.push_str(&n.summary.chars().take(2000).collect::<String>());
                    sample.push('\n');
                }
            }
        }
        if sample.trim().is_empty() {
            return;
        }
        // [fix 2026-08-15] 会话另存 pack 判别：章节内容是「故事馆 · turn 0 · tavern-session-xxx」开场白，
        // 不是小说正文（孕船-88828 只有 665 字节开场白），抽取无意义，跳过并标记。
        if sample.contains("来源：故事馆") || sample.contains("tavern-session-") {
            tracing::warn!(pack_id, "auto cast extraction skipped: 会话另存 pack 无小说正文");
            return;
        }
        let llm = state
            .app_state
            .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
        let prov_kind = crate::llm_stream::runtime_provider_kind(&llm, &state.provider_kind);
        if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
            tracing::warn!(pack_id, "auto cast extraction skipped: no llm runtime");
            return;
        }
        let system = "你是小说角色抽取助手。从给定的小说章节文本中识别有跨章节意义的角色（人名，排除旁白/读者/玩家），输出 JSON 数组：[{\"name\":\"角色名\",\"role\":\"主角|配角\",\"personality\":\"一句话性格\",\"speechStyle\":\"说话风格\",\"identity\":\"身份\",\"aliases\":[\"该角色的其他称呼/别名/外号，不含亲缘称谓如母亲/哥哥，不含泛称如那怪/众人\"],\"exampleDialogs\":[\"一句代表性台词\"]}]。name 必须是明确的人物专名（姓名/称呼/外号），严禁输出句子片段、短语、普通名词或口语碎片（如\"那就\"\"冲进下水\"\"另一个\"这类都不是角色名）；拿不准的名字不要输出。只输出 JSON，不要其他文字。";
        match crate::llm_stream::stream_chat_completions_dispatch(
            &llm.base_url,
            &llm.api_key,
            &llm.model,
            &prov_kind,
            system,
            &sample,
            0.1,
            4096,
            150,
            |_| true,
        )
        .await
        {
            Ok(raw) => {
                // [fix 2026-08-15] rfind(']') 会截到错误位置：LLM 输出常在 JSON 数组后
                // 追加说明文字（「以上是角色…」「```」）或中途出现嵌套 ]，导致 trailing characters。
                // 改为括号匹配：从第一个 [ 起按深度配对取完整数组。
                let parsed: Vec<Value> = match extract_json_array(&raw) {
                    Some(v) => v,
                    None => {
                        tracing::warn!(pack_id, "auto cast extraction: no valid JSON array in llm output");
                        return;
                    }
                };
                let mut pack = match state.packs.get(&pack_id) {
                    Ok(p) => p,
                    Err(_) => return,
                };
                let mut existing: std::collections::HashSet<String> = pack
                    .characters
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                let mut added = 0usize;
                // [fix 2026-08-15 alias_merge 接线] 收集本轮 LLM 抽取的 (name, aliases)，
                // 若 aliases 命中已有角色（如新抽「美猴王」aliases=[孙悟空] 而已有孙悟空），
                // 归并到已有角色而非重复添加；未命中的别名也全部归一化到组内 canonical。
                let mut alias_pairs: Vec<(String, String)> = Vec::new();
                for v in &parsed {
                    let name = v
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if name.is_empty() {
                        continue;
                    }
                    if let Some(arr) = v.get("aliases").and_then(|a| a.as_array()) {
                        for a in arr {
                            if let Some(alias) = a.as_str() {
                                let alias = alias.trim();
                                if !alias.is_empty() && alias != name {
                                    alias_pairs.push((alias.to_string(), name.clone()));
                                }
                            }
                        }
                    }
                }
                let alias_map = kaleido_core::alias_merge::build_alias_map(&alias_pairs);
                // [P7] merged_aliases 计数器从未被读取（仅日志可观测），移除变量保留日志语义。
                for (i, v) in parsed.into_iter().enumerate() {
                    let name = v
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if name.is_empty() || existing.contains(&name) {
                        continue;
                    }
                    // [fix 2026-08-15 alias_merge 接线] 别名命中已有角色 → 归并跳过
                    // （「美猴王」aliases=[孙悟空] 而已有孙悟空 → 美猴王不再新增占位卡）
                    let resolved = kaleido_core::alias_merge::resolve_alias(&alias_map, &name);
                    if resolved != name && existing.contains(&resolved) {
                        tracing::info!(pack_id, alias = %name, canon = %resolved, "auto cast: 别名归并到已有角色");
                        continue;
                    }
                    // [fix 2026-08-13 乱码名专项] 角色名合法性过滤 + 正文存在性兜底：
                    // LLM 常把句子片段当人名（「冲进下水」「过电车轨」「那就」），
                    // 直接 push 会产生空壳占位卡污染 pack。名字必须在抽取样本中出现
                    // （乱码名如「湮风通」「人里克斯」在正文中从不以该形式出现）。
                    if !crate::convert::is_plausible_character_name(&name) {
                        tracing::warn!(pack_id, name = %name, "auto cast: 过滤疑似乱码角色名");
                        continue;
                    }
                    if !sample.contains(&name) {
                        tracing::warn!(pack_id, name = %name, "auto cast: 角色名未在正文样本出现，跳过");
                        continue;
                    }
                    let role = if v.get("role").and_then(|r| r.as_str()) == Some("主角") {
                        "protagonist"
                    } else {
                        "supporting"
                    };
                    pack.characters.push(kaleido_core::PackCharacterRef {
                        id: format!("c-cast-{i}"),
                        name: name.clone(),
                        role: role.into(),
                        importance: if i == 0 { "high".into() } else { "medium".into() },
                        gender: "未知".into(),
                        appearance: "未知".into(),
                        opening_scene: "未知".into(),
                        opening_lines: "".into(),
                        nsfw_profile: String::new(),
                        content_tier: None,
                        example_dialogs: v
                            .get("exampleDialogs")
                            .and_then(|d| serde_json::from_value(d.clone()).ok())
                            .unwrap_or_default(),
                        boundaries: vec![],
                        personality: v
                            .get("personality")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        speech_style: v
                            .get("speechStyle")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        voice_profile: v
                            .get("voiceProfile")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        motivation: String::new(),
                        relationships: vec![],
                        evidence_refs: vec![],
                        mental_models: vec![],
                        decision_heuristics: vec![],
                        beliefs: vec![],
                        expressions: Default::default(),
            voice: None,
            archive: None,
                        avatar: None,
                        starting_wardrobe: Default::default(),
                    });
                    existing.insert(name);
                    added += 1;
                }
                if added > 0 {
                    let _ = state.packs.save(pack);
                    tracing::info!(pack_id, added, "auto cast extraction: +{added} chars");
                } else {
                    // 收敛态：LLM 返回的角色全部已存在（多轮补抽后常见），
                    // 明确打日志避免"零日志"被误判为卡死/未触发。
                    tracing::info!(pack_id, "auto cast extraction: 收敛（LLM 返回角色均已存在，无新增）");
                }
            }
            Err(e) => {
                tracing::warn!(pack_id, "auto cast extraction llm error: {e}");
            }
        }
    });
}
