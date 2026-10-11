//! convert/characters —— 角色卡蒸馏（单卡/整包/具名角色 + ST26 开场守卫）
#![allow(unused_imports)]

use super::*;

/// 从召回证据字符串里提取涉及的章节代号（形如 "ch01"）。
pub(crate) fn card_to_ref(idx: usize, name: &str, importance: &str, card: &Value) -> PackCharacterRef {
    let identity = jstr(card, "identity", "identity");
    let role = if identity.is_empty() {
        if importance == "high" {
            "protagonist".to_string()
        } else {
            "supporting".to_string()
        }
    } else {
        identity
    };
    PackCharacterRef {
        id: format!("c-distil-{idx}"),
        name: name.to_string(),
        role,
        gender: jstr(card, "gender", "gender"),
        appearance: jstr(card, "appearance", "appearance"),
        opening_scene: jstr(card, "opening_scene", "openingScene"),
        opening_lines: jstr(card, "opening_lines", "openingLines"),
        importance: importance.to_string(),
        content_tier: None,
        example_dialogs: jstr_arr(card, "example_dialogs", "exampleDialogs"),
        boundaries: jstr_arr(card, "boundaries", "boundaries"),
        personality: jstr(card, "personality", "personality"),
        voice_profile: jstr(card, "voice_profile", "voiceProfile"),
        speech_style: jstr(card, "speech_style", "speechStyle"),
        motivation: jstr(card, "motivation", "motivation"),
        relationships: jstr_arr(card, "relationships", "relationships"),
        evidence_refs: jstr_arr(card, "evidence_refs", "evidenceRefs"),
        mental_models: jstr_arr(card, "mental_models", "mentalModels"),
        decision_heuristics: jstr_arr(card, "decision_heuristics", "decisionHeuristics"),
        beliefs: jstr_arr(card, "beliefs", "beliefs"),
        nsfw_profile: jstr(card, "nsfw_profile", "nsfwProfile"),
        expressions: Default::default(),
            voice: None,
            archive: None,
        avatar: None,
        starting_wardrobe: Default::default(),
    }
}

