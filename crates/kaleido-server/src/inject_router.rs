//! 注入 router（动态提示词注入 L1 层，2026-09-10；M1 2026-10-07 自 museai-server 移植，
//! env 前缀 MUSEAI_→KALEIDO_，其余零改动。）
//!
//! 方案见 `docs/动态提示词注入-pi前置分析-20260910.md`。
//! 三层漏斗：L0 规则（已有 C5/C6）→ **L1 语义 router（本模块）** → L2 pi agent（按需升级）。
//!
//! 职责：一次小模型 JSON 调用，决定本回合「注什么」——内容分级档位、需要哪些角色、
//! 记忆召回预算、可选块开关。**只做减注/选注，不做内容生成**。
//!
//! 三条铁律：
//! 1. **失败回退全量**（= 改造前现状），零回归；
//! 2. **L0 夹逼**：router 不能召 pack 外角色、tier 不能超过会话上限；
//! 3. **影子模式**（`KALEIDO_INJECT_SHADOW=1`）只记不生效，先收数据再开闸。
//!
//! 开关：
//! - `KALEIDO_INJECT_ROUTER`：默认 `0`（关，= 现状全量）。置 `1` 启用生效。
//! - `KALEIDO_INJECT_SHADOW`：置 `1` 时 router 照跑照记但**不生效**（灰度收数据）。
//! - `KALEIDO_INJECT_ROUTER_MODEL`：router 用的模型（默认走 aux LLM，缺省回退主模型）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 注入决策（router 输出 + L0 夹逼后的最终 plan）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectPlan {
    /// 本轮意图（自由文本，仅记账/可观测用，不参与逻辑）。
    #[serde(default)]
    pub intent: String,
    /// 本轮内容分级档位（safe/standard/open）。
    #[serde(default)]
    pub tier: Option<ContentTierLite>,
    /// 本轮建议注入的角色 id（语义召回，L0 白名单夹逼后）。
    #[serde(default)]
    pub characters: Vec<String>,
    /// 记忆召回档位。
    #[serde(default)]
    pub recall: RecallBudget,
    /// 可选块开关（block id → 是否注入）。未知 id 丢弃。
    #[serde(default)]
    pub blocks: std::collections::BTreeMap<String, bool>,
    /// router 给出的理由（记账用）。
    #[serde(default)]
    pub reason: String,
    /// 本次 plan 的来源：`router` | `fallback_full`（失败回退全量）| `l0_only`（router 关闭）
    #[serde(default)]
    pub source: String,
    /// 失败原因（仅 fallback 时有值，失败可见性）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 内容分级（与 core ContentTier 同构；router JSON 反序列化用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentTierLite {
    Safe,
    Standard,
    Open,
}

impl ContentTierLite {
    fn rank(self) -> u8 {
        match self {
            Self::Safe => 0,
            Self::Standard => 1,
            Self::Open => 2,
        }
    }
    fn from_rank(r: u8) -> Self {
        match r {
            0 => Self::Safe,
            1 => Self::Standard,
            _ => Self::Open,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Standard => "standard",
            Self::Open => "open",
        }
    }
}

/// 记忆召回预算（档位而非逐条——逐条挑选不稳定且难验收）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecallBudget {
    /// L2 近期事件条数预算（0 = 不注）。
    #[serde(default = "default_l2")]
    pub memory_l2: usize,
    /// L3 细粒度记忆条数预算（0 = 不注）。
    #[serde(default = "default_l3")]
    pub memory_l3: usize,
    /// 是否注入世界线。
    #[serde(default = "default_true")]
    pub worldline: bool,
    /// 是否注入未完结伏笔/承诺索引。
    #[serde(default = "default_true")]
    pub hooks: bool,
    /// 是否注入图记忆上下文。
    #[serde(default = "default_true")]
    pub graph: bool,
}

fn default_l2() -> usize {
    12
}
fn default_l3() -> usize {
    3
}
fn default_true() -> bool {
    true
}

