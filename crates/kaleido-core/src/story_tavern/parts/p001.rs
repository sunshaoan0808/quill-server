#![allow(unused_imports)]
use crate::{
    st_compass::{Compass, CompassStore},
    st_skimming::SkimIssue,
    text_hash, CoreError, CoreResult, DataRoot,
};
use crate::world_state::{EntityKind as WsEntityKind, WorldEntity, WorldEvent, WorldState};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};
use uuid::Uuid;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContentTier {
    Safe,
    Standard,
    Open,
}

impl ContentTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Standard => "standard",
            Self::Open => "open",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::Safe => 0,
            Self::Standard => 1,
            Self::Open => 2,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "safe" => Some(Self::Safe),
            "standard" => Some(Self::Standard),
            "open" => Some(Self::Open),
            _ => None,
        }
    }

    /// contentTierFinal = min(user, card/pack, global)
    pub fn min3(user: Self, pack_or_card: Self, global: Self) -> Self {
        let r = user.rank().min(pack_or_card.rank()).min(global.rank());
        match r {
            0 => Self::Safe,
            1 => Self::Standard,
            _ => Self::Open,
        }
    }
}

impl Default for ContentTier {
    fn default() -> Self {
        Self::Standard
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlayMode {
    Mainline,
    Free,
    /// Side story / 番外：冻结主线 chapterCursor/nodeId，回主线用 resumeNodeId
    Side,
}

impl Default for PlayMode {
    fn default() -> Self {
        Self::Mainline
    }
}

/// P0 (吞噬 denova novel-lite / novel-standard / novel-heavy): 回合叙事生成写作档位。
/// - `lite`：主 Agent 单次直出（现状，零回归）
/// - `standard`：初稿 → 审稿 → 修订
/// - `heavy`：context-plan → write → review → fix → final-gate 全管道（上限 2 轮修复）
/// 会话级持久化；`TurnStartRequest.quality` 可 per-turn 覆盖并回写会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Quality {
    Lite,
    Standard,
    Heavy,
}

impl Default for Quality {
    fn default() -> Self {
        Self::Lite
    }
}

impl Quality {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lite => "lite",
            Self::Standard => "standard",
            Self::Heavy => "heavy",
        }
    }
}

/// 宽松反序列化：缺省/非法值一律回落 lite（旧请求体与未知档位兼容）。
impl<'de> Deserialize<'de> for Quality {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "standard" => Self::Standard,
            "heavy" => Self::Heavy,
            _ => Self::Lite,
        })
    }
}

