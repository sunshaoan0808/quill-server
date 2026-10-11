//! convert/evidence —— 向量证据检索基建（EvidenceBlock/余弦 top-k/召回/回退扫描）
#![allow(unused_imports)]

use super::*;

/// 单块目标字数 ~800、相邻块重叠 ~100 字（避免证据被切断裂在关键句处）。
#[derive(Debug, Clone)]
pub struct EvidenceBlock {
    /// 章节标识（如 "ch01"），用于 evidence_refs 与检索展示前缀。
    pub chapter: String,
    /// 原文片段。
    pub text: String,
}

pub(crate) fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na2 = 0.0f64;
    let mut nb2 = 0.0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += (x as f64) * (y as f64);
        na2 += (x as f64) * (x as f64);
        nb2 += (y as f64) * (y as f64);
    }
    let denom = na2.sqrt() * nb2.sqrt();
    if denom == 0.0 {
        return 0.0;
    }
    (dot / denom) as f32
}

/// 分析文本生成确定性固定维度签名向量（字符出现计数 → L2 归一化）。
/// 仅用于让 [`cosine_top_k`] 在无 embed 模型时也能独立完成可复现的匹配与测试。
#[allow(dead_code)]

pub(crate) fn block_signature(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; SIG_DIM];
    for ch in text.chars().flat_map(|c| c.to_lowercase()) {
        let code = ch as u32;
        let i = (code % SIG_DIM as u32) as usize;
        v[i] += 1.0;
        let j = (code.wrapping_add(31) % SIG_DIM as u32) as usize;
        v[j] += 0.5;
    }
    let norm = v.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x = (*x as f64 / norm) as f32;
        }
    }
    v
}

/// 手写余弦相似度 top-k 检索。
///
/// 对每个 `EvidenceBlock.text` 计算与 `query` 向量的余弦相似度，返回相似度最高的
/// `k` 个 `(index, score)`，按 score 降序；不足 `k` 个时返回全部。
///
/// 说明：为保持纯函数、可脱离 embed 模型测试，这里把块正文经 [`block_signature`]
/// 映射为定维向量后与 `query` 比较；真实蒸馏流程里的向量检索见 [`retrieve_character_evidence`]。
#[allow(dead_code)]

pub fn cosine_top_k(query: &[f32], blocks: &[EvidenceBlock], k: usize) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (i, cosine_sim(query, &block_signature(&b.text))))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

/// 在已向量化的嵌入列表里做余弦 top-k（供真实检索路径用，向量维度一致）。
#[allow(dead_code)] // [P7] 向量检索辅助预留

/// 把一段正文字符串按 `block_size` 字、重叠 `overlap` 字切成若干块（按 char 边界）。
/// 返回 trim 后非空的块；空输入返回空数组。
pub(crate) fn top_k_vec(query: &[f32], embs: &[Vec<f32>], k: usize) -> Vec<(usize, f32)> {
    let mut scored: Vec<(usize, f32)> = embs
        .iter()
        .enumerate()
        .map(|(i, e)| (i, cosine_sim(query, e)))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

/// 角色认知卡系统提示词（仓颉 7 级证据法 + 女娲认知卡结构 + TavernWeave 模块契约，输出严格 JSON）。
///
/// 模块契约设计(吸收 TavernWeave cot-design 方法论):
/// 每个字段=一个模块,有明确"要回答的问题/输入/产出/跳过条件/降级";
/// 所有结论必须 evidence_refs 可溯源(我们的差异优势,保留);
/// 证据不足的字段显式降级为"未知",绝不脑补。
pub(crate) fn split_into_blocks(text: &str, block_size: usize, overlap: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return vec![];
    }
    let bs = block_size.max(1);
    let step = bs.saturating_sub(overlap).max(1);
    let mut blocks = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let end = (i + bs).min(chars.len());
        let piece: String = chars[i..end].iter().collect();
        let trimmed = piece.trim().to_string();
        if !trimmed.is_empty() {
            blocks.push(trimmed);
        }
        if end >= chars.len() {
            break;
        }
        i += step;
    }
    if blocks.is_empty() {
        blocks.push(text.to_string());
    }
    blocks
}