/// 角色名合法性过滤：拦截 LLM 角色清单里的正文碎片/短语/普通名词。
///
/// 背景（2026-08-13 乱码名专项）：LLM 抽取角色名时把句子片段当人名
/// （「冲进下水」「过电车轨」「那就」「纷纷询」等），产生无内容的空壳卡
/// 污染 pack。判定规则（误杀控制：真名/称呼 苏早/沈棠知/光头男/外婆/
/// 林小宇/弗雷德/王一博/李进 均不命中）：
/// - 长度 1..=12；不含标点/空白/引号
/// - 高频虚词单字命中且**不在末位** → 拒（末位容忍单字名如「李进」）
/// - 句子碎片黑名单子串命中 → 拒
/// - 首字为典型动作动词 → 拒（「听早早」）
/// ST-26 校验层 (2026-08-18): 蒸馏产出 opening_scene 的确定性违规检测。
/// 背景实证：度蜜月 08-18 04:49 在 ST-26 prompt 在线的蒸馏仍产出「蜜月套房/接沈雨棠电话」
/// 后期场景卡——纯 prompt 约束 LLM 可不遵守，需代码兜底。
/// 检测信号（保守，宁可回退不可剧透）：
/// ① 后期场景词：酒店/套房/机场/蜜月/海边/海滩/度假村/客房
/// ② 剧透后缀：另：/另注/深夜/电话/将（将来时叙事）出现在开场画布
/// 命中任一 → 违规（蒸馏侧回退「证据不足」，不脑补后期画面）。
pub(crate) async fn distill_one_character_card<F, Fut>(
    name: &str,
    importance: &str,
    mut attempt: F,
) -> Option<Value>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    // B4: 网络错误(conn_err)与坏字(fffd)分开计数，各有独立上限。
    // fffd 配额按重要性分级：high 10 / medium 6 / low 4；网络错误单独给上限 6。
    // 注:deepseek 长中文输入下 FFFD 复现率实测可达 ~33%,high 给 10 次(0.33^10≈0.002%)
    // 确保主角几乎必然出卡;low 角色少轮节省成本。
    let max_fffd_rounds: u32 = match importance.trim().to_lowercase().as_str() {
        "high" => 10,
        "medium" => 6,
        _ => 4,
    };
    let max_conn_err_rounds: u32 = match importance.trim().to_lowercase().as_str() {
        "high" | "medium" => 6,
        _ => 2, // 边缘角色网络错误快速放弃，避免单角色拖死整批蒸馏
    }; // 网络错误重试上限（独立于 fffd 配额）
    let is_high = importance.trim().to_lowercase() == "high";

    let mut round_fffd: u32 = 0; // fffd / 空响应 / JSON 解析失败
    let mut round_conn: u32 = 0; // 网络错误 / 调用失败

    loop {
        let done_fffd = round_fffd >= max_fffd_rounds;
        let done_conn = round_conn >= max_conn_err_rounds;
        if done_fffd || done_conn {
            if done_fffd {
                tracing::warn!(
                    name = %name, round_fffd, round_conn,
                    "角色蒸馏[{name}] fffd 配额耗尽({round_fffd}/{max_fffd_rounds})"
                );
            } else {
                tracing::warn!(
                    name = %name, round_fffd, round_conn,
                    "角色蒸馏[{name}] 网络错误配额耗尽({round_conn}/{max_conn_err_rounds})"
                );
            }
            break;
        }
        let ok = match attempt().await {
            Ok(raw) => raw,
            Err(e) => {
                round_conn += 1;
                tracing::warn!(
                    name = %name, round_conn, round_fffd,
                    "角色蒸馏[{name}] 调用失败: {e}，重试 (round_conn={round_conn})"
                );
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
        };
        if ok.trim().is_empty() {
            round_fffd += 1;
            tracing::warn!(
                name = %name, round_fffd, round_conn,
                "角色蒸馏[{name}] 空响应，重试 (round_fffd={round_fffd})"
            );
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }
        if let Some(mut v) = crate::llm_stream::extract_json_value(&ok) {
            if json_has_fffd(&v) {
                round_fffd += 1;
                tracing::warn!(
                    name = %name, round_fffd, round_conn,
                    "角色蒸馏[{name}] 输出含 U+FFFD 损坏字，丢弃重试 (round_fffd={round_fffd})"
                );
                // [DIAG] 首次 fffd 落盘原始返回，定位是模型噪声还是服务端解析问题
                // [fix 审计] 不再写 CWD：本函数无 pack 上下文 → $KALEIDO_DATA/diag/
                if round_fffd == 1 {
                    diag_dump(
                        None,
                        "fffd-raw-dump.json",
                        &serde_json::json!({
                            "name": name,
                            "raw": ok,
                            "raw_len": ok.chars().count(),
                            "fffd_count": ok.chars().filter(|c| *c == '\u{FFFD}').count(),
                        }),
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            if json_is_empty_shell(&v) {
                round_fffd += 1;
                tracing::warn!(
                    name = %name, round_fffd, round_conn,
                    "角色蒸馏[{name}] 输出为空壳 JSON（核心字段全空），丢弃重试 (round_fffd={round_fffd})"
                );
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                continue;
            }
            // ST-26 校验层 (2026-08-18): opening_scene 确定性违规检测——后期场景/剧透信号
            // → 回退「证据不足」（宁可平淡不可剧透）。实证：度蜜月 04:49 蒸馏违反 ST-26
            // （蜜月套房 + 「另：接沈雨棠电话」），纯 prompt 约束 LLM 不遵守，加代码兜底。
            if let Some(map) = v.as_object_mut() {
                let os_violates = map
                    .get("opening_scene")
                    .and_then(|x| x.as_str())
                    .map(opening_scene_violates_st26)
                    .unwrap_or(false);
                if os_violates {
                    tracing::warn!(
                        name = %name,
                        "角色蒸馏[{name}] opening_scene 违反 ST-26（后期场景/剧透），回退证据不足"
                    );
                    map.insert(
                        "opening_scene".into(),
                        serde_json::Value::String("证据不足，无合适开场场景（ST-26 守卫回退）".into()),
                    );
                    map.insert(
                        "opening_lines".into(),
                        serde_json::Value::String("（该角色尚未有合适的日常开场场景，等待剧情展开。）".into()),
                    );
                }
            }
            return Some(v);
        }
        round_fffd += 1;
        tracing::warn!(
            name = %name, round_fffd, round_conn,
            "角色蒸馏[{name}] JSON 解析失败，重试 (round_fffd={round_fffd})"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // B5: high 角色保底兜底 — fffd 配额耗尽后，再尝试 2 次，力求至少产出最小卡
    if is_high {
        tracing::warn!(
            name = %name, round_fffd, round_conn,
            "角色蒸馏[{name}] 启动 high 角色降级保底"
        );
        for fallback_round in 0..2u32 {
            match attempt().await {
                Ok(raw) => {
                    if raw.trim().is_empty() {
                        continue;
                    }
                    if let Some(v) = crate::llm_stream::extract_json_value(&raw) {
                        if json_has_fffd(&v) {
                            continue;
                        }
                        if json_is_empty_shell(&v) {
                            tracing::warn!(
                                name = %name, fallback_round,
                                "角色蒸馏[{name}] 降级保底输出为空壳 JSON，继续尝试 (fallback_round={fallback_round})"
                            );
                            continue;
                        }
                        tracing::warn!(
                            name = %name, fallback_round,
                            "角色蒸馏[{name}] 降级保底成功，产出最小卡"
                        );
                        return Some(v);
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        name = %name, fallback_round,
                        "角色蒸馏[{name}] 降级保底调用失败: {e}"
                    );
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        tracing::warn!(
            name = %name,
            "角色蒸馏[{name}] 降级保底仍无法产出认知卡，跳过"
        );
    } else {
        tracing::warn!(name = %name, "角色蒸馏[{name}] 重试后仍无法产出认知卡，跳过");
    }
    None
}

pub fn opening_scene_violates_st26(os: &str) -> bool {
    const LATE_WORDS: [&str; 12] = [
        "酒店", "套房", "机场", "蜜月", "海边", "海滩", "度假村", "客房", "月见", "领证后",
        "海岛", "度假",
    ];
    if LATE_WORDS.iter().any(|w| os.contains(w)) {
        return true;
    }
    const SPOILER_MARKERS: [&str; 6] = ["另：", "另注", "深夜", "后来", "之后", "将要"];
    SPOILER_MARKERS.iter().any(|m| os.contains(m))
}

/// 完整角色蒸馏流程：产出有血有肉的 `PackCharacterRef` 列表。
///
/// a) 让 LLM 从全书摘要产出"角色谱 JSON"；
/// b) 按 importance 高 / 前 ≤[`MAX_DISTIL_CHARS`] 个逐个召回证据、蒸馏认知卡；
/// c) 每个卡带 evidence_refs。
pub(crate) fn is_plausible_character_name(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() {
        return false;
    }
    let chars: Vec<char> = n.chars().collect();
    let len = chars.len();
    if len > 12 {
        return false;
    }
    // 标点/空白/引号/控制字符
    if chars.iter().any(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(
                *c,
                '，' | '。' | '！' | '？' | '、' | '；' | '：' | '“' | '”' | '‘' | '’' | '（' | '）'
                    | '《' | '》' | '〈' | '〉' | '【' | '】' | '…' | '·' | '-' | '_' | ',' | '.'
                    | ';' | ':' | '!' | '?' | '"' | '\'' | '`' | '/' | '\\' | '|' | '~' | '@'
                    | '#' | '$' | '%' | '^' | '&' | '*' | '=' | '+' | '[' | ']' | '(' | ')' | '{'
                    | '}'
            )
    }) {
        return false;
    }
    // 高频虚词单字（命中且不在末位 → 拒）。刻意不含「一」（王一博/周一）「小」「子」。
    const VIRTUAL: &str = "的了着过在把被向从到就都也还又很更最和与或而且但是为对给让那这个里中后前旁进出上下起像如有没不别要会能我你他她它们种样般地得并已曾正才只再却若虽因果则各每某另将既";
    // [fix 2026-08-15] 音译名容忍：加里克斯/格列佛/萨拉丁 这类 3+ 字名字含「里/斯/尔/格/克/特/罗/拉/尼/卡」
    // 是正常音译字符，不能因 VIRTUAL 里的「里」误杀。检测到音译特征字 ≥1 个时跳过虚词检查。
    const TRANSLIT: &str = "里斯尔格克特罗拉尼卡穆奥芬恩德";
    let has_translit = chars.iter().filter(|c| TRANSLIT.contains(**c)).count() >= 2;
    if !has_translit {
        for (i, c) in chars.iter().enumerate() {
            if i < len - 1 && VIRTUAL.contains(*c) {
                // [fix 2026-08-15] 姓氏豁免：3 字以上名字首字命中虚词可能是姓氏
                // （向明初 = 向+明初，向是姓；「那怪/那就」2 字仍严格拦截）。
                // 2 字名严格：那怪/那小子 这类泛称不得放行。
                if i == 0 && len >= 3 {
                    continue;
                }
                return false;
            }
        }
    }
    // 句子碎片黑名单（子串命中 → 拒）
    const FRAGMENTS: &[&str] = &[
        "那就", "另一个", "像是想", "有一种", "里面", "时候", "开始", "已经", "看见", "知道", "自己",
        "什么", "怎么", "一样", "下来", "出去", "回来", "眼前", "身后", "面前", "旁边", "突然", "然后",
        "可是", "但是", "因为", "所以", "如果", "虽然", "不过", "还是", "就是", "只是", "并且", "甚至",
        "大概", "仿佛", "好像", "依然", "终于", "连忙", "赶紧", "立刻", "马上", "缓缓", "慢慢", "轻轻",
        "深深", "紧紧", "冷冷", "淡淡", "微微", "渐渐", "不断", "不停", "一直", "再也", "越来越", "纷纷",
        "粮食", "电车", "下水", "味道", "示意", "尽管", "走得", "来得", "说得", "看得", "想着", "看着",
        "那里", "这里", "起来", "进去", "下去", "上去", "离开", "走近", "抬头", "低头", "转身", "回头",
        "开口", "伸手", "点头", "摇头", "皱眉", "叹气", "微笑", "沉默", "停顿", "犹豫", "考虑", "决定",
        "想要", "觉得", "感到", "发现", "明白", "了解", "记得", "忘记", "告诉", "询问", "回答", "答应",
        "拒绝", "接受", "留下", "拉住", "抓住", "推开", "放下", "拿起", "抱起", "站起", "坐下", "躺下",
        "走过", "路过", "穿过", "越过", "冲进", "冲出", "跑进", "跑出", "走进", "走出", "退后", "上前",
        "靠近", "接近", "远离", "脸色", "声音", "目光", "眼神", "语气", "表情", "样子", "感觉", "心里", "颜色",
        "胸前", "怀里", "桌上", "门外", "屋里", "家里", "学校", "公司", "房间", "门口", "窗前",
    ];
    for f in FRAGMENTS {
        if n.contains(f) {
            return false;
        }
    }
    // 首字为典型动作动词 → 拒（「听早早」=“听，早早…”的碎片）
    const VERB_HEAD: &str = "听说看走来去冲拉留放拿问答跟随喊叫望瞧盯嗅尝吃喝玩打拍推抱背提扛抬拽拖蹬踩踢跳跑爬飞驶开关门关停站坐躺睡醒念读写画唱弹吹奏演算数记查找搜索摸碰擦洗刷扫拖抹挥舞甩掷抛投射砍劈刺戳插扎缝补织戴穿脱换洗晾晒煮炒煎炖蒸烤烧点熄灭燃";
    if VERB_HEAD.contains(chars[0]) {
        return false;
    }
    true
}

/// 指定角色蒸馏：跳过角色谱自动识别，按调用方名单直接蒸馏指定角色。
///
/// - 每个名字：向量召回证据 → 为空则词频兜底取证（调用方点名即重视，不限于 high）；
/// - importance 优先复用 pack 目录下 `roster-diag.json`（由 [`distill_pack_characters`]
///   按 pack 隔离落盘）中同名候选的标注，无则默认 "high"；
/// - 增量存档：每蒸馏完一张卡立即合并写回 pack（同名替换、其余追加）；
/// - 新卡 id 从既有 `c-distil-N` 之后续排，避免与存量卡冲突。
pub async fn distill_pack_characters(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    max_chars: usize,
    incremental_pid: Option<&str>,
) -> Result<Vec<PackCharacterRef>, String> {
    let client = reqwest::Client::new();
    let llm = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return Err("角色蒸馏: LLM 未配置".into());
    }


    // a) 角色谱：LLM 抽取角色清单（name/aliases/identity/importance）并过滤乱码名；
    let roster_input = build_roster_input(title, chapters, max_chars);
    let roster_system = "你是小说角色清单助手。从给定小说正文中识别**所有**有叙事功能的人物（排除旁白/读者/玩家），只输出 JSON 数组，每个元素为 {\"name\":\"角色名\",\"aliases\":[\"别名\"],\"identity\":\"身份\",\"appears_first\":\"首次出场章节\",\"importance\":\"high|medium|low\"}。importance 依据戏份与剧情权重判断。name 必须是明确的人物专名（姓名/称呼/外号），严禁输出句子片段、短语、普通名词或口语碎片；拿不准的名字不要输出。\
要求：1) 宁多勿漏——主角、重要配角、次要配角、有名字的龙套都要列出；\
2) 数量参考：10 章以下 ≥5 个，10-50 章 ≥10 个，50 章以上 ≥15 个；\
3) 只输出 JSON 数组，不要任何解释文字。";
    let raw = crate::llm_stream::chat_completion_dispatch(
        &llm.base_url,
        &llm.api_key,
        &llm.model,
        &state.provider_kind,
        roster_system,
        &roster_input,
        0.1, 16384, LLM_TIMEOUT_SECS,
        &client,
    )
    .await
    .map_err(|e| format!("角色蒸馏[角色谱]: {e}"))?;
    // [fix 审计·转换导出与爬虫-6] DIAG/调试产物统一按 pack 目录隔离落盘（无 pid
    // 时退 $KALEIDO_DATA/diag/），不再写进程 CWD。本函数内所有 DIAG dump 共用。
    let pack_dir = incremental_pid.and_then(|pid| state.packs.pack_dir(pid).ok());
    let roster_v = crate::llm_stream::extract_json_value(&raw).or_else(|| {
        // [DIAG] 解析失败时落盘原始返回，定位上游格式问题
        diag_dump(
            pack_dir.as_deref(),
            "roster-parse-fail.json",
            &serde_json::json!({
                "raw": raw,
                "raw_len": raw.chars().count(),
            }),
        );
        None
    })
    .ok_or_else(|| "角色蒸馏[角色谱]: 无法解析 LLM 返回的 JSON".to_string())?;
    let roster: Vec<Value> = match roster_v {
        Value::Array(a) => a,
        other => vec![other],
    };

    // 过滤 + 排序（high>medium>low） + 截断
    let rank = |im: &str| -> u8 { match im { "high" => 0, "medium" => 1, _ => 2 } };
    let mut candidates: Vec<(String, Vec<String>, String)> = Vec::new();
    for v in &roster {
        let name = jstr(v, "name", "name");
        if name.is_empty() {
            continue;
        }
        // [fix 2026-08-13 乱码名专项] 角色名合法性过滤：LLM 常把正文碎片当人名
        // （「冲进下水」「过电车轨」「那就」等），过滤后不进入蒸馏候选。
        if !is_plausible_character_name(&name) {
            tracing::warn!(name = %name, "角色蒸馏[角色谱]: 过滤疑似乱码角色名");
            continue;
        }
        let aliases = jstr_arr(v, "aliases", "aliases");
        let importance = jstr(v, "importance", "importance").to_lowercase();
        candidates.push((name, aliases, importance));
    }
    candidates.sort_by_key(|(_, _, im)| rank(im));
    // [fix 2026-08-15 关系端点补漏] 角色谱 LLM 往往只给主角（13 章书只给 3 个），
    // 但关系图谱（distill_chapter_relations）已确定性识别更多角色端点
    // （红姐/冯婷/莫旺财/浓妆女人 等）。把这些端点合并进候选，medium 优先，
    // 补漏不覆盖 LLM 已识别的高优先角色。
    // [fix 2026-08-15 去重] 同名角色会同时出现在 LLM 角色谱和关系端点里
    // （实证：冯婷×2/红姐×2/浓妆女人×2），合并前按 name 去重。
    let mut seen_names: std::collections::HashSet<String> =
        candidates.iter().map(|(n, _, _)| n.clone()).collect();
    if let Some(pid) = incremental_pid {
        let rel_path = state.packs.pack_dir(pid).ok().map(|d| d.join("relations.json"));
        if let Some(rp) = rel_path {
            if let Ok(s) = std::fs::read_to_string(&rp) {
                if let Ok(edges) = serde_json::from_str::<Vec<Value>>(&s) {
                    for e in edges {
                        for k in ["from", "to"] {
                            let nm = e.get(k).and_then(|x| x.as_str()).unwrap_or("").trim();
                            if nm.is_empty() || !seen_names.insert(nm.to_string()) {
                                continue;
                            }
                            if !is_plausible_character_name(nm) {
                                continue;
                            }
                            candidates.push((nm.to_string(), vec![], "medium".to_string()));
                        }
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|(_, _, im)| rank(im));
    let candidates: Vec<_> = candidates.into_iter().take(MAX_DISTIL_CHARS).collect();
    // [DIAG] 落盘角色谱原始返回 + 最终 candidates，供排查庄眉等主角缺卡问题。
    // [fix 2026-08-15] 按 pack 目录隔离：原全局单文件 roster-diag.json 会被
    // 其他 pack 的蒸馏覆盖，导致续跑判定读到陈旧 roster（跨 pack 污染，如
    // 兔子想吃窝边草 的向明初被旧版误杀后，旧 roster 判定"high 已覆盖"跳过重蒸馏）。
    // [fix 2026-09-28 审计] 无 pid 时的兜底也从 CWD 改为 $KALEIDO_DATA/diag/。
    // 此文件是功能性产物（crawler.rs 续跑门读它判定 high 覆盖），故不走
    // diag_dump 的 KALEIDO_DIAG 开关，保持无条件落盘。
    {
        let probe = serde_json::json!({
            "title": title,
            "raw_roster": String::from_utf8_lossy(raw.as_bytes()).to_string(),
            "candidates": candidates.iter().map(|(n,a,im)| json!({"name":n,"aliases":a,"importance":im})).collect::<Vec<_>>(),
        });
        let diag_path = pack_dir
            .as_ref()
            .map(|d| d.join("roster-diag.json"))
            .unwrap_or_else(|| diag_fallback_dir().join("roster-diag.json"));
        if let Err(e) = std::fs::write(
            &diag_path,
            serde_json::to_string_pretty(&probe).unwrap_or_default(),
        ) {
            tracing::warn!(path = %diag_path.display(), "roster-diag 落盘失败: {e}");
        }
    }
    if candidates.is_empty() {
        return Ok(vec![]);
    }

    // b) 逐角色蒸馏认知卡
    let mut cards: Vec<PackCharacterRef> = Vec::new();
    // [DIAG] 每个候选角色的处理结果追踪
    let mut diag_trace: Vec<serde_json::Value> = Vec::new();
    for (n, (name, aliases, importance)) in candidates.into_iter().enumerate() {
        let evidence =
            retrieve_character_evidence(state, chapters, &name, &aliases, EVIDENCE_TOP_K).await?;
        let evidence = evidence.trim();
        // [DIAG] 记录本角色召回情况
        diag_trace.push(serde_json::json!({
            "name": name,
            "importance": importance,
            "evidence_len": evidence.chars().count(),
            "evidence_head": evidence.chars().take(120).collect::<String>(),
            "outcome": "pending",
        }));
        if evidence.is_empty() {
            // 主角/重要角色证据召回为空：pure-text 词频兜底，避免 high 角色因嵌入检索
            // 偏移而被静默跳过（如庄眉全本高频但向量 query 未命中）。
            if importance == "high" {
                let fb = fallback_evidence_scan(chapters, &name, &aliases, 8);
                if !fb.is_empty() {
                    tracing::warn!(
                        name = %name,
                        "角色蒸馏: 向量召回为空，high 角色启用词频兜底取证"
                    );
                    // 重新进入蒸馏，用兜底证据
                    let card_user = format!(
                        "小说《{title}》\n目标角色：{name}\n\
                         检索到的原文证据（【ch】前缀指示章节）：\n{fb}\n\
                         请按系统要求为该角色输出一份严格 JSON 角色认知卡。"
                    );
                    let card_system = distill_characters_system_prompt();
                    let Some(card_v) = distill_one_character_card(
                        &name,
                        &importance,
                        || {
                            crate::llm_stream::chat_completion_dispatch(
                                &llm.base_url,
                                &llm.api_key,
                                &llm.model,
                                &state.provider_kind,
                                &card_system,
                                &card_user,
                                0.1, 16384, LLM_TIMEOUT_SECS,
                                &client,
                            )
                        },
                    )
                    .await
                    else {
                        if let Some(t) = diag_trace.last_mut() { t["outcome"] = "failed_fallback_distill".into(); }
                        continue;
                    };
                    let mut card = card_to_ref(n, &name, &importance, &card_v);
                    if card.evidence_refs.is_empty() {
                        card.evidence_refs = extract_evidence_chapters(&fb);
                    }
                    cards.push(card);
                    if let Some(t) = diag_trace.last_mut() { t["outcome"] = "ok_fallback".into(); }
                    // [增量存档] 每蒸馏完一个角色立即写回 pack，避免中断丢全部
                    if let Some(pid) = incremental_pid {
                        if let Ok(mut p) = state.packs.get(pid) {
                            p.characters = cards.clone();
                            let _ = state.packs.save(p);
                        }
                    }
                    continue;
                }
            }
            tracing::warn!(name=%name, "角色蒸馏: 无召回证据，跳过");
            if let Some(t) = diag_trace.last_mut() { t["outcome"] = "skipped_no_evidence".into(); }
            continue;
        }
        let card_user = format!(
            "小说《{title}》\n目标角色：{name}\n\
             检索到的原文证据（【ch】前缀指示章节）：\n{evidence}\n\
             请按系统要求为该角色输出一份严格 JSON 角色认知卡。"
        );
        let card_system = distill_characters_system_prompt();
        let Some(card_v) = distill_one_character_card(
            &name,
            &importance,
            || {
                crate::llm_stream::chat_completion_dispatch(
                    &llm.base_url,
                    &llm.api_key,
                    &llm.model,
                    &state.provider_kind,
                    &card_system,
                    &card_user,
                    0.1, 16384, LLM_TIMEOUT_SECS,
                    &client,
                )
            },
        )
        .await
        else {
            if let Some(t) = diag_trace.last_mut() { t["outcome"] = "failed_distill".into(); }
            continue;
        };
        let mut card = card_to_ref(n, &name, &importance, &card_v);
        if card.evidence_refs.is_empty() {
            // 兜底：用本次召回到的证据章节补足 evidence_refs
            card.evidence_refs = extract_evidence_chapters(evidence);
        }
        cards.push(card);
        if let Some(t) = diag_trace.last_mut() { t["outcome"] = "ok".into(); }
        // [增量存档] 每蒸馏完一个角色立即写回 pack，避免中断丢全部
        if let Some(pid) = incremental_pid {
            if let Ok(mut p) = state.packs.get(pid) {
                p.characters = cards.clone();
                let _ = state.packs.save(p);
            }
        }
    }
    // [DIAG] 角色蒸馏逐角色追踪落盘
    // [fix 审计] 不再写 CWD：按 pack 目录隔离，无 pid 时退 $KALEIDO_DATA/diag/
    {
        let probe = serde_json::json!({ "title": title, "trace": diag_trace, "cards": cards.iter().map(|c| c.name.clone()).collect::<Vec<_>>() });
        diag_dump(pack_dir.as_deref(), "char-diag.json", &probe);
    }
    Ok(cards)
}

/// 一次 LLM JSON 调用（复用 chat_completion_dispatch + extract_json_value）。
///
/// - `stage`：失败信息里的阶段名，方便调用方定位失败点。
/// - LLM 未配置 / 调用失败 / JSON 解析失败一律返回 Err。
pub async fn distill_named_characters(
    state: &AppState,
    title: &str,
    chapters: &[(String, String)],
    names: &[String],
    incremental_pid: Option<&str>,
) -> Result<Vec<PackCharacterRef>, String> {
    let client = reqwest::Client::new();
    let llm = state
        .app_state
        .resolve_llm(state.llm_base.as_deref(), state.llm_key.as_deref(), &state.llm_model);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return Err("角色蒸馏: LLM 未配置".into());
    }
    let names: Vec<String> = names
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if names.is_empty() {
        return Ok(vec![]);
    }

    // 复用最近一次角色谱的 aliases/importance（若名单角色曾进过谱，取其别名与权重）
    // [fix 审计·转换导出与爬虫-6] 必须读同一 pack 目录下的 roster-diag.json——
    // distill_pack_characters 已把该文件按 pack 目录隔离落盘（crawler.rs 续跑门也
    // 只认 pack 目录路径）。原实现读进程 CWD 的全局单文件：多书并发/换书重跑时，
    // 指定角色蒸馏读到的是别的书或陈旧 roster，aliases/importance 跨书污染。
    // 无 pack 上下文时 known 保持空 → importance 走默认 "high"、别名为空（文档化缺省）。
    let mut known: std::collections::HashMap<String, (Vec<String>, String)> = Default::default();
    let roster_candidates: Vec<Value> = incremental_pid
        .and_then(|pid| state.packs.pack_dir(pid).ok())
        .map(|d| d.join("roster-diag.json"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("candidates").and_then(|c| c.as_array()).cloned())
        .unwrap_or_default();
    for c in &roster_candidates {
        let n = c
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if n.is_empty() {
            continue;
        }
        let aliases = c
            .get("aliases")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let imp = c
            .get("importance")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        known.insert(n, (aliases, imp));
    }

    // 新卡 id 起点：既有 c-distil-N 最大值 + 1
    let start_idx = incremental_pid
        .and_then(|pid| state.packs.get(pid).ok())
        .map(|p| {
            p.characters
                .iter()
                .filter_map(|c| {
                    c.id
                        .strip_prefix("c-distil-")
                        .and_then(|s| s.parse::<usize>().ok())
                })
                .max()
                .map(|m| m + 1)
                .unwrap_or(0)
        })
        .unwrap_or(0);

    let mut cards: Vec<PackCharacterRef> = Vec::new();
    // [DIAG] 每个指定角色的处理结果追踪
    let mut diag_trace: Vec<serde_json::Value> = Vec::new();
    for (n, name) in names.iter().enumerate() {
        let (aliases, imp) = known
            .get(name)
            .cloned()
            .unwrap_or_else(|| (Vec::new(), "high".to_string()));
        let importance = if imp.trim().is_empty() {
            "high".to_string()
        } else {
            imp
        };
        let evidence = retrieve_character_evidence(state, chapters, name, &aliases, EVIDENCE_TOP_K)
            .await?
            .trim()
            .to_string();
        diag_trace.push(serde_json::json!({
            "name": name,
            "importance": importance,
            "evidence_len": evidence.chars().count(),
            "evidence_head": evidence.chars().take(120).collect::<String>(),
            "outcome": "pending",
        }));
        // 向量召回为空 → 词频兜底取证（点名角色一视同仁，不限于 high）
        let effective = if evidence.is_empty() {
            let fb = fallback_evidence_scan(chapters, name, &aliases, 8);
            if !fb.is_empty() {
                tracing::warn!(name = %name, "指定角色蒸馏: 向量召回为空，启用词频兜底取证");
            }
            fb
        } else {
            evidence
        };
        if effective.trim().is_empty() {
            tracing::warn!(name = %name, "指定角色蒸馏: 无召回证据，跳过");
            if let Some(t) = diag_trace.last_mut() {
                t["outcome"] = "skipped_no_evidence".into();
            }
            continue;
        }
        let card_user = format!(
            "小说《{title}》\n目标角色：{name}\n\
             检索到的原文证据（【ch】前缀指示章节）：\n{effective}\n\
             请按系统要求为该角色输出一份严格 JSON 角色认知卡。"
        );
        let card_system = distill_characters_system_prompt();
        let Some(card_v) = distill_one_character_card(
            &name,
            &importance,
            || {
                crate::llm_stream::chat_completion_dispatch(
                    &llm.base_url,
                    &llm.api_key,
                    &llm.model,
                    &state.provider_kind,
                    &card_system,
                    &card_user,
                    0.1, 16384, LLM_TIMEOUT_SECS,
                    &client,
                )
            },
        )
        .await
        else {
            if let Some(t) = diag_trace.last_mut() {
                t["outcome"] = "failed_distill".into();
            }
            continue;
        };
        let mut card = card_to_ref(start_idx + n, name, &importance, &card_v);
        if card.evidence_refs.is_empty() {
            card.evidence_refs = extract_evidence_chapters(&effective);
        }
        cards.push(card);
        if let Some(t) = diag_trace.last_mut() {
            t["outcome"] = "ok".into();
        }
        // [增量存档] 每蒸馏完一个角色立即合并写回 pack（同名替换、其余追加）
        if let Some(pid) = incremental_pid {
            if let Ok(mut p) = state.packs.get(pid) {
                if let Some(last) = cards.last() {
                    p.characters.retain(|c| c.name != last.name);
                    p.characters.push(last.clone());
                    let _ = state.packs.save(p);
                }
            }
        }
    }
    // [DIAG] 指定角色蒸馏逐角色追踪落盘
    // [fix 审计] 不再写 CWD：按 pack 目录隔离，无 pid 时退 $KALEIDO_DATA/diag/
    {
        let probe = serde_json::json!({
            "title": title,
            "kind": "named",
            "trace": diag_trace,
            "cards": cards.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
        });
        let pack_dir = incremental_pid.and_then(|pid| state.packs.pack_dir(pid).ok());
        diag_dump(pack_dir.as_deref(), "char-diag.json", &probe);
    }
    Ok(cards)
}

// ─── Stage 3：世界树 / 节拍 / 多出口 / 世界线 ────────────────────────────────