impl Default for RecallBudget {
    fn default() -> Self {
        Self {
            memory_l2: default_l2(),
            memory_l3: default_l3(),
            worldline: true,
            hooks: true,
            graph: true,
        }
    }
}

impl Default for InjectPlan {
    /// 全量 plan = 改造前现状（回退即用这个）。
    fn default() -> Self {
        Self {
            intent: String::new(),
            tier: None,
            characters: Vec::new(),
            recall: RecallBudget::default(),
            blocks: std::collections::BTreeMap::new(),
            reason: String::new(),
            source: "fallback_full".into(),
            error: None,
        }
    }
}

impl InjectPlan {
    /// L0 全量 plan（router 关闭时的现状等价物）。
    pub fn full() -> Self {
        Self {
            source: "l0_only".into(),
            ..Default::default()
        }
    }

    /// router 是否启用（生效）。`KALEIDO_INJECT_ROUTER=1` 且非影子模式。
    pub fn enabled() -> bool {
        let on = std::env::var("KALEIDO_INJECT_ROUTER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let shadow = shadow_mode();
        on && !shadow
    }

    /// 影子模式：router 跑但不生效（只记账）。
    pub fn shadow() -> bool {
        shadow_mode()
    }

    /// 该块是否应注入。默认 true（缺省 = 注入，保守）。
    pub fn block_on(&self, id: &str) -> bool {
        self.blocks.get(id).copied().unwrap_or(true)
    }

    /// 本回合实际生效的 tier（已夹逼；None = 沿用会话档）。
    pub fn effective_tier(&self) -> Option<ContentTierLite> {
        self.tier
    }
}

fn shadow_mode() -> bool {
    std::env::var("KALEIDO_INJECT_SHADOW")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 从 LLM 原始回复中抽出 JSON 对象（容忍 ```json 围栏与前后废话）。
pub fn extract_json_object(raw: &str) -> Option<Value> {
    let t = raw.trim();
    // 去掉 markdown 围栏
    let t = if let Some(rest) = t.strip_prefix("```") {
        let rest = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        rest.trim_start_matches(['\n', '\r']).trim()
    } else {
        t
    };
    let t = t.strip_suffix("```").unwrap_or(t).trim();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        if v.is_object() {
            return Some(v);
        }
    }
    // 退一步：找第一个 { 到最后一个 }
    let start = t.find('{')?;
    let end = t.rfind('}')?;
    if end <= start {
        return None;
    }
    let slice = &t[start..=end];
    serde_json::from_str::<Value>(slice)
        .ok()
        .filter(|v| v.is_object())
}

/// 解析 router 原始输出 → plan（**不含** L0 夹逼，夹逼见 `clamp_plan`）。
/// 缺字段 → 用默认（= 全量）；非法 → Err。
pub fn parse_plan(raw: &str) -> Result<InjectPlan, String> {
    let v = extract_json_object(raw).ok_or_else(|| "no JSON object in router output".to_string())?;
    let mut plan: InjectPlan = serde_json::from_value(v)
        .map_err(|e| format!("router plan schema mismatch: {e}"))?;
    plan.source = "router".into();
    plan.error = None;
    Ok(plan)
}

/// L0 夹逼：router 有决策权、无越权空间。
///
/// - `characters` 过滤为 `allowed_char_ids` 内（pack 真实角色）；
/// - `tier` 不得超过 `session_ceiling`（会话声明上限）——**只允许降，不允许升**；
/// - `blocks` 只保留 `known_blocks` 内的 id；
/// - `recall` 预算 clamp 到 `[0, max]`；
/// - **Core 块强制置 true**（不可被 router 关闭，保契约）。
pub fn clamp_plan(
    mut plan: InjectPlan,
    allowed_char_ids: &[String],
    session_ceiling: ContentTierLite,
    known_blocks: &[&str],
    core_blocks: &[&str],
) -> InjectPlan {
    // 角色白名单
    plan.characters
        .retain(|c| allowed_char_ids.iter().any(|a| a == c));
    plan.characters.sort();
    plan.characters.dedup();

    // 分级只降不升
    if let Some(t) = plan.tier {
        let clamped = t.rank().min(session_ceiling.rank());
        plan.tier = Some(ContentTierLite::from_rank(clamped));
    }

    // 块白名单 + Core 强制开
    let known: std::collections::BTreeSet<&str> = known_blocks.iter().copied().collect();
    plan.blocks.retain(|k, _| known.contains(k.as_str()));
    for c in core_blocks {
        plan.blocks.insert((*c).to_string(), true);
    }

    // 召回预算 clamp
    plan.recall.memory_l2 = plan.recall.memory_l2.min(20);
    plan.recall.memory_l3 = plan.recall.memory_l3.min(8);

    plan
}