impl PlayMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mainline => "mainline",
            Self::Free => "free",
            Self::Side => "side",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mainline" => Some(Self::Mainline),
            "free" => Some(Self::Free),
            "side" => Some(Self::Side),
            _ => None,
        }
    }

    /// Modes that must not advance mainline node/chapter cursors.
    pub fn freezes_mainline_cursor(self) -> bool {
        matches!(self, Self::Free | Self::Side)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Playable {
    #[serde(rename = "P1")]
    P1,
    #[serde(rename = "P2")]
    P2,
    #[serde(rename = "P3")]
    P3,
    #[serde(rename = "P4")]
    P4,
}

impl Default for Playable {
    fn default() -> Self {
        Self::P1
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryRole {
    Supporting,
    Protagonist,
    Isekai,
    Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MetaKnowledge {
    None,
    Reader,
}

impl Default for MetaKnowledge {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RewriteIntensity {
    Canon,
    Rewrite,
}

impl Default for RewriteIntensity {
    fn default() -> Self {
        Self::Canon
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EngineTag {
    Canon,
    Advance,
    Idle,
}

// ─── Pack ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackSource {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(default)]
    pub refs: Vec<String>,
}

impl Default for PackSource {
    fn default() -> Self {
        Self {
            source_type: "demo".into(),
            refs: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeExit {
    pub id: String,
    pub when: String,
    pub next: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryNode {
    pub id: String,
    pub chapter_id: String,
    pub title: String,
    #[serde(default)]
    pub entry: String,
    #[serde(default)]
    pub exit: Vec<NodeExit>,
    #[serde(default)]
    pub locked_beats: Vec<String>,
    /// none | branch | open
    #[serde(default = "default_divergence")]
    pub allowed_divergence: String,
    #[serde(default)]
    pub present_characters: Vec<String>,
    #[serde(default)]
    pub location_id: Option<String>,
    /// Short excerpt for prompt (from chapter md)
    #[serde(default)]
    pub summary: String,
}

pub(crate) fn default_divergence() -> String {
    "branch".into()
}

/// Key node candidate for side-branch picker (整本总结 → 重要节点).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideBranchNode {
    pub id: String,
    pub chapter_id: String,
    pub chapter_title: String,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub entry: String,
    #[serde(default)]
    pub present_characters: Vec<String>,
    /// Why this node is offered as a side branch.
    #[serde(default)]
    pub reason: String,
    pub order: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideBranchCatalog {
    pub pack_id: String,
    pub pack_title: String,
    /// Whole-novel blurb distilled from chapters/nodes/lore.
    pub novel_summary: String,
    pub nodes: Vec<SideBranchNode>,
    #[serde(default)]
    pub resume_node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryChapter {
    pub id: String,
    pub title: String,
    pub order: u32,
    #[serde(default)]
    pub goals: Vec<String>,
    #[serde(default)]
    pub node_ids: Vec<String>,
    /// Relative path under pack dir, e.g. chapters/ch01.md
    #[serde(default)]
    pub body_path: String,
    /// U10: 章节插图——工作区相对路径（images/illustrations/...），无插图时为空串。
    /// 前端经 GET /api/v1/works/image-data-url?path= 读取展示。serde default 兼容旧 pack.json。
    #[serde(default)]
    pub image_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackCharacterRef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub role: String,
    /// 性别(证据可推则写,否则"未知")。
    #[serde(default)]
    pub gender: String,
    /// 外貌/形象(证据可推则写,否则"未知")。
    #[serde(default)]
    pub appearance: String,
    /// 开场场景一句话(地点/时间/在场人,蒸馏产出)。
    #[serde(default)]
    pub opening_scene: String,
    /// 开场白 1-2 句(蒸馏产出,建立情境+可行动位置+约束)。
    #[serde(default)]
    pub opening_lines: String,
    /// 敏感度判定边界（蒸馏产出,格式"敏感点→判定边界",用于 Open/Standard 档演出护栏）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nsfw_profile: String,
    /// 角色谱 importance(high/medium/low),由蒸馏环节写入,供主角断言/过滤使用。
    #[serde(default)]
    pub importance: String,
    #[serde(default)]
    pub content_tier: Option<ContentTier>,
    #[serde(default)]
    pub example_dialogs: Vec<String>,
    #[serde(default)]
    pub boundaries: Vec<String>,
    #[serde(default)]
    pub personality: String,
    /// [P8 D1 2026-08-16] 声线特征（蒸馏 M2 产出）：角色默认语气一句话 + 代表金句支撑。
    /// 修复宿醉声线漂移源头——personality 概括化丢泼辣面，声线单独成字段防止被稀释。
    #[serde(default)]
    pub voice_profile: String,
    #[serde(default)]
    pub speech_style: String,
    /// 深层动机/目标（女娲"心智模型"提炼，带证据）。
    #[serde(default)]
    pub motivation: String,
    /// 与该角色相关的人与关系简述，格式 "角色名:关系描述"，带证据。
    #[serde(default)]
    pub relationships: Vec<String>,
    /// 每条结论的证据出处，格式 "ch12:block3" 等。
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    /// 女娲"核心心智模型"：角色看世界的镜片，每条 "模型名：一句话描述（证据ch）"。
    #[serde(default)]
    pub mental_models: Vec<String>,
    /// 女娲"决策启发式"：角色做判断的直觉规则，每条 "规则名：触发场景→做法（证据ch）"。
    #[serde(default)]
    pub decision_heuristics: Vec<String>,
    /// 仓颉"信念形成故事"：角色相信什么、为什么信，每条 "信念：形成故事（证据ch）"。
    #[serde(default)]
    pub beliefs: Vec<String>,
    /// P2-1 立绘层：情绪名 → 表情图片 URL。情绪名对齐 P2-1 枚举（平静/开心/愤怒/悲伤/害羞/惊讶/恐惧/厌恶/疲惫/心动）。serde default 兼容旧 pack 无此字段。
    #[serde(default)]
    pub expressions: std::collections::HashMap<String, String>,
    /// P2-1 立绘层：默认立绘（无情绪匹配时回退）。serde default 兼容旧 pack。
    #[serde(default)]
    pub avatar: Option<String>,
    /// P3 语音层：edge-tts 音色名（如 zh-CN-YunxiNeural）。可选；未指定时前端按角色名 hash 从默认音色池稳定选。serde default/skip 兼容旧 pack + 列表（P2-1 教训：列表端点必须透传）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// SoulLink 吸收：对话驱动的增量档案（标量字段 + personality/worldview/family/relationships/memory 分节）。
    /// AI 每轮生成后自动维护；前端可查看/手动分析/精编。serde default 兼容旧 pack。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<crate::character_archive::CharacterArchive>,
    /// [吞噬 Front Porch AI pockets.dart] 初始口袋与衣物（蒸馏/卡面播种）。
    /// worn = 身上穿的，carrying = 身上带的，均为 display 字符串（如 "jacket (rain-soaked)"）。
    /// 旧 pack 无此字段 → 空口袋，不影响既有会话。
    #[serde(default)]
    pub starting_wardrobe: crate::pockets::Pockets,
}

#[cfg(test)]
mod sprite_ref_tests {
    use super::PackCharacterRef;

    /// P2-1 立绘层：pack roundtrip 带 expressions/avatar 不丢，旧数据（无这两字段）也不爆。
    #[test]
    fn pack_character_ref_expressions_roundtrip() {
        let mut ch = PackCharacterRef {
            id: "c1".into(),
            name: "母亲".into(),
            role: String::new(),
            gender: String::new(),
            appearance: String::new(),
            opening_scene: String::new(),
            opening_lines: String::new(),
            nsfw_profile: String::new(),
            importance: "high".into(),
            content_tier: None,
            example_dialogs: Vec::new(),
            boundaries: Vec::new(),
            personality: String::new(),
            speech_style: String::new(),
            voice_profile: String::new(),
            motivation: String::new(),
            relationships: Vec::new(),
            evidence_refs: Vec::new(),
            mental_models: Vec::new(),
            decision_heuristics: Vec::new(),
            beliefs: Vec::new(),
            expressions: std::collections::HashMap::new(),
            voice: None,
            archive: None,
            avatar: None,
                starting_wardrobe: Default::default(),
        };
        ch.expressions
            .insert("开心".into(), "https://img/x/1.png".into());
        ch.avatar = Some("https://img/x/avatar.png".into());
        ch.nsfw_profile = "接吻以上→nsfw；日常对白→non".into();
        let json = serde_json::to_value(&ch).unwrap();
        let back: PackCharacterRef = serde_json::from_value(json).unwrap();
        assert_eq!(back.expressions.get("开心").map(|s| s.as_str()), Some("https://img/x/1.png"));
        assert_eq!(back.avatar.as_deref(), Some("https://img/x/avatar.png"));
        assert_eq!(back.nsfw_profile, "接吻以上→nsfw；日常对白→non");
    }

    /// 旧 pack 数据无 expressions/avatar 字段——默认值不爆。
    #[test]
    fn pack_character_ref_old_data_defaults() {
        let ch: PackCharacterRef = serde_json::from_value(serde_json::json!({
            "id": "c1", "name": "母亲"
        }))
        .unwrap();
        assert!(ch.expressions.is_empty());
        assert!(ch.avatar.is_none());
        assert!(ch.nsfw_profile.is_empty());
    }
}

// ─── 演出机 S1：导演台（Stage Director）与 Actor 状态（纯数据结构骨架）────────

/// S5: 事件卡包（吞噬 denova event_package）。一包一组同类 TellerEventCard，
/// 由 pack.stage_director.modules.eventPackageIds 指定启用哪些包参与每回合抽取。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EventPackage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cards: Vec<TellerEventCard>,
}

/// S5: 事件卡。每回合由 teller 按权重抽一张，注入 ST-27 引导 LLM 演出。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TellerEventCard {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: String,
    /// 注入给 LLM 的演出提示（与 denova event_card.prompt 对齐）。
    #[serde(default)]
    pub prompt: String,
    /// 抽取权重（0 = 不参与；默认 1）。
    #[serde(default = "default_event_weight")]
    pub weight: u32,
    /// 卡片是否启用。
    #[serde(default)]
    pub enabled: bool,
    /// 每会话只抽一次（抽过后 usedInSession 置 true，后续回合排除）。
    #[serde(default)]
    pub once_per_session: bool,
    /// 本会话已抽过（由 pick_event_card 维护；会话开始时应为 false）。
    #[serde(default)]
    pub used_in_session: bool,
    /// 事件类型名（用户可见，如「外门考核打脸」）；缺省空 = 不展示类型标签
    #[serde(default)]
    pub type_name: String,
    /// 类别（如 打脸/奇遇/秘境/恋爱/冲突…）；空 = 未分类
    #[serde(default)]
    pub category: String,
    /// 短标签（可检索）
    #[serde(default)]
    pub tags: Vec<String>,
    /// 强度档（low/medium/high）；空 = 不展示
    #[serde(default)]
    pub intensity: String,
    /// 冷却回合数（非负，默认 0 = 无冷却）：同一张卡抽过后 N 回合内不再抽
    #[serde(default)]
    pub cooldown_turns: u32,
    /// 所属章节范围（如 ["ch01","ch03"]，蒸馏产出，基于切分 chXX 编号与运行时 chapter_cursor 对齐）。
    /// 空 = 旧数据/未标注 → 任意章节可抽（兼容 A3）。
    #[serde(default)]
    pub chapter_range: Vec<String>,
}

pub(crate) fn default_event_weight() -> u32 {
    1
}

/// S5: 每回合事件卡抽取纯函数（吸收 denova event_package teller 抽样逻辑）。
/// - 候选：pack.eventPackages 中 enabled 的包；若 modules.eventPackageIds 非空则只取其中列出的包。
/// - 卡必须 enabled、weight>0；oncePerSession 且 usedInSession 的卡排除。
/// - 冷却：card.cooldown_turns>0 且 last_event 命中同一张卡、且距上次抽取 < 冷却回合 → 排除。
///   last_event=None 视为无冷却限制（兼容旧调用）；next_turn 为本次抽取所在回合（与 last_event.turn 做差）。
/// - A2 按章过滤：current_chapter 传入当前 chapter_cursor（如 "ch03"）时，
///   只抽 chapter_range 覆盖当前章的卡；chapter_range 为空（旧数据/未标注）→ 不参与章过滤（A3 兼容）；
///   有 range 但当前章不命中 → 该卡排除；全部排除 → 返回 None（本回合不抽事件卡）。
/// - 按 weight 加权抽样；seed 由调用方提供（server 可用 turn/时间戳），无 rand 依赖。
pub fn pick_event_card<'a>(
    pack: &'a StoryPack,
    seed: u64,
    next_turn: u32,
    last_event: Option<&EventLogEntry>,
    current_chapter: Option<&str>,
) -> Option<(&'a EventPackage, &'a TellerEventCard)> {
    let mut candidates: Vec<(&'a EventPackage, &'a TellerEventCard)> = Vec::new();
    let allowed = &pack.stage_director.modules.event_package_ids;
    for pkg in &pack.event_packages {
        if !pkg.enabled {
            continue;
        }
        if !allowed.is_empty() && !allowed.contains(&pkg.id) {
            continue;
        }
        for card in &pkg.cards {
            if !card.enabled || card.weight == 0 {
                continue;
            }
            if card.once_per_session && card.used_in_session {
                continue;
            }
            // A2 按章过滤：card 标注了 chapter_range 且当前章非空时，只抽覆盖当前章的卡。
            // 未标注（空 range）或当前章未知（None）→ 不拦截（旧行为）。
            if !card.chapter_range.is_empty() {
                if let Some(cur) = current_chapter {
                    if !card.chapter_range.iter().any(|r| r == cur) {
                        continue;
                    }
                }
            }
            // 冷却：同一张卡抽过后 cooldown_turns 内不再抽（基于 last_event.turn 与 next_turn 的回合差）
            if card.cooldown_turns > 0 {
                if let Some(ev) = last_event {
                    if ev.card_id == card.id && next_turn > ev.turn && next_turn - ev.turn < card.cooldown_turns
                    {
                        continue;
                    }
                }
            }
            candidates.push((pkg, card));
        }
    }
    if candidates.is_empty() {
        return None;
    }
    let total: u64 = candidates.iter().map(|(_, c)| c.weight as u64).sum();
    if total == 0 {
        return None;
    }
    // xorshift64* 确定性伪随机（不同种子 → 不同分布；可复现测试）
    let mut x = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0xBF58_476D_1CE4_E5B9);
    if x == 0 {
        x = 0x9E37_79B9_7F4A_7C15;
    }
    let mut next = move || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let mut r = next() % total;
    for (pkg, card) in &candidates {
        if r < card.weight as u64 {
            return Some((*pkg, *card));
        }
        r -= card.weight as u64;
    }
    Some(*candidates.last().expect("candidates non-empty"))
}

/// S5: 事件卡抽取记录（挂到 session.lastEvent，供前端展示 + ST-27 注入）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EventLogEntry {
    #[serde(default)]
    pub turn: u32,
    #[serde(default)]
    pub package_id: String,
    #[serde(default)]
    pub card_id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub created_at: String,
    /// 事件类型名（冗余记录卡字段，供本回合注入直接展示；旧数据缺省空 = 不展示类型标签）。
    #[serde(default)]
    pub type_name: String,
    /// 类别（冗余记录卡字段）：本回合注入展示用。
    #[serde(default)]
    pub category: String,
    /// 强度档（冗余记录卡字段，low/medium/high）：本回合注入展示用。
    #[serde(default)]
    pub intensity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StageDirectorConfig {
    #[serde(default)]
    pub run_policy: DirectorRunPolicy,
    #[serde(default)]
    pub modules: DirectorModuleRefs,
    /// P1-2 (吞噬 denova mainline_strength): 主线强度档位。
    /// - strong_arc = 严格遵循原著主线；balanced = 主线优先、允许合理分支（默认）；soft = 宽松探索
    #[serde(default = "default_mainline_strength")]
    pub mainline_strength: String,
    #[serde(default)]
    pub resolved_snapshot: Option<DirectorResolvedSnapshot>,
}

/// P1-2: mainline_strength 默认档位（兼容旧包，缺字段时回退 "balanced"）。
pub(crate) fn default_mainline_strength() -> String {
    "balanced".into()
}

/// S4: 导演计划（吞噬 denova director_plan）。**导演计划是意图，不是自由改写**：
/// 不得改写 locked_beats 硬事实。hits_beats 只允许引用当前 node.locked_beats 原文。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirectorPlan {
    /// 导演目标（一句话）
    pub goal: String,
    /// 当前压力
    #[serde(default)]
    pub pressure: Option<String>,
    /// 达成代价
    #[serde(default)]
    pub cost: Option<String>,
    /// 声明命中的 locked_beats（红线声明，只能引用当前 node.locked_beats 原文）
    #[serde(default)]
    pub hits_beats: Vec<String>,
    #[serde(default)]
    pub created_turn: u32,
    #[serde(default)]
    pub updated_turn: u32,
    /// G1 (吞噬 denova DirectorPlanDocs.plan): 计划正文——承前（当前回合的叙事意图与承接要点）。
    #[serde(default)]
    pub plan: String,
    /// G1 (吞噬 denova DirectorPlanDocs.agent_brief): 本回合指令——给演出 agent 的导演指令。
    #[serde(default)]
    pub agent_brief: String,
    /// G1 (吞噬 denova DirectorPlanDocs.lore_context): 启后铺垫——世界观/设定上下文。
    #[serde(default)]
    pub lore_context: String,
    /// G2 (吞噬 denova DirectorPlanMetadata.last_run): 最近一次执行状态（ready/running/conflict）。
    #[serde(default)]
    pub last_run: Option<DirectorPlanRunStatus>,
}

/// G2 (吞噬 denova DirectorPlanRunStatus): 导演计划执行状态。旧数据无此字段 → None（未启动）。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DirectorPlanRunStatus {
    /// 状态：ready=计划就绪 / running=执行中 / conflict=LLM 失败或冲突
    pub status: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl DirectorPlanRunStatus {
    pub fn ready(summary: impl Into<String>) -> Self {
        Self {
            status: "ready".into(),
            summary: Some(summary.into()),
            error: None,
            updated_at: Some(chrono::Utc::now().to_rfc3339()),
        }
    }
    pub fn running() -> Self {
        Self {
            status: "running".into(),
            summary: None,
            error: None,
            updated_at: Some(chrono::Utc::now().to_rfc3339()),
        }
    }
    pub fn conflict(error: impl Into<String>) -> Self {
        Self {
            status: "conflict".into(),
            summary: None,
            error: Some(error.into()),
            updated_at: Some(chrono::Utc::now().to_rfc3339()),
        }
    }
}

/// S4: 导演计划调度纯函数（吸收 denova director_run_policy.go DecideDirectorRunAfterTurn 的 interval 分支）。
/// - "manual" → false（仅 API 手动触发）
/// - "interval" → interval_turns>0 && turn>=1 && turn % interval_turns == 0 && last_plan_turn != Some(turn)（不重复触发）
/// - "on_demand" → false（由 server 触发条件决定）
/// - 其他 → false
pub fn director_due(
    mode: &str,
    interval_turns: u32,
    turn: u32,
    last_plan_turn: Option<u32>,
) -> bool {
    match mode.trim() {
        "manual" => false,
        "on_demand" => false,
        "interval" => {
            interval_turns > 0
                && turn >= 1
                && turn % interval_turns == 0
                && last_plan_turn != Some(turn)
        }
        _ => false,
    }
}

/// G13/G14 (吞噬 denova workspaceDirectorTaskGroup / GoKeyed): 后台导演任务组。
///
/// 导演 agent 的「回合外单次同步 LLM 调用」会随 HTTP 断线取消；本任务组把导演后台任务
/// 登记为 workspace 级串行任务组：
/// - **同 key 并发串行**：`start(key, task_id, f)` 只在 key 空闲时真正启动（返回 `true`）；
///   同 key 已在跑 → 不启动并返回 `false`（denova 语义：`!started` → 调用方按
///   `context.Canceled` 处理，不排队、不覆盖）。
/// - **任务登记**：`with_task(task_id)` 登记当前执行任务 id（线程级），供后台任务生命周期
///   可见性（GET director-config 的 `directorTask` 字段即由 server 写入 session）。
/// - **panic 捕获**：任务体 panic 被 `catch_unwind` 捕获并打 warn 日志，不炸进程，
///   运行表照常清理，key 可复用。
/// - **异步原语**：`acquire`/`release` 供 server 在 `tokio::spawn` 后台任务里手动
///   登记/释放 key（`start` 即 acquire + 执行 + 释放 的同步封装）。
pub struct DirectorTaskGroup {
    running: Mutex<std::collections::HashMap<String, String>>,
}

impl Default for DirectorTaskGroup {
    fn default() -> Self {
        Self {
            running: Mutex::new(std::collections::HashMap::new()),
        }
    }
}

// 当前线程正在执行的导演任务 key（`start` 执行期间登记，供 `with_task` 无 key 登记语义）。
// [P7] 改为普通注释：doc 注释在 thread_local! 宏调用上不生效（unused_doc_comments）。
thread_local! {
    static DIRECTOR_TASK_KEY: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

impl DirectorTaskGroup {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前 key 是否在跑。
    pub fn is_running(&self, key: &str) -> bool {
        self.running.lock().contains_key(key)
    }

    /// 当前 key 正在跑的任务 id（None = 未在跑）。
    pub fn current_task(&self, key: &str) -> Option<String> {
        self.running.lock().get(key).cloned()
    }

    /// 尝试登记 key（同 key 在跑 → 返回 false 不启动）。
    /// 成功时登记 task_id，并执行 f；f panic 被 `catch_unwind` 捕获（warn 日志），
    /// 运行表照常清理。返回是否真正启动（非执行成败）。
    pub fn start<F>(&self, key: impl Into<String>, task_id: impl Into<String>, f: F) -> bool
    where
        F: FnOnce(),
    {
        let key = key.into();
        let task_id = task_id.into();
        if !self.acquire(key.clone(), task_id.clone()) {
            return false;
        }
        DIRECTOR_TASK_KEY.with(|c| *c.borrow_mut() = Some(key.clone()));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        DIRECTOR_TASK_KEY.with(|c| *c.borrow_mut() = None);
        self.release(&key);
        match result {
            Ok(()) => {}
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown panic payload".into());
                tracing::warn!(
                    key = %key,
                    task_id = %task_id,
                    error = %msg,
                    "director task group caught task panic"
                );
            }
        }
        true
    }

    /// 登记当前执行任务的 task_id（替换/更新当前线程 key 的运行表登记）。
    /// 线程无正在执行的 key 时 no-op（async 后台任务请直接写 session.director_task）。
    pub fn with_task(&self, task_id: impl Into<String>) {
        let task_id = task_id.into();
        let current = DIRECTOR_TASK_KEY.with(|c| c.borrow().clone());
        if let Some(key) = current {
            if let Some(mut running) = self.running.try_lock() {
                if running.contains_key(&key) {
                    running.insert(key, task_id);
                }
            }
        }
    }

    /// 异步场景原语：登记 key（同 key 在跑 → false）。与 `release` 成对使用。
    pub fn acquire(&self, key: impl Into<String>, task_id: impl Into<String>) -> bool {
        let key = key.into();
        let mut running = self.running.lock();
        if running.contains_key(&key) {
            return false;
        }
        running.insert(key, task_id.into());
        true
    }

    /// 异步场景原语：释放 key（不校验归属；成对使用见 [`DirectorTaskGroup::acquire`]）。
    pub fn release(&self, key: &str) {
        self.running.lock().remove(key);
    }
}

/// G3 (吞噬 denova prepareInteractiveDirectorBeforeOpening): 开局导演规划调度纯函数。
/// 会话首个回合（turns == 0）且 opening 已 seed 且尚无任何导演 plan → true（应生成开局三文档：
/// 选角/场景/分支规划，给故事一个导演意图锚点）。
/// 幂等：已有 plan（含 last_run 任意状态）→ false，不重复生成。
pub fn opening_plan_due(turn: u32, has_plan: bool, opening_seeded: bool) -> bool {
    turn == 0 && !has_plan && opening_seeded
}

/// G4 (吞噬 denova fitTextToTokenBudget): 按字符预算拟合文本——**头尾保留、中间省略**。
/// CJK 按 ~1 字符/token 近似（中文 1 token ≈ 1 字符），预算单位统一为「字符」。
/// 超过预算时：头部保留 head_ratio（默认 0.5）份额、尾部保留其余，中间以「\n…（省略 N 字符）…\n」连接。
/// 预算内原样返回（零拷贝语义由调用方决定）。空串/预算为 0 → 空串。
pub fn fit_text_to_token_budget(text: &str, budget: usize, head_ratio: f64) -> String {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    if total == 0 || budget == 0 {
        return String::new();
    }
    if total <= budget {
        return text.to_string();
    }
    let head = ((budget as f64 * head_ratio.clamp(0.0, 1.0)).floor() as usize).min(total);
    let tail_budget = budget.saturating_sub(head);
    let tail = tail_budget.min(total - head);
    let omitted = total - head - tail;
    let mut out = String::with_capacity(budget + 32);
    out.extend(chars.iter().take(head));
    if omitted > 0 {
        out.push_str(&format!("\n…（省略 {omitted} 字符）…\n"));
    }
    out.extend(chars.iter().skip(total - tail));
    out
}

/// G5 (吞噬 denova ContextLedger): 导演上下文账本条目——记录每个注入块的来源/标题/字节/预算/是否进入最终消息。
/// 仅供审计与日志：构建导演 LLM 请求时逐块登记，可观测「长会话导演上下文溢出」与「裁剪行为」。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorLedgerEntry {
    /// 来源块标识（如 "recent" / "node_summary" / "strategy"）。
    #[serde(default)]
    pub source: String,
    /// 人类可读标题（如 "最近剧情"）。
    #[serde(default)]
    pub title: String,
    /// 原始字节数。
    #[serde(default)]
    pub body_bytes: usize,
    /// 预算上限（0 = 无预算）。
    #[serde(default)]
    pub limit: usize,
    /// 是否进入最终消息（false = 被预算裁剪/省略）。
    #[serde(default)]
    pub included: bool,
    /// 备注（裁剪原因、省略字符数等）。
    #[serde(default)]
    pub note: String,
}

/// G15 (吞噬 denova retainedTurnsForInteractiveCompaction): 叙界守卫事件保留策略。
/// 守卫事件字符串格式 `[high|med][维度] 消息`。长会话无限增长会拖慢存储/回放，
/// 按「最近 max_recent 条」窗口裁剪（denova 语义：可见轮次 + 压缩保留）；
/// 裁剪时**优先保留 high 严重度**（低优先级 med 先被淘汰），返回裁剪后的完整列表。
pub fn retain_guard_events(events: &[String], max_recent: usize) -> Vec<String> {
    if max_recent == 0 {
        return Vec::new();
    }
    if events.len() <= max_recent {
        return events.to_vec();
    }
    let overflow = events.len() - max_recent;
    // 从头淘汰 overflow 条，优先淘汰 med（high 保底）
    let mut kept: Vec<String> = events.to_vec();
    let mut removed = 0usize;
    // 第一遍：只淘汰 med（从头扫，保留窗口尾巴的 high）
    let mut i = 0usize;
    while i < kept.len() && removed < overflow {
        if !kept[i].starts_with("[high]") {
            kept.remove(i);
            removed += 1;
        } else {
            i += 1;
        }
    }
    // 第二遍：仍超窗 → 从头淘汰（含 high，最老优先）
    while kept.len() > max_recent {
        kept.remove(0);
    }
    kept
}

/// G10 (吞噬 denova D1 回合提交状态机): 回合提交幂等守卫（纯函数）。
/// 同一回合同内容已提交（已有 user 消息且 hash 相同）→ true（拒绝重复提交）；
/// 否则 false。
///
/// 判定规则：取末条 user 消息，其内容 hash 与本次提交 hash 相同，**且**其后已有
/// assistant 回应（该回合已完成，重复提交只回执不重跑）→ true。
/// 其余情况一律 false：
/// - 不同内容（末条 user 消息 hash 不同）→ 放行；
/// - 新回合（末条为 assistant，上一回合已完成；或末条 user 消息尚无 assistant 回应，
///   即回合挂起/LLM 空响应后可复用重试路径）→ 放行。
///
/// hash 用简单 FNV（`crate::text_hash`），由调用方（server `start_turn`）计算。
pub fn turn_submit_guard(session: &TavernSession, user_msg_hash: &str) -> bool {
    let Some(last_user_pos) = session.messages.iter().rposition(|m| m.role == "user") else {
        return false;
    };
    if text_hash(&session.messages[last_user_pos].content) != user_msg_hash {
        return false;
    }
    // 该 user 消息之后必须已有 assistant 回应（回合已完成）才算重复提交；
    // 无回应（回合挂起/失败可重试）→ 放行复用重试路径。
    session.messages[last_user_pos + 1..]
        .iter()
        .any(|m| m.role == "assistant")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectorRunPolicy {
    #[serde(default = "default_director_mode")]
    pub mode: String,
    #[serde(default)]
    pub interval_turns: u32,
    /// G8 (吞噬 denova strategy.failure_policy): 导演计划 LLM 失败时的处理策略。
    /// 枚举: fail_forward=失败继续推进（默认）/ success_at_cost=代价成功 / blocked=阻断 / hard_failure=硬失败
    #[serde(default = "default_failure_policy")]
    pub failure_policy: String,
    /// G8 (吞噬 denova strategy.pacing_curve): 剧情节奏曲线（wave=波浪 / goal-pressure-payoff=目标压力回报 / linear=线性…）。
    /// 空串 = 不指定，由导演计划自由发挥。
    #[serde(default)]
    pub pacing_curve: String,
    /// G8 (吞噬 denova strategy.event_frequency): 事件频率档。off=关闭 / sparse=稀疏 / balanced=均衡（默认）/ frequent=频繁
    #[serde(default = "default_event_frequency")]
    pub event_frequency: String,
    /// G8 (吞噬 denova strategy.rule_visibility_mode): 检定规则可见性。audit_only=仅审计（默认）/ public_roll=公开掷骰
    #[serde(default = "default_rule_visibility")]
    pub rule_visibility_mode: String,
    /// G8 (吞噬 denova strategy.branch_planning_turns): 分支规划回合数（导演计划为未来 N 回合准备承接策略），默认 5。
    #[serde(default = "default_branch_planning_turns")]
    pub branch_planning_turns: u32,
}

pub(crate) fn default_director_mode() -> String {
    "on_demand".into()
}

pub(crate) fn default_failure_policy() -> String {
    "fail_forward".into()
}

pub(crate) fn default_event_frequency() -> String {
    "balanced".into()
}

pub(crate) fn default_rule_visibility() -> String {
    "audit_only".into()
}

pub(crate) fn default_branch_planning_turns() -> u32 {
    5
}

impl Default for DirectorRunPolicy {
    fn default() -> Self {
        Self {
            mode: default_director_mode(),
            interval_turns: 0,
            failure_policy: default_failure_policy(),
            pacing_curve: String::new(),
            event_frequency: default_event_frequency(),
            rule_visibility_mode: default_rule_visibility(),
            branch_planning_turns: default_branch_planning_turns(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirectorModuleRefs {
    #[serde(default)]
    pub narrative_style_id: Option<String>,
    #[serde(default)]
    pub event_package_ids: Vec<String>,
    #[serde(default)]
    pub rule_system_id: Option<String>,
    #[serde(default)]
    pub actor_state_id: Option<String>,
    #[serde(default)]
    pub image_preset_id: Option<String>,
    /// G11 (吞噬 denova module_refs *_disabled): 显式关闭模块（关闭时保留原 ID 以便重新启用）。
    /// 默认 false = 模块启用；true = 模块关闭但 ID 保留。
    #[serde(default)]
    pub narrative_style_disabled: bool,
    #[serde(default)]
    pub event_packages_disabled: bool,
    #[serde(default)]
    pub rule_system_disabled: bool,
    #[serde(default)]
    pub actor_state_disabled: bool,
    #[serde(default)]
    pub image_preset_disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DirectorResolvedSnapshot {
    #[serde(default)]
    pub narrative_style: Option<Value>,
    #[serde(default)]
    pub event_packages: Vec<Value>,
    #[serde(default)]
    pub rule_system: Option<Value>,
    #[serde(default)]
    pub actor_state: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryPack {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub source: PackSource,
    #[serde(default)]
    pub characters: Vec<PackCharacterRef>,
    #[serde(default)]
    pub world_book_ids: Vec<String>,
    #[serde(default)]
    pub chapters: Vec<StoryChapter>,
    #[serde(default)]
    pub nodes: Vec<StoryNode>,
    #[serde(default)]
    pub lore_entries: Vec<Value>,
    #[serde(default)]
    pub default_mode: PlayMode,
    #[serde(default)]
    pub max_tier: ContentTier,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// 演出机导演台配置（S1 骨架，缺省为空策略，不驱动任何行为）。
    #[serde(default)]
    pub stage_director: StageDirectorConfig,
    /// S5: 事件卡包（吞噬 denova event_package）。
    #[serde(default)]
    pub event_packages: Vec<EventPackage>,
    /// S5/S6: 演出机 Actor 状态配置（吞噬 denova actor_state）。
    /// pack 级模板 + 初始角色；create_from_pack 时初始化进 session.actor_states。
    #[serde(default)]
    pub actor_state_config: ActorStatePackConfig,
    /// T 层 (2026-08-19): 原著静态时间线（旁挂 worldline.json 蒸馏产物）。
    /// 运行时按当前章过滤注入（chapter ≤ 当前章），与会话 L2 事件账本构成「静态+动态」双源时间线。
    /// 此前旁挂文件零消费——PackStore 只读 pack.json，worldline.json 蒸了白蒸（D3 缺口）。
    /// serde default 保证旧 pack（无该字段）加载零影响。
    #[serde(default)]
    pub worldline: Vec<Value>,
}

impl StoryPack {
    pub fn is_playable(&self) -> bool {
        if self.characters.is_empty() || self.chapters.len() < 2 {
            return false;
        }
        // Need a connected chain of ≥2 nodes across chapters
        if self.nodes.len() < 2 {
            return false;
        }
        let mut has_edge = false;
        for n in &self.nodes {
            if !n.exit.is_empty() {
                has_edge = true;
                break;
            }
        }
        has_edge
    }

    pub fn first_node_id(&self) -> Option<String> {
        let mut chs = self.chapters.clone();
        chs.sort_by_key(|c| c.order);
        for ch in chs {
            if let Some(nid) = ch.node_ids.first() {
                return Some(nid.clone());
            }
        }
        self.nodes.first().map(|n| n.id.clone())
    }

    pub fn first_chapter_id(&self) -> Option<String> {
        let mut chs = self.chapters.clone();
        chs.sort_by_key(|c| c.order);
        chs.first().map(|c| c.id.clone())
    }
}

/// Lightweight character ref carried by pack list responses, so the create
/// wizard can offer vessel/focus choices without a second full-pack fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackCharacterSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub role: String,
    /// P2-1 立绘层：默认立绘（列表也要带，供剧场立绘容器渲染）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// P2-1 立绘层：情绪名 → 立绘图 URL（列表带全量，前端按情绪取）。
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub expressions: std::collections::HashMap<String, String>,
    /// P3 语音层：edge-tts 音色名（列表端点透传，前端朗读按角色取音色）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackSummary {
    pub id: String,
    pub title: String,
    pub max_tier: ContentTier,
    pub chapter_count: usize,
    pub node_count: usize,
    pub character_count: usize,
    pub playable: bool,
    pub language: String,
    /// Short blurb for library cards (from lore 简介 / first lore / first node summary).
    #[serde(default)]
    pub blurb: String,
    /// Display cast (excludes narrator/player and junk auto-extracted names).
    #[serde(default)]
    pub cast_names: Vec<String>,
    /// Lightweight character refs (id+name+role), same filter as cast_names.
    #[serde(default)]
    pub characters: Vec<PackCharacterSummary>,
    /// First chapter title for library subtitle.
    #[serde(default)]
    pub first_chapter_title: String,
    /// Pack source type: novel | manual | demo | …
    #[serde(default)]
    pub source_type: String,
}

pub(crate) fn lore_text_field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
