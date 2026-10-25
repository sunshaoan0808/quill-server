//! 信息边界（Epistemic Boundary）纯函数模块。
//! 吸收自 EvolvingWorld「角色私有/公开信息分离」+ ConStory knowledge_contradictions 维度。
//! 纯函数、无 IO、无 LLM 依赖；可单测矩阵全绿。
//!
//! 两类能力：
//! 1. 玩家消息代词归一：召回/归档玩家原文进系统提示前，把第一人称锚定到玩家角色名，
//!    杜绝「她穿着自己买的蕾丝」里的「自己」被 NPC 误读成在场他人（窝边草蕾丝 case）。
//! 2. 秘密越界检测：角色发言/视角文本中出现其不在知情名单内的秘密关键词 → 违例。
//!
//! 设计依据：docs/四项目融合方案v2-源码实证-20260907.md（M3 / G5）。

/// 一条秘密边界：秘密关键词（或其别名）+ 知情者名单（角色 id/名，小写归一后比对）。
#[derive(Debug, Clone)]
pub struct SecretBoundary {
    /// 秘密标识（用于违例信息，如「林念身世」）
    pub label: String,
    /// 命中即视为「说出了该秘密」的关键词/短语列表（任一命中即可）
    pub keywords: Vec<String>,
    /// 知情者角色名列表（小写包含匹配；不在此列的角色说出即越界）
    pub knowers: Vec<String>,
}

impl SecretBoundary {
    pub fn new(label: impl Into<String>, keywords: Vec<&str>, knowers: Vec<&str>) -> Self {
        Self {
            label: label.into(),
            keywords: keywords.into_iter().map(|s| s.to_string()).collect(),
            knowers: knowers.into_iter().map(|s| s.to_lowercase()).collect(),
        }
    }

    /// 角色是否知情（名字小写子串匹配，兼容「林晚」命中「林晚清」这类带称谓的写法）。
    pub fn knows(&self, speaker: &str) -> bool {
        let s = speaker.trim().to_lowercase();
        if s.is_empty() {
            return false;
        }
        self.knowers.iter().any(|k| s.contains(k) || k.contains(&s))
    }

    /// 文本是否触及该秘密（任一关键词子串命中）。
    pub fn mentioned_in(&self, text: &str) -> bool {
        self.keywords.iter().any(|kw| text.contains(kw.as_str()))
    }
}

/// 一条信息边界违例。
#[derive(Debug, Clone, PartialEq)]
pub struct EpistemicViolation {
    /// 越界角色名
    pub speaker: String,
    /// 秘密标识
    pub secret_label: String,
    /// 命中的关键词
    pub hit_keyword: String,
}

/// 检测单个角色（说话/视角持有者）在给定文本中是否越界说出其不知情的秘密。
///
/// - `speaker`：当前发言/视角角色名（传空则不检测，返回空）。
/// - `text`：该角色产出的正文/台词。
/// - `secrets`：全部秘密边界。
///
/// 规则：文本触及某秘密关键词，且该角色不在该秘密知情名单 → 违例。
/// 知情者命中用小写子串双向包含，兼容称谓/全名写法。
pub fn epistemic_leaks(
    speaker: &str,
    text: &str,
    secrets: &[SecretBoundary],
) -> Vec<EpistemicViolation> {
    let speaker = speaker.trim();
    if speaker.is_empty() || text.trim().is_empty() || secrets.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for sb in secrets {
        if sb.knows(speaker) {
            continue; // 知情者，合法
        }
        for kw in &sb.keywords {
            if text.contains(kw.as_str()) {
                out.push(EpistemicViolation {
                    speaker: speaker.to_string(),
                    secret_label: sb.label.clone(),
                    hit_keyword: kw.clone(),
                });
                break; // 同一秘密只报一次
            }
        }
    }
    out
}