/// 构造 router 的 system prompt（要求严格 JSON、白名单、只降不升）。
pub fn build_router_system_prompt(known_blocks: &[&str], session_ceiling: &str) -> String {
    format!(
        r#"你是叙事注入调度器。只输出一个 JSON 对象，不要任何解释或 markdown 围栏。

任务：读用户本回合的输入（+少量上下文），判断本回合该给叙事模型注入哪些提示词。

输出 schema：
{{
  "intent": "日常闲聊|情感推进|冲突爆发|成人向|战斗|探索|元指令",
  "tier": "safe|standard|open",
  "characters": ["角色id", ...],
  "recall": {{"memoryL2": 8, "memoryL3": 3, "worldline": true, "hooks": true, "graph": true}},
  "blocks": {{"<块id>": true|false, ...}},
  "reason": "一句话理由"
}}

规则：
1. tier 表示**本回合实际需要**的内容尺度，不是会话上限。写温馨日常就填 "standard"，
   明确进入亲密/暴力场景才填 "open"。**绝不高于会话上限 {session_ceiling}**（超了会被压回）。
2. characters 只填**本回合确实涉及**的角色 id。宁少勿多——没出现、没被提及的角色不要填。
3. recall 是预算档位：memoryL2 ∈ [0,20]，memoryL3 ∈ [0,8]；worldline/hooks/graph 是布尔。
   日常闲聊可以调低，剧情推进/回忆章调高。
4. blocks 只允许出现这些 id（其余忽略）：{blocks}
   只填你要**关闭**或**开启**的；未提及的块按默认（注入）。不确定就别填。
5. 拿不准时**倾向全量**（保守）。你这个判断是优化，不是关卡。

只输出 JSON。"#,
        session_ceiling = session_ceiling,
        blocks = known_blocks.join(", ")
    )
}

/// 构造 router 的 user prompt（本回合输入 + 精简上下文）。
pub fn build_router_user_prompt(
    user_input: &str,
    chapter: &str,
    present_names: &[String],
    recent: &[String],
) -> String {
    let mut s = String::new();
    s.push_str(&format!("当前章节：{}\n", chapter));
    s.push_str(&format!(
        "在场角色：{}\n",
        if present_names.is_empty() {
            "(无)".to_string()
        } else {
            present_names.join("、")
        }
    ));
    if !recent.is_empty() {
        s.push_str("\n最近对话（从旧到新）：\n");
        for r in recent {
            let t: String = r.chars().take(200).collect();
            s.push_str(&format!("- {}\n", t));
        }
    }
    s.push_str(&format!("\n本回合用户输入：\n{}", user_input));
    s
}

/// [精简router 2026-09-10] mini system prompt：只判 intent+tier，不带全量blocks schema。
/// 实测完整版287 prompt token→thinking推理1591-1911 completion（17-30s）；
/// mini版76 prompt→383 completion（muse-spark 3.6s / ling 2.2s），快5-10倍。
/// 输出缺字段→parse走默认全量（已有测试覆盖），只做减注方向判断。
pub fn build_router_mini_system_prompt(session_ceiling: &str) -> String {
    format!(
        r#"你是叙事注入调度器。只输出一个 JSON 对象，不要任何解释或 markdown 围栏。

任务：读用户本回合输入，判断意图与内容尺度。

输出 schema：
{{"intent": "日常闲聊|情感推进|冲突爆发|成人向|战斗|探索|元指令", "tier": "safe|standard|open"}}

规则：
1. tier 表示本回合实际需要的内容尺度，不是会话上限。日常填 "standard"，
   明确亲密/暴力场景才填 "open"。绝不高于会话上限 {session_ceiling}。
2. 只输出 JSON。"#,
        session_ceiling = session_ceiling,
    )
}

/// [精简router 2026-09-10] mini user prompt：只带本回合输入，不带章节/角色/历史。
pub fn build_router_mini_user_prompt(user_input: &str) -> String {
    format!("用户本回合：{}", user_input)
}

/// mini模式开关：`KALEIDO_INJECT_ROUTER_MINI=0` 关（走完整prompt），默认开。
pub fn mini_mode() -> bool {
    std::env::var("KALEIDO_INJECT_ROUTER_MINI")
        .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false")))
        .unwrap_or(true)
}

/// 已注册的可由 router 控制的块 id 清单（与 build_tavern_system_prompt 的块对应）。
/// Core 块不在此列（不可关）。
pub const ROUTER_BLOCKS: &[&str] = &[
    // Conditional（可关，省 token/注意力）
    "style_preset",      // 风格预设（含破限指令）
    "content_tier_extra", // 内容分级扩展边界（敏感度判定）
    "writing_style",     // 文风要求
    "writing_craft",     // 文笔要求（复刻原作）
    "director_plan",     // 导演计划
    "event_card",        // 本回合事件卡
    "emotion_trend",     // 情绪趋势
    "personality_traits", // 人格特征
    "lore",              // 世界书 / Lore
    "worldline",         // （世界线，经 recall.worldline 生效）
    "asset_blocks",      // 资产块（情感曲线/角色弧/关系演化）
    "ledger",            // 记忆账本（kaleido 独立块）
    "outline_clip",      // 原著剖析
    "graph_memory",      // 图记忆（经 recall.graph 生效）
];

/// Core 块（router 不可关闭，保契约）。
pub const CORE_BLOCKS: &[&str] = &[
    "story_identity",
    "present_cast",
    "output_rules",
    "continuity_guard",
];