/// 把章节正文切成带章节号的证据块。
pub fn distill_characters_system_prompt() -> String {
    r#"你是资深小说角色分析专家。我会提供若干段经过向量检索召回的小说原文证据，请基于这些证据为指定角色建立"认知卡"。

## 全局约束(所有模块适用)
1. 只许从提供的原文证据中提炼，绝不编造、不脑补剧情之外的信息；每条结论必须能在证据里找到依据。
2. 证据等级参考（若证据覆盖则优先采用）：
   ① 代表金句 —— 最能体现其性格与说话方式的原文台词（性格/声线归纳**优先**使用台词证据）
   ② 信念形成故事 —— 角色"我信X是因为Y"的来源
   ③ 关键决策与复盘 —— 角色在节点上做过什么选择、之后如何反思
   ④ 失败与承认 —— 角色承认过错、示弱的时刻
   ⑤ 内在矛盾 —— 言行不一、愿望与压抑的冲突
   ⑥ 认知边界 —— 角色不知道、不愿承认或误解的事
   ⑦ 思维习惯 —— 反复出现的判断方式/口头推理
3. 模块按顺序执行，每个模块先判断"证据是否支撑"，不支撑则按该模块降级规则处理，不得跳过后端模块。

## 模块契约
### M1 身份模块 —— 回答"角色是谁"
- 输入:证据中的称谓/身份/场景描述
- 产出:name(角色名), identity(身份), gender(性别), appearance(外貌)
- gender 降级:证据无法判断("他/她/母亲/父亲"等可推)写 "未知"
- appearance 降级:证据没有外貌描写写 "未知"，绝不脑补
### M2 性格模块 —— 回答"角色怎么处事"
- 输入:行为/决策/复盘证据 + **代表金句（证据等级①，优先）**
- 产出:
  - personality(2-4句，从具体行为与决策归纳，不要标签堆砌；**必须涵盖性格的"带刺面"与"柔软面"两面**，若原文存在直接/泼辣/强硬等互动证据，不得只保留概括性印象)
  - voice_profile(声线特征，1-2句：角色**默认语气**——如"泼辣嗔怪""嘴硬心软""阴沉寡言"——必须包含至少 1 条来自代表金句的证据支撑；严禁用"时而…时而…"稀释成多变无默认)
  - speech_style(口头禅/句式/用词/称谓习惯；**双段式**："默认语气：<一句话>" + "变化范围：<在什么情境下语气怎么变>")
- 约束:性格结论必须能回指证据；**声线结论必须优先引用台词原文（代表金句），不得只依据行为归纳**
### M3 台词模块 —— 回答"角色怎么说话"
- 输入:原文对话证据
- 产出:example_dialogs(必须是原文中出现过的台词，可适度截取), boundaries(该角色绝不会做、或正文中明示的行为底线/禁忌)
### M4 动机模块 —— 回答"角色为什么这么做"
- 产出:motivation(深层动机与目标层级：外显目标 → 内在渴望 → 恐惧/创伤驱动)
- 约束:三层都要有证据支撑，无证据的层级写"证据不足"
### M5 关系模块 —— 回答"角色和谁什么关系"
- 产出:relationships(格式"他人名:关系描述"，需能回到原文)
### M6 认知模块 —— 回答"角色怎么看世界"
- 产出:mental_models(心智模型，3-7条；格式"模型名：一句话描述（适用场景，证据ch）")
- 产出:decision_heuristics(决策启发式，5-10条；格式"规则名:触发场景→做法（案例证据ch）")
- 产出:beliefs(信念形成故事，2-5条；格式"信念:形成它的故事/经历（证据ch）")
### M7 演出模块 —— 回答"角色怎么开场"
- 产出:opening_scene(开场场景画布 2-4 句：地点/时间/在场人物/环境氛围/角色状态，从最常见的出场情境提炼，证据无则"未知")
- 产出:opening_lines(完整开场白 first_mes，60-200字，第一人称沉浸式。必须依次含：①场景画布 ②角色视角与即时情境 ③对方可行动的位置 ④关键约束或悬念 ⑤信息收尾。体现角色声音，不剧透后续情节。证据不足时用证据能支撑的表演，绝不脑补未出现的事实)
- ST-26 开场时间线守卫: opening_scene 与 opening_lines 只能取自该角色**最早出场章节（全书前 1/3 内、且为最早出现的日常/平静场景）**中"已经发生"的画面；**严禁引用后续章节才发生的剧情事件、亲密/越界/冲突场景、或任何带"将来时"标记的锁定节拍内容**。若该角色前 1/3 章节无合适日常场景，opening_lines 用最朴素的"角色在场+可行动"收尾，宁可平淡不可剧透。
- 产出:nsfw_profile(该角色剧情相关的敏感度判定边界：依据原文证据列出"什么内容算露骨/接吻以上/性描写"与"什么只算日常/暧昧/non 敏感"，格式"敏感点→判定边界"，证据不足以判断写"证据不足")
### M8 证据模块 —— 回答"结论从哪来"
- 产出:evidence_refs(每条结论的证据出处，格式"ch12:block3"或"ch34"，章节代号按证据前缀)
- 约束:所有字段的结论都应能在 evidence_refs 找到对应

只输出 JSON，不要任何解释性文字。JSON 结构（严格）：
{"name":"角色名","identity":"身份","gender":"性别或未知","appearance":"外貌或未知","opening_scene":"开场场景画布","opening_lines":"完整开场白 first_mes","personality":"2-4句","voice_profile":"声线特征（默认语气，含代表金句支撑）","speech_style":"默认语气+变化范围双段式","example_dialogs":["原文台词1","原文台词2"],"boundaries":["行为底线/禁忌/绝不做"],"motivation":"深层动机与目标层级","relationships":["他人名:关系描述"],"evidence_refs":["ch12:block3","ch34"],"mental_models":["模型名:一句话描述（证据ch）"],"decision_heuristics":["规则:触发场景→做法（证据ch）"],"beliefs":["信念:形成故事（证据ch）"],"nsfw_profile":"敏感度判定边界"}"#
        .to_string()
}