/// 玩家消息第一人称代词归一：把明确的第一人称锚定到玩家角色名。
///
/// 仅做**高置信、零误伤**替换：
/// - 「我自己」→「{name}」（最长匹配优先，避免被「我」规则先吃掉）
/// - 句中独立的「我」「本人」→「{name}」
/// - 「咱们/我们」→「{name}等人」（保守，保留群体语义）
///
/// **不动「自己」**：「自己」是照应代词，NPC 台词里也大量出现（「他自己」），
/// 盲替换会误伤；「自己」的视角锚定由注入端加前缀标注解决（见 `player_view_prefix`）。
///
/// `player_name` 为空时原样返回（无玩家角色名可锚定）。
pub fn normalize_player_referents(text: &str, player_name: &str) -> String {
    let name = player_name.trim();
    if name.is_empty() || text.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    // 最长匹配优先：「我自己」先于「我」
    out = out.replace("我自己", name);
    // 独立「我」：用字符边界保守替换（中文无空格，按常见搭配处理，避免「我国/自我」类词误伤）
    // 仅替换作为代词独立使用的「我」——后面跟动词/标点/助词的高频代词位。
    for (from, to) in [
        ("我", name),
        ("本人", name),
    ] {
        out = replace_pronoun(&out, from, to);
    }
    out = out.replace("咱们", &format!("{}等人", name));
    out = out.replace("我们", &format!("{}等人", name));
    out
}