/// 记账 JSON（合并进 job payload；影子模式也记）。
pub fn accounting_json(plan: &InjectPlan, applied: bool, latency_ms: u64) -> Value {
    json!({
        "injectRouter": {
            "applied": applied,
            "source": plan.source,
            "intent": plan.intent,
            "tier": plan.tier.map(|t| t.as_str()),
            "characters": plan.characters,
            "recall": {
                "memoryL2": plan.recall.memory_l2,
                "memoryL3": plan.recall.memory_l3,
                "worldline": plan.recall.worldline,
                "hooks": plan.recall.hooks,
                "graph": plan.recall.graph,
            },
            "blocks": plan.blocks,
            "reason": plan.reason,
            "latencyMs": latency_ms,
            "error": plan.error,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<String> {
        vec!["cc-a".into(), "cc-b".into(), "cc-c".into()]
    }
    const KNOWN: &[&str] = &["style_preset", "lore", "event_card"];

    #[test]
    fn extract_json_tolerates_fence_and_prose() {
        assert!(extract_json_object("{\"a\":1}").is_some());
        assert!(extract_json_object("```json\n{\"a\":1}\n```").is_some());
        assert!(extract_json_object("好的，这是结果：\n{\"a\":1}\n以上。").is_some());
        assert!(extract_json_object("no json here").is_none());
        assert!(extract_json_object("[1,2,3]").is_none()); // 非对象
    }

    #[test]
    fn parse_plan_basic() {
        let raw = r#"{"intent":"日常闲聊","tier":"standard","characters":["cc-a","cc-ghost"],
            "recall":{"memoryL2":4,"memoryL3":0,"worldline":false,"hooks":true,"graph":false},
            "blocks":{"style_preset":false},"reason":"温馨日常"}"#;
        let p = parse_plan(raw).unwrap();
        assert_eq!(p.tier, Some(ContentTierLite::Standard));
        assert_eq!(p.characters.len(), 2);
        assert_eq!(p.recall.memory_l2, 4);
        assert!(!p.recall.worldline);
        assert_eq!(p.block_on("style_preset"), false);
        assert_eq!(p.block_on("lore"), true); // 未提及 → 默认注入
        assert_eq!(p.source, "router");
    }

    #[test]
    fn parse_plan_missing_fields_default_full() {
        // 空对象 → 全默认（= 全量），不报错
        let p = parse_plan("{}").unwrap();
        assert_eq!(p.tier, None);
        assert_eq!(p.recall.memory_l2, 12);
        assert!(p.recall.worldline);
        assert!(p.block_on("anything"));
    }

    #[test]
    fn parse_plan_rejects_garbage() {
        assert!(parse_plan("完全不是 JSON").is_err());
    }

    #[test]
    fn clamp_filters_ghost_characters() {
        let raw = r#"{"characters":["cc-a","cc-ghost","cc-b"],"tier":"open",
            "blocks":{"style_preset":false,"evil_block":false}}"#;
        let p = parse_plan(raw).unwrap();
        let c = clamp_plan(p, &ids(), ContentTierLite::Standard, KNOWN, CORE_BLOCKS);
        assert_eq!(c.characters, vec!["cc-a".to_string(), "cc-b".to_string()]);
        assert!(!c.blocks.contains_key("evil_block")); // 未知块丢弃
        assert_eq!(c.blocks.get("style_preset"), Some(&false));
    }

    #[test]
    fn clamp_tier_only_descends() {
        // router 想升到 open，会话上限 standard → 压回 standard
        let p = parse_plan(r#"{"tier":"open"}"#).unwrap();
        let c = clamp_plan(p, &ids(), ContentTierLite::Standard, KNOWN, CORE_BLOCKS);
        assert_eq!(c.tier, Some(ContentTierLite::Standard));

        // router 想降到 safe，上限 open → 允许（只降不升）
        let p2 = parse_plan(r#"{"tier":"safe"}"#).unwrap();
        let c2 = clamp_plan(p2, &ids(), ContentTierLite::Open, KNOWN, CORE_BLOCKS);
        assert_eq!(c2.tier, Some(ContentTierLite::Safe));
    }

    #[test]
    fn clamp_forces_core_blocks_on() {
        // router 试图关闭 Core 块 → 被强制打开
        let p = parse_plan(r#"{"blocks":{"output_rules":false,"story_identity":false}}"#).unwrap();
        let c = clamp_plan(p, &ids(), ContentTierLite::Open, KNOWN, CORE_BLOCKS);
        assert_eq!(c.block_on("output_rules"), true);
        assert_eq!(c.block_on("story_identity"), true);
    }

    #[test]
    fn clamp_recall_budget_capped() {
        let p = parse_plan(r#"{"recall":{"memoryL2":999,"memoryL3":999}}"#).unwrap();
        let c = clamp_plan(p, &ids(), ContentTierLite::Open, KNOWN, CORE_BLOCKS);
        assert_eq!(c.recall.memory_l2, 20);
        assert_eq!(c.recall.memory_l3, 8);
    }

    #[test]
    fn default_plan_is_full_injection() {
        let p = InjectPlan::default();
        assert!(p.block_on("any_block")); // 默认全注
        assert!(p.recall.worldline);
        assert_eq!(p.recall.memory_l2, 12);
        assert_eq!(p.source, "fallback_full");
    }

    #[test]
    fn accounting_shape() {
        let p = parse_plan(r#"{"intent":"战斗","tier":"open","reason":"打起来了"}"#).unwrap();
        let v = accounting_json(&p, true, 320);
        let r = &v["injectRouter"];
        assert_eq!(r["applied"], true);
        assert_eq!(r["tier"], "open");
        assert_eq!(r["intent"], "战斗");
        assert_eq!(r["latencyMs"], 320);
    }
}