/// 在阻塞线程池里批量调用本地 embed。
pub(crate) fn build_evidence_blocks(chapters: &[(String, String)]) -> Vec<EvidenceBlock> {
    let mut out = Vec::new();
    for (ch_id, body) in chapters {
        for blk in split_into_blocks(body, BLOCK_SIZE, BLOCK_OVERLAP) {
            out.push(EvidenceBlock {
                chapter: ch_id.clone(),
                text: blk,
            });
        }
    }
    out
}

/// 按名字+别名从章节正文召回 top-k 原文证据片段。
///
/// - 逐章把正文切成 ~800 字重叠 100 的证据块；
/// - 对每章用 `embed_many` 一次批量向量化（"每章也只 embed 一次"）；
/// - 检索查询（名字+别名）做一次 embedding；
/// - 与全部块向量按余弦相似度 top-k，拼接返回原文片段，每段带 【chXX】 前缀。
pub(crate) async fn embed_blocking(texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    tokio::task::spawn_blocking(move || crate::embed_local::embed_many(&texts))
        .await
        .map_err(|e| format!("embed join: {e}"))?
}

/// 从 LLM 返回 JSON 中安全取值：snake_case 优先，兼容 camelCase。
pub async fn retrieve_character_evidence(
    _state: &AppState,
    chapters: &[(String, String)],
    name: &str,
    aliases: &[String],
    top_k: usize,
) -> Result<String, String> {
    let blocks = build_evidence_blocks(chapters);
    if blocks.is_empty() {
        return Ok(String::new());
    }

    // 检索查询 = 名字 + 别名
    let mut query = name.trim().to_string();
    for a in aliases {
        let t = a.trim();
        if !t.is_empty() {
            if !query.is_empty() {
                query.push(' ');
            }
            query.push_str(t);
        }
    }
    if query.trim().is_empty() {
        query = "主角".to_string();
    }
    // [P8 D4 2026-08-16] 检索召回补充声线维度：查询词叠加说话动作/语气词，
    // 让台词块（角色怎么说话）与情节块（角色做了什么）都能被召回——
    // 否则蒸馏输入偏向情节，性格归纳拿不到泼辣原文台词（声线漂移源头之一）。
    query.push_str(" 说话 说 骂 吼 嗔怪 语气 喊 叫 嘀咕 抱怨");

    // 查询向量化
    let qvec = embed_blocking(vec![query.clone()])
        .await
        .and_then(|mut v| v.pop().ok_or_else(|| "查询向量为空".to_string()))
        .map_err(|e| format!("角色证据检索[{name}]: embed 查询失败: {e}"))?;

    // 逐章批量向量化（每章一次 embed_many），据此填回所有块的向量
    let mut embs: Vec<Vec<f32>> = vec![vec![]; blocks.len()];
    let mut by_chapter: std::collections::BTreeMap<String, Vec<usize>> = Default::default();
    for (i, b) in blocks.iter().enumerate() {
        by_chapter.entry(b.chapter.clone()).or_default().push(i);
    }
    for (_, idxs) in by_chapter {
        let texts: Vec<String> = idxs.iter().map(|&i| blocks[i].text.clone()).collect();
        let chunk = embed_blocking(texts)
            .await
            .map_err(|e| format!("角色证据检索[{name}]: embed 章节块失败: {e}"))?;
        for (j, vec) in chunk.into_iter().enumerate() {
            if let Some(&bi) = idxs.get(j) {
                embs[bi] = vec;
            }
        }
    }

    // 余弦 top-k 并拼接证据
    // [fix 2026-08-15 实体精确命中加权] embedding 对实体歧义脆弱：查询「宿舍阿姨」
    // 的 top-k 会被「阿姨=庄眉」的高频块占据（ch09「你喜欢阿姨什么」排第 1），
    // 真实巡逻块被挤出 top-6（兔子想吃窝边草实证：宿舍阿姨卡误判"没有直接出场"）。
    // 修复：块内出现角色名（或任意别名）精确子串的，相似度加固定权重，
    // 保证实体真实出现的块必然进 top-k。
    let mut scored: Vec<(usize, f32)> = embs
        .iter()
        .enumerate()
        .map(|(i, e)| (i, cosine_sim(&qvec, e)))
        .collect();
    let mut entity_terms: Vec<String> = vec![name.trim().to_string()];
    for a in aliases {
        let t = a.trim();
        if !t.is_empty() {
            entity_terms.push(t.to_string());
        }
    }
    // [fix 2026-08-15 2] 只对 ≥3 字的术语做实体加权：2 字别名（「阿姨」）高频歧义
    // （全书指庄眉），若加权会把「阿姨=庄眉」块抬进 top-k，违背实体精确意图。
    // 「宿舍阿姨」4 字全名精确出现才 +0.25，真实巡逻块必进。
    let entity_bonus = 0.25f32; // 实体出现块的加权重（≈相似度 0.25 的抬升）
    for idx in 0..scored.len() {
        let blk_text = &blocks[scored[idx].0].text;
        if entity_terms
            .iter()
            .any(|t| t.chars().count() >= 3 && blk_text.contains(t.as_str()))
        {
            scored[idx].1 += entity_bonus;
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    let hits = scored;
    if hits.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::new();
    for (i, _score) in hits {
        out.push_str(&format!("【{}】{}\n\n", blocks[i].chapter, blocks[i].text));
    }
    Ok(out.trim().to_string())
}

/// 构造"角色谱"步骤的 LLM 输入：书名 + 每章摘要（整体截断到 max_chars）。
pub(crate) fn fallback_evidence_scan(
    chapters: &[(String, String)],
    name: &str,
    aliases: &[String],
    max_snippets: usize,
) -> String {
    let mut needles: Vec<String> = {
        let mut v = vec![name.trim().to_string()];
        for a in aliases {
            let t = a.trim();
            if !t.is_empty() {
                v.push(t.to_string());
            }
        }
        v
    };
    needles.retain(|s| !s.is_empty());
    if needles.is_empty() {
        needles.push("主角".to_string());
    }

    let mut out = String::new();
    let mut found = 0usize;
    'outer: for (cid, body) in chapters {
        for nd in &needles {
            if let Some(pos) = body.find(nd.as_str()) {
                // 前后各取 150 字符，char-safe（pos 是 find 返回的边界）
                let prefix: String = body[..pos].chars().rev().take(150).collect::<Vec<_>>().into_iter().rev().collect();
                let suffix: String = body[pos..].chars().take(150).collect();
                out.push_str(&format!("【{cid}】…{}{}…\n\n", prefix, suffix));
                found += 1;
                if found >= max_snippets {
                    break 'outer;
                }
                break; // 每章只取第一处，避免同名片段灌满
            }
        }
    }
    out.trim().to_string()
}

/// 把角色认知卡 JSON 填充成 `PackCharacterRef`。
pub(crate) fn build_roster_input(title: &str, chapters: &[(String, String)], max_chars: usize) -> String {
    let mut out = format!("小说《{title}》章节正文摘要：\n");
    let budget = max_chars;
    let mut used = out.chars().count();
    // A0 完整版 (2026-08-19): 章节前缀 chXX 用原著章号（缺章自动跳号），与 build_pack_from_chapters
    // 的 pack.chapters.id 同一编号体系——保证事件卡蒸馏标注的 chapterRange 与运行时 cursor 对齐，
    // 修复「原著标题 vs 切分序号」错位（A1 锚定 + D1）。此前用切分序号 i+1，源缺章时蒸馏标号错位。
    let chapter_ids = crate::crawler::chapter_id_seq(chapters);
    // [fix 2026-08-15 角色谱全量覆盖] 原实现按序逐章喂到预算耗尽——13 章书 6000 字预算
    // 只覆盖前 ~4 章，后段角色（红姐/山楂/冯婷/莫旺财）LLM 根本看不见，导致角色谱永远
    // 只有前几章主角。改为均匀分层采样：预算内均匀取 ≤12 个采样点覆盖全书。
    // [fix 2026-08-15 2] ≤24 章全喂：12 采样点覆盖 13 章会漏 1 章（ch12 缺失实证）。
    // 24 章以内全部章节都喂（每章均分预算），24 章以上才分层采样。
    if chapters.len() <= 24 {
        // 中短书全喂（每章截断到预算均分，保证每章都可见不漏章）
        let per = (budget.saturating_sub(used) / chapters.len().max(1)).max(100);
        for (i, (cid, body)) in chapters.iter().enumerate() {
            if used >= budget {
                break;
            }
            let part: String = body
                .trim()
                .chars()
                .take(per)
                .take(budget.saturating_sub(used))
                .collect();
            // A1: 章节前缀带 chXX 切分序号（ch01 起按输入顺序），与运行时 chapter_cursor 编号对齐，
            // 供事件卡蒸馏标注 chapterRange 时使用同一编号体系（避免「原著标题 vs 切分序号」语义错位）。
            let label = format!("{}:{cid}", chapter_ids[i]);
            out.push_str(&format!("【{label}】{part}\n"));
            used += label.chars().count() + part.chars().count() + 4;
        }
    } else {
        // 长书分层采样：均匀取 ≤12 个采样点（开头/1/11…结尾）。
        // [fix 2026-08-15 预算均分] 原实现每点全量 1600 字，6000 字预算只够
        // 喂 3-4 个点就 break——13 章书只覆盖前 4 章（兔子想吃窝边草实证：
        // LLM 只看到 ch01-04，红姐/山楂/冯婷/莫旺财全在 ch05+ 看不见）。
        // 改为预算均分给全部采样点：每点 ~500 字，12 个点全部可见。
        let n = chapters.len();
        let mut sampled: Vec<usize> = Vec::new();
        for k in 0..12 {
            sampled.push((n - 1) * k / 11);
        }
        sampled.dedup();
        let per = (budget.saturating_sub(used) / sampled.len().max(1)).max(100);
        for idx in sampled {
            if used >= budget {
                break;
            }
            let (cid, body) = &chapters[idx];
            let part: String = body
                .trim()
                .chars()
                .take(per)
                .take(budget.saturating_sub(used))
                .collect();
            // A1：长书采样同样带 chXX 序号前缀。
            let label = format!("{}:{cid}", chapter_ids[idx]);
            out.push_str(&format!("【{label}】{part}\n"));
            used += label.chars().count() + part.chars().count() + 4;
        }
    }
    out
}

/// 递归检查一个 LLM 产出的 JSON `Value` 里是否含 U+FFFD(替换符)。
/// 上游模型会偶发把单字损坏成 FFFD，此类内容应丢弃重试而非落盘。
pub(crate) fn extract_evidence_chapters(evidence: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for line in evidence.lines() {
        let Some(start) = line.find('【') else { continue };
        let after = &line[start..];
        let Some(rest) = after.strip_prefix('【') else { continue };
        if let Some(end) = rest.find('】') {
            let tag = rest[..end].trim().to_string();
            if !tag.is_empty() && !seen.contains(&tag) {
                seen.push(tag);
            }
        }
    }
    seen
}