/// 保守替换代词「我」：跳过「我国/我军/自我」等成词，只换独立代词位。
/// 中文代词「我」后接动词/助词/标点/量词时是代词；接「国/军/校/厂/方/行」等成词语素时跳过。
fn replace_pronoun(text: &str, pron: &str, name: &str) -> String {
    if pron != "我" {
        // 「本人」之类直接替换（成词风险低）
        return text.replace(pron, name);
    }
    // 「我」后接这些字时是成词语素，不替换
    const NO_REPLACE_NEXT: &[char] = &[
        '国', '军', '校', '厂', '方', '行', '党', '家', '们', '自',
    ];
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + name.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '我' {
            let next = chars.get(i + 1).copied();
            let blocked = next.map(|c| NO_REPLACE_NEXT.contains(&c)).unwrap_or(false);
            // 前一字若为「自」，是「自我」成词，也跳过
            let prev_self = i > 0 && chars[i - 1] == '自';
            if blocked || prev_self {
                out.push('我');
            } else {
                out.push_str(name);
            }
        } else {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

/// 玩家原文注入召回上下文时的视角锚定前缀。
///
///  wi/S7 召回的玩家消息原文进系统提示前，加此前缀明确「这是玩家角色的第一人称陈述，
///  其中第一人称指代玩家角色，不是任何在场 NPC」，从注入侧根除「自己买的」被误读。
pub fn player_view_prefix(player_name: &str) -> String {
    let name = player_name.trim();
    if name.is_empty() {
        return "【以下为玩家角色的第一人称陈述，其中「我/自己」均指玩家本人，不指任何在场角色】".to_string();
    }
    format!(
        "【以下为玩家角色「{}」的第一人称陈述，其中「我/自己」均指「{}」，不指任何在场NPC】",
        name, name
    )
}

/// 召回注入时给玩家原文条目加视角锚定前缀；非玩家条目（`m-assistant-*` 等）原样返回。
///
/// 归档 uid 约定（story_tavern.rs 归档点）：`m-{role}-{hash}`，玩家消息 role=user → `m-user-*`。
/// 用于 S7/会话历史召回注入：玩家第一人称原文进系统提示前打锚，
/// 根除「自己买的」被 LLM 误读成在场 NPC（窝边草蕾丝 case）。
pub fn annotate_recall_entry(uid: &str, text: &str, player_name: &str) -> String {
    if uid.starts_with("m-user") {
        format!("{}\n{}", player_view_prefix(player_name), text)
    } else {
        text.to_string()
    }
}

// ─── 生成端信息边界 LLM 审稿（ConStory knowledge_contradictions 维度吸收）────────
// 纯规则检测缺可靠结构化秘密数据源（graph secret 事件 summary 是自由文本、actors 是
// 参与者非知情者），故生成端改用 LLM judge：对照 sys_prompt（含角色卡/关系/秘密/已发生
// 事件）判角色是否越界知情。审稿结论 append 进既有质量环 review，由 fix 阶段修订。
// 纪律照搬 ConStory：逐字引证、空清单合法、文学手法豁免、存疑从宽（压误报）。

/// 信息边界审稿 system prompt（只审不改，专查角色越界知情）。
pub fn epistemic_review_system() -> &'static str {
    "你是一位叙事信息边界审稿人。只审不改，专门检查【角色是否说出或知道了其按剧情不应知道的信息】。\n\
判定依据：对照系统约束中的角色设定、人物关系、秘密、已发生事件（谁在场、谁知情、谁被告知过）。\n\
检查三类越界：\n\
1. 角色说出了只有其他角色或玩家才知道的秘密、私事、隐藏身份、真实想法；\n\
2. 角色引用了其不在场时发生、或剧情中无人告知过它的事件；\n\
3. 角色表现出与其身份/背景不符的知识（如普通人知晓机密、外人知晓家内私事）。\n\
纪律（必须严格遵守）：\n\
- 每条问题必须引用正文逐字片段作为证据，注明是哪个角色越界、泄露了什么；\n\
- 若不存在信息边界问题，只输出「无信息边界问题」六个字，不要为找问题而找问题；\n\
- 文学手法（反讽、比喻、夸张、角色故意说谎/套话/试探、全知旁白叙述）不算问题；存疑从宽。\n\
输出格式：有问题时逐条一行，每行 `[信息边界] 角色「X」：问题说明（引用：「…」）；修改建议：…`；\
无问题时只输出「无信息边界问题」。"
}

/// 信息边界审稿 user prompt：把系统约束（知情范围依据）与待审正文喂给 judge。
pub fn build_epistemic_review_user(sys_prompt: &str, draft: &str) -> String {
    format!(
        "## 系统约束（角色设定/关系/秘密/已发生事件，是判定谁知情的依据）\n{}\n\n\
## 待审正文\n{}\n\n\
请输出信息边界审稿结果（有问题逐条引证，无问题只输出「无信息边界问题」）。",
        sys_prompt, draft
    )
}

/// 解析 judge 输出：有越界问题返回 Some(问题清单)，无问题/空输出返回 None。
///
/// 判定「无问题」：trim 后为空，或整体就是「无信息边界问题」类结论（允许前后缀语气词）。
pub fn extract_epistemic_issues(review_output: &str) -> Option<String> {
    let t = review_output.trim();
    if t.is_empty() {
        return None;
    }
    // 结论型回答：包含「无信息边界问题」且不含问题标记 → 视为无问题。
    // 含问题标记（[信息边界]/越界/泄露）即使前缀说「无问题」也以标记为准 → Some。
    let has_issue_marker = t.contains("[信息边界]") || t.contains("【信息边界】") || t.contains("越界") || t.contains("泄露");
    let declares_none = t.contains("无信息边界问题") || t.contains("未发现信息边界") || t.contains("没有信息边界");
    if declares_none && !has_issue_marker {
        return None;
    }
    Some(t.to_string())
}

// ---------------------------------------------------------------------------
// 信息边界账本（吸收自 dramatica-flow KnownInfoRecord：角色只知道亲眼见过 /
// 亲耳听到的信息；structured 记录谁在哪一章以什么来源知道了什么）。
// 与 SecretBoundary/epistemic_leaks 互补：前者是「已知边界判越界」的规则引擎，
// 这里是「谁知道了什么」的可追溯账本（供长篇连续性审计）。
// ---------------------------------------------------------------------------

/// 信息来源四值（dramatica 吸收，保持字面量一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InfoSource {
    /// 亲眼所见
    Witnessed,
    /// 道听途说
    Hearsay,
    /// 自己推断
    Deduced,
    /// 阅读文档/信件
    Document,
}

impl InfoSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            InfoSource::Witnessed => "witnessed",
            InfoSource::Hearsay => "hearsay",
            InfoSource::Deduced => "deduced",
            InfoSource::Document => "document",
        }
    }
}

/// 一条「角色知道了什么」记录（dramatica 吸收）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KnownInfoRecord {
    /// 谁知道
    pub character_id: String,
    /// 信息标识（如 `protagonist_true_identity`）
    pub info_key: String,
    /// 具体内容
    pub content: String,
    /// 在哪一章知道的
    pub learned_in_chapter: i64,
    /// 信息来源
    pub source: String,
}

impl KnownInfoRecord {
    pub fn new(
        character_id: &str,
        info_key: &str,
        content: &str,
        learned_in_chapter: i64,
        source: InfoSource,
    ) -> Self {
        Self {
            character_id: character_id.trim().to_string(),
            info_key: info_key.trim().to_string(),
            content: content.to_string(),
            learned_in_chapter,
            source: source.as_str().to_string(),
        }
    }
}

/// 角色信息账本（dramatica 吸收：`character_knows()` 查重 + 按角色/章节筛选）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InfoLedger {
    pub records: Vec<KnownInfoRecord>,
}

impl InfoLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// 角色是否已知该信息（dramatica `character_knows` 语义，防重复记录）。
    pub fn knows(&self, character_id: &str, info_key: &str) -> bool {
        self.records.iter().any(|r| {
            r.character_id == character_id.trim() && r.info_key == info_key.trim()
        })
    }

    /// 记录一条已知信息（重复则忽略）。
    pub fn learn(&mut self, rec: KnownInfoRecord) -> bool {
        if self.knows(&rec.character_id, &rec.info_key) {
            return false;
        }
        self.records.push(rec);
        true
    }

    /// 某角色在某章前知道的全部信息（供「该角色此刻不知道的事」判定）。
    pub fn known_before_chapter(&self, character_id: &str, chapter: i64) -> Vec<&KnownInfoRecord> {
        self.records
            .iter()
            .filter(|r| r.character_id == character_id.trim() && r.learned_in_chapter <= chapter)
            .collect()
    }

    /// 某角色不知道某信息（越界判定用：返回 None = 已知道，Some(key) = 不知道）。
    pub fn unknown_key<'a>(&self, character_id: &str, info_key: &'a str) -> Option<&'a str> {
        if self.knows(character_id, info_key) {
            None
        } else {
            Some(info_key.trim())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secrets() -> Vec<SecretBoundary> {
        vec![
            // 林念是主角亲女儿——只有主角与母亲知情
            SecretBoundary::new(
                "林念身世",
                vec!["林念是你亲女儿", "念念是你的亲生女儿", "亲生女儿"],
                vec!["向明初", "母亲", "苏晚"],
            ),
        ]
    }

    #[test]
    fn test_knower_not_flagged() {
        let s = secrets();
        // 知情者（主角向明初）说出秘密 → 不违例
        let v = epistemic_leaks("向明初", "他终于知道，林念是你亲女儿。", &s);
        assert!(v.is_empty(), "知情者说出秘密不应违例， got {:?}", v);
    }

    #[test]
    fn test_outsider_leak_flagged() {
        let s = secrets();
        // 不知情的 NPC（庄眉）说出秘密 → 违例
        let v = epistemic_leaks("庄眉", "庄眉笑道：林念是你亲女儿，这事我早知道了。", &s);
        assert_eq!(v.len(), 1, "不知情者泄密应报 1 条， got {:?}", v);
        assert_eq!(v[0].speaker, "庄眉");
        assert_eq!(v[0].secret_label, "林念身世");
        assert_eq!(v[0].hit_keyword, "林念是你亲女儿");
    }

    #[test]
    fn test_no_secret_mentioned_no_violation() {
        let s = secrets();
        let v = epistemic_leaks("庄眉", "今天天气不错，我们去吃饭吧。", &s);
        assert!(v.is_empty());
    }

    #[test]
    fn test_empty_speaker_skips() {
        let s = secrets();
        assert!(epistemic_leaks("", "林念是你亲女儿", &s).is_empty());
    }

    #[test]
    fn test_knower_alias_substring() {
        let sb = SecretBoundary::new("x", vec!["秘密词"], vec!["林晚"]);
        // 「林晚清」包含「林晚」→ 视为知情
        assert!(sb.knows("林晚清"));
        // 全名包含写法
        assert!(sb.knows("林晚"));
        // 无关角色不知情
        assert!(!sb.knows("王麻子"));
    }

    #[test]
    fn test_normalize_basic_first_person() {
        let out = normalize_player_referents("我今天买了一套蕾丝内衣。", "向明初");
        assert!(out.contains("向明初"), "应锚定玩家名: {}", out);
        assert!(!out.starts_with("我"), "句首我应被替换: {}", out);
    }

    #[test]
    fn test_normalize_woziji_longest_match() {
        let out = normalize_player_referents("这是我自己买的。", "向明初");
        assert_eq!(out, "这是向明初买的。", "我自己应整体替换: {}", out);
    }

    #[test]
    fn test_normalize_skip_compound_words() {
        // 「我国/自我」成词不应被替换
        let out = normalize_player_referents("我觉得我国的文化和自我修养很重要。", "向明初");
        assert!(out.contains("我国"), "我国不应被拆: {}", out);
        assert!(out.contains("自我"), "自我不应被拆: {}", out);
        // 但独立的「我觉得」的「我」应替换
        assert!(out.starts_with("向明初"), "句首独立我应替换: {}", out);
    }

    #[test]
    fn test_normalize_we_becomes_group() {
        let out = normalize_player_referents("我们一起去。", "向明初");
        assert!(out.contains("向明初等人"), "我们应保守转为群体: {}", out);
    }

    #[test]
    fn test_normalize_empty_name_unchanged() {
        let text = "我自己去。";
        assert_eq!(normalize_player_referents(text, ""), text);
    }

    #[test]
    fn test_ziji_not_touched() {
        // 「自己」不在本函数替换范围（交给前缀标注），确保不被误伤
        let out = normalize_player_referents("她穿着自己买的蕾丝。", "向明初");
        assert!(out.contains("自己"), "自己不应被本函数替换: {}", out);
    }

    #[test]
    fn test_player_view_prefix_contains_name() {
        let p = player_view_prefix("向明初");
        assert!(p.contains("向明初"));
        assert!(p.contains("第一人称"));
        // 空名也应有兜底前缀
        assert!(player_view_prefix("").contains("玩家"));
    }

    #[test]
    fn test_annotate_recall_user_entry_prefixed() {
        // 玩家归档条目（m-user-*）加视角锚定前缀
        let out = annotate_recall_entry("m-user-ab12cd34ef", "她还穿着自己买的蕾丝。", "向明初");
        assert!(out.contains("第一人称"), "玩家条目应带视角前缀: {}", out);
        assert!(out.contains("向明初"));
        assert!(out.contains("自己买的蕾丝"), "原文必须保留: {}", out);
    }

    #[test]
    fn test_annotate_recall_assistant_entry_untouched() {
        // 助手/叙事条目（m-assistant-*）不加前缀
        let src = "庄眉端起茶杯，笑了笑。";
        let out = annotate_recall_entry("m-assistant-ff99ee88", src, "向明初");
        assert_eq!(out, src, "助手条目应原样返回: {}", out);
        assert!(!out.contains("第一人称"));
    }

    #[test]
    fn test_epistemic_system_prompt_has_discipline() {
        let s = epistemic_review_system();
        assert!(s.contains("逐字") || s.contains("引用"), "应要求引证: {}", s);
        assert!(s.contains("无信息边界问题"), "应有空清单出口: {}", s);
        assert!(s.contains("存疑从宽"), "应有文学豁免: {}", s);
    }

    #[test]
    fn test_build_epistemic_review_user_contains_both() {
        let u = build_epistemic_review_user("【系统约束片段】", "庄眉知道林念是亲女儿。");
        assert!(u.contains("【系统约束片段】"));
        assert!(u.contains("庄眉知道林念是亲女儿"));
    }

    #[test]
    fn test_extract_issues_none_when_clean() {
        // judge 判无问题 → None
        assert_eq!(extract_epistemic_issues("无信息边界问题"), None);
        assert_eq!(extract_epistemic_issues("  无信息边界问题。 "), None);
        assert_eq!(extract_epistemic_issues(""), None);
        assert_eq!(extract_epistemic_issues("   "), None);
    }

    #[test]
    fn test_extract_issues_some_when_leak() {
        // judge 报越界 → Some(清单)
        let out = extract_epistemic_issues(
            "[信息边界] 角色「庄眉」：庄眉不应知道林念身世（引用：「林念是你亲女儿」）；修改建议：删去该句。",
        );
        assert!(out.is_some(), "有越界问题应返回 Some");
        assert!(out.unwrap().contains("庄眉"));
    }

    #[test]
    fn test_extract_issues_declares_none_but_lists_marker() {
        // 模型先说"无问题"但后面又列了越界（矛盾输出）→ 以问题标记为准，返回 Some
        let out = extract_epistemic_issues("无信息边界问题。\n[信息边界] 角色「X」：泄露秘密（引用：「…」）");
        assert!(out.is_some(), "含问题标记即使前缀说无也应 Some");
    }

    // ── dramatica 吸收：KnownInfoRecord 信息边界账本 ────────────────────────

    #[test]
    fn info_ledger_learn_dedupe_and_knows() {
        let mut l = InfoLedger::new();
        let rec = KnownInfoRecord::new("向明初", "林念身世", "林念是亲女儿", 3, InfoSource::Witnessed);
        assert!(l.learn(rec.clone()));
        assert!(l.knows("向明初", "林念身世"));
        // 重复 learn 忽略
        assert!(!l.learn(rec));
        assert_eq!(l.records.len(), 1);
        // 他人不知
        assert!(!l.knows("庄眉", "林念身世"));
    }

    #[test]
    fn info_ledger_known_before_chapter() {
        let mut l = InfoLedger::new();
        l.learn(KnownInfoRecord::new("甲", "k1", "第一章得知", 1, InfoSource::Witnessed));
        l.learn(KnownInfoRecord::new("甲", "k2", "第三章得知", 3, InfoSource::Hearsay));
        l.learn(KnownInfoRecord::new("乙", "k3", "别人的", 2, InfoSource::Document));
        let before2 = l.known_before_chapter("甲", 2);
        assert_eq!(before2.len(), 1, "第 2 章前甲只知道 k1: {:?}", before2);
        assert_eq!(before2[0].info_key, "k1");
        let before4 = l.known_before_chapter("甲", 4);
        assert_eq!(before4.len(), 2);
        // 乙的账不混入
        assert_eq!(l.known_before_chapter("乙", 4).len(), 1);
    }

    #[test]
    fn info_ledger_unknown_key_for_boundary_check() {
        let mut l = InfoLedger::new();
        l.learn(KnownInfoRecord::new("主角", "secret_identity", "真实身份", 1, InfoSource::Deduced));
        // 主角知道 → None（不越界）
        assert_eq!(l.unknown_key("主角", "secret_identity"), None);
        // 配角不知道 → Some（越界）
        assert_eq!(l.unknown_key("配角", "secret_identity"), Some("secret_identity"));
    }

    #[test]
    fn info_source_serialization() {
        assert_eq!(InfoSource::Witnessed.as_str(), "witnessed");
        assert_eq!(InfoSource::Hearsay.as_str(), "hearsay");
        assert_eq!(InfoSource::Deduced.as_str(), "deduced");
        assert_eq!(InfoSource::Document.as_str(), "document");
        let rec = KnownInfoRecord::new("甲", "k", "v", 1, InfoSource::Witnessed);
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("witnessed"), "source 应序列化为 witnessed: {json}");
    }
}
