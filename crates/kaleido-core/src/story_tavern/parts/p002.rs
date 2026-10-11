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

/// Heuristic: real character names are short CJK (or latin), not narrative fragments.
pub fn is_clean_cast_name(name: &str) -> bool {
    let n = name.trim();
    let chars: Vec<char> = n.chars().collect();
    if chars.len() < 2 || chars.len() > 8 {
        return false;
    }
    // reject common auto-extract garbage prefixes / particles
    let junk_prefixes = [
        "露出", "眼角", "换鞋", "随口", "轻声", "低头", "抬起", "转身", "伸手", "走过去", "看向",
        "听见", "突然", "只是", "已经", "然后", "因为", "所以", "连忙", "依旧", "还是", "还是",
        "你也没", "你也有", "你倒是", "我也没", "他也没", "她也没", "它也没", "有一", "有一个",
        "有一张", "有一件", "有一本", "有一瓶", "有一把", "第一道", "第二道", "第三道", "第四道",
        "第五道", "第六道", "第七道", "第八道", "第九道", "第一张", "第二张", "第三张", "第四张",
        "第五张", "第六张", "第七张", "第八张", "第九张", "张依旧", "李依旧", "王依旧", "依旧",
        "跟莫", "跟张", "跟李", "跟王", "跟赵", "跟钱", "跟孙", "跟周", "跟吴", "跟郑", "跟陈",
        "跟刘", "跟黄", "跟杨", "跟着", "跟着",
    ];
    for p in junk_prefixes {
        if n.starts_with(p) {
            return false;
        }
    }
    // reject names that start with obvious non-name words (pronouns, particles, prepositions, numbers)
    let non_name_starts: &[(char, Option<char>)] = &[
        ('跟', None), ('同', None), ('向', None), ('把', None), ('被', None), ('给', None),
        ('对', None), ('从', None), ('和', None), ('与', None), ('及', None), ('或', None),
        ('让', None), ('叫', None), ('让', None),
        ('你', None), ('我', None), ('他', None), ('她', None), ('它', None),
        ('这', None), ('那', None), ('第', None),
        ('一', None), ('二', None), ('三', None), ('四', None), ('五', None),
        ('六', None), ('七', None), ('八', None), ('九', None), ('十', None),
        ('有', None), ('没', None), ('不', None), ('也', None), ('就', None), ('还', None),
        ('都', None), ('却', None), ('而', None), ('但', None), ('且', None),
    ];
    if let Some(first) = chars.first().copied() {
        if non_name_starts.iter().any(|(c, _)| *c == first) {
            // [fix 2026-08-16] '向' 是常见姓氏（向明初/向华强）：「向」开头的 3 字及以上
            // 名字放行，仅拦截 2 字介词短语（向他/向前/向外）。
            if !(first == '向' && chars.len() >= 3) {
                return false;
            }
        }
    }
    // reject names that end with adverbial particles / common sentence fragments
    let junk_suffixes = ["连忙", "突然", "一直", "已经", "然后", "于是", "接着", "并且", "而且"];
    for s in junk_suffixes {
        if n.ends_with(s) {
            return false;
        }
    }
    if n.contains('的') || n.contains('了') || n.contains('着') || n.contains('在') {
        // allow classic 2-char names only if pure CJK without particles mid-word — already filtered
        if chars.iter().any(|c| matches!(c, '的' | '了' | '着' | '在' | '把' | '被')) {
            return false;
        }
    }
    let cjk = chars
        .iter()
        .filter(|&&c| matches!(c, '\u{4e00}'..='\u{9fff}' | '·' | '•'))
        .count();
    let latin = chars
        .iter()
        .filter(|&&c| c.is_ascii_alphabetic() || matches!(c, ' ' | '-' | '\''))
        .count();
    cjk == chars.len() || (latin == chars.len() && chars.len() >= 2)
}

pub(crate) fn pack_blurb(p: &StoryPack) -> String {
    // Prefer lore titled 简介 / 概述
    for lore in &p.lore_entries {
        let title = lore_text_field(lore, "title").unwrap_or_default();
        if title.contains("简介") || title.contains("概述") || title.eq_ignore_ascii_case("blurb")
        {
            if let Some(t) = lore_text_field(lore, "text").or_else(|| lore_text_field(lore, "content"))
            {
                return t.chars().take(140).collect();
            }
        }
    }
    // Any permanent lore text
    for lore in &p.lore_entries {
        let permanent = lore
            .get("permanent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if permanent {
            if let Some(t) = lore_text_field(lore, "text").or_else(|| lore_text_field(lore, "content"))
            {
                return t.chars().take(140).collect();
            }
        }
    }
    // First lore
    if let Some(lore) = p.lore_entries.first() {
        if let Some(t) = lore_text_field(lore, "text").or_else(|| lore_text_field(lore, "content")) {
            return t.chars().take(140).collect();
        }
    }
    // First node summary
    if let Some(n) = p.nodes.first() {
        let s = n.summary.trim();
        if !s.is_empty() {
            return s.chars().take(120).collect();
        }
    }
    String::new()
}

pub(crate) fn pack_cast_names(p: &StoryPack) -> Vec<String> {
    p.characters
        .iter()
        .filter(|c| {
            let role = c.role.to_ascii_lowercase();
            if role.contains("narrator") || role.contains("player") {
                return false;
            }
            let name = c.name.trim();
            if name.is_empty() || name == "旁白" || name == "读者" || name == "玩家" {
                return false;
            }
            is_clean_cast_name(name)
        })
        .map(|c| c.name.trim().to_string())
        .take(64)
        .collect()
}

/// Lightweight character refs for list responses (same filter as cast_names).
pub(crate) fn pack_character_summaries(p: &StoryPack) -> Vec<PackCharacterSummary> {
    p.characters
        .iter()
        .filter(|c| {
            let role = c.role.to_ascii_lowercase();
            if role.contains("narrator") || role.contains("player") {
                return false;
            }
            let name = c.name.trim();
            if name.is_empty() || name == "旁白" || name == "读者" || name == "玩家" {
                return false;
            }
            is_clean_cast_name(name)
        })
        .take(64)
        .map(|c| PackCharacterSummary {
            id: c.id.clone(),
            name: c.name.trim().to_string(),
            role: c.role.clone(),
            avatar: c.avatar.clone(),
            expressions: c.expressions.clone(),
            voice: c.voice.clone(),
        })
        .collect()
}

impl From<&StoryPack> for PackSummary {
    fn from(p: &StoryPack) -> Self {
        Self {
            id: p.id.clone(),
            title: p.title.clone(),
            max_tier: p.max_tier,
            chapter_count: p.chapters.len(),
            node_count: p.nodes.len(),
            character_count: p.characters.len(),
            playable: p.is_playable(),
            language: if p.language.is_empty() {
                "zh".into()
            } else {
                p.language.clone()
            },
            blurb: pack_blurb(p),
            cast_names: pack_cast_names(p),
            characters: pack_character_summaries(p),
            first_chapter_title: p
                .chapters
                .first()
                .map(|c| c.title.clone())
                .unwrap_or_default(),
            source_type: p.source.source_type.clone(),
        }
    }
}

// ─── Session ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EntryConfig {
    #[serde(default)]
    pub entry_role: Option<EntryRole>,
    #[serde(default)]
    pub vessel_character_id: Option<String>,
    #[serde(default)]
    pub meta_knowledge: MetaKnowledge,
    #[serde(default)]
    pub rewrite_intensity: RewriteIntensity,
    #[serde(default)]
    pub isekai: Option<Value>,
    #[serde(default)]
    pub extra_profile: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlayerState {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub control_character_id: Option<String>,
    #[serde(default)]
    pub persona: String,
    #[serde(default)]
    pub inventory: Vec<String>,
    #[serde(default)]
    pub flags: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryL1 {
    #[serde(default)]
    pub scene_summary: String,
    #[serde(default)]
    pub updated_at_turn: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryL2Event {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub turn: u32,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub actors: Vec<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    /// Pre-computed embedding vector (ST-21: embedding RAG).
    #[serde(default)]
    pub embedding: Vec<f32>,
}

/// L2: short-horizon event log (capped).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryL2 {
    #[serde(default)]
    pub events: Vec<MemoryL2Event>,
    #[serde(default)]
    pub updated_at_turn: u32,
}

/// L3: fine-grained relationship edges / facts (capped).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryL3 {
    /// list of {from,to,rel,note,turn}
    #[serde(default)]
    pub edges: Vec<Value>,
    /// free-form facts
    #[serde(default)]
    pub facts: Vec<String>,
    /// [fix §10 2026-08-16] 永久层 facts：玩家显式声明的关键物品/收藏/承诺——
    /// 注入时不参与 take 裁剪（窝边草「素描原稿」被 take(6) 挤出的案例根治）。
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default)]
    pub updated_at_turn: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MemoryL4 {
    /// characterId → affinity 0–100
    #[serde(default)]
    pub affinity: Value,
    #[serde(default)]
    pub secrets_known: Vec<String>,
    #[serde(default)]
    pub promises: Vec<String>,
    #[serde(default)]
    pub relationships: Value,
}

/// L2 事件账本上限收缩：保留最近 `max` 条；超限时按重要性淘汰
/// （romance 等关系关键事件保底，其余从头淘汰）。与 retain_guard_events
/// 同策略（denova 语义：可见轮次 + 压缩保留），2026-08-14 补 L2 无上限收缩缺口。
pub fn retain_l2_events(events: &[MemoryL2Event], max: usize) -> Vec<MemoryL2Event> {
    if max == 0 || events.is_empty() {
        return Vec::new();
    }
    if events.len() <= max {
        return events.to_vec();
    }
    let overflow = events.len() - max;
    // 第一遍：优先淘汰非关键事件（kind != romance / 不含关系关键词）
    let mut kept: Vec<MemoryL2Event> = events.to_vec();
    let mut removed = 0usize;
    let mut i = 0usize;
    while i < kept.len() && removed < overflow {
        let e = &kept[i];
        let critical = e.kind == "romance"
            || e.summary.contains("接吻")
            || e.summary.contains("亲嘴")
            || e.summary.contains("亲吻")
            || e.summary.contains("亲热")
            || e.summary.contains("吻")
            || e.summary.contains("承诺")
            || e.summary.contains("秘密");
        if !critical {
            kept.remove(i);
            removed += 1;
        } else {
            i += 1;
        }
    }
    // 第二遍：仍超限则从头淘汰（保窗口尾巴）
    while kept.len() > max {
        kept.remove(0);
    }
    kept
}

#[cfg(test)]
mod l2_event_tests {
    use super::super::*;

    fn ev(id: &str, kind: &str, summary: &str) -> MemoryL2Event {
        MemoryL2Event {
            id: id.into(),
            turn: 0,
            kind: kind.into(),
            summary: summary.into(),
            actors: vec![],
            node_id: None,
            embedding: vec![],
        }
    }

    #[test]
    fn l2_keeps_within_limit() {
        let evs: Vec<MemoryL2Event> = (0..5).map(|i| ev(&format!("e{i}"), "other", "x")).collect();
        assert_eq!(retain_l2_events(&evs, 10).len(), 5);
    }

    #[test]
    fn l2_trims_old_first() {
        let mut evs: Vec<MemoryL2Event> = (0..10).map(|i| ev(&format!("e{i}"), "other", "x")).collect();
        evs.push(ev("rom1", "romance", "亲吻"));
        let kept = retain_l2_events(&evs, 5);
        assert_eq!(kept.len(), 5);
        // romance 保底：最后一条必须是关键事件
        assert!(kept.last().unwrap().kind == "romance" || kept.last().unwrap().summary.contains("吻"));
    }

    #[test]
    fn l2_empty_and_zero() {
        assert!(retain_l2_events(&[], 5).is_empty());
        assert!(retain_l2_events(&[ev("e1", "other", "x")], 0).is_empty());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TavernMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub options: Vec<String>,
    /// [Swipe 多备选 吞噬 Front Porch AI swipes] 同条回复的备选正文（regen 不覆盖，左右切换）。
    #[serde(default)]
    pub swipes: Vec<String>,
    #[serde(default)]
    pub swipe_index: usize,
    #[serde(default)]
    pub engine_tag: Option<EngineTag>,
    /// 消息流内嵌程序卡 HTML(吸收自梨园 show_html);前端沙箱 iframe 渲染。
    #[serde(default)]
    pub program: Option<String>,
    /// 模型推理/导演思考内容（折叠展示，玩家点箭头展开）。正文剥离出的
    /// 「好的我是XX…」自白段与 LLM reasoning_content 均归入此字段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// [token 显示 2026-08-16] 消息生成消耗的 token 总数（上游 SSE usage.total_tokens）。
    /// 默认 0 = 旧数据/未捕获；前端 stMsgMeta 开关显示「HH:MM · N tok」。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tokens: u32,
}

pub(crate) fn is_zero(v: &u32) -> bool {
    *v == 0
}

// ─── 演出机 S1：Actor 状态系统（纯数据结构骨架）────────────────────────────

// P2-1: 情绪字段约定——角色 fields["emotion"] 的合法取值（驱动前端表情角标）。
// [P7] EMOTION_FIELD/EMOTION_VALUES 常量已删（无代码引用；约定以注释为准）：
//   字段名固定 "emotion"；合法值 平静/开心/愤怒/悲伤/害羞/惊讶/恐惧/厌恶/疲惫/心动。

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActorStateSystem {
    #[serde(default = "default_actor_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub initial_actors: Vec<ActorStateInitialActor>,
    #[serde(default)]
    pub actors: std::collections::HashMap<String, ActorStateEntry>,
    #[serde(default)]
    pub trait_pools: Vec<ActorTraitPool>,
    #[serde(default)]
    pub archive: Vec<ActorArchiveSnapshot>,
    /// 创作罗盘（T2）：全书承诺 + 近期目标。随会话落盘，build_context_text 置顶注入。
    /// 空罗盘（默认）不注入。
    #[serde(default)]
    pub compass: Compass,
}

pub(crate) fn default_actor_schema_version() -> u32 {
    3
}

/// Pack 级 Actor 状态配置（吞噬 denova actor_state）。
/// create_from_pack 时据此初始化 session.actor_states：
/// initial_actors 声明哪些角色启用状态机 + 用哪个模板；templates 提供字段模板。
///
/// 字段生命周期契约(对齐 TavernWeave variable-systems 方法论):
/// - writer:    ST-26 LLM 更新块(经 min/max/options 校验后写回)
/// - reader:    前端 UI(display 字段)/叙事渲染
/// - renderer:  actor_states → 剧情上下文投影
/// - cleanup:   无(字段随 actor 存档归档)
/// - migration: schema_version 控制;旧存档按模板默认值补齐
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorStatePackConfig {
    #[serde(default = "default_actor_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub initial_actors: Vec<ActorStateInitialActor>,
    /// template_id → 字段模板（含数值范围/枚举/更新说明；value 为初始值）。
    #[serde(default)]
    pub templates: std::collections::HashMap<String, ActorStateTemplate>,
}

/// 角色状态模板：一组字段定义（数值范围/枚举/更新说明）+ 可选特质池。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorStateTemplate {
    #[serde(default)]
    pub fields: std::collections::HashMap<String, ActorFieldValue>,
    #[serde(default)]
    pub trait_pools: Vec<ActorTraitPool>,
}

impl ActorStatePackConfig {
    /// 把 pack 级配置物化为可落盘的 ActorStateSystem（不直接改 session 序列化结构）。
    pub fn to_system(&self) -> ActorStateSystem {
        let mut sys = ActorStateSystem {
            schema_version: self.schema_version,
            initial_actors: self.initial_actors.clone(),
            actors: std::collections::HashMap::new(),
            trait_pools: Vec::new(),
            archive: Vec::new(),
            compass: Compass::empty(),
        };
        for ia in &self.initial_actors {
            let mut entry = ActorStateEntry {
                template_id: ia.template_id.clone(),
                fields: std::collections::HashMap::new(),
                traits: Vec::new(),
            };
            if let Some(tpl) = self.templates.get(&ia.template_id) {
                entry.fields = tpl.fields.clone();
                // 收集该模板的 trait_pools（按 id 去重）
                for pool in &tpl.trait_pools {
                    if !sys.trait_pools.iter().any(|p| p.id == pool.id) {
                        sys.trait_pools.push(pool.clone());
                    }
                }
            }
            sys.actors.insert(ia.character_id.clone(), entry);
        }
        sys
    }
}

impl Default for ActorStateSystem {
    fn default() -> Self {
        Self {
            schema_version: default_actor_schema_version(),
            initial_actors: vec![],
            actors: std::collections::HashMap::new(),
            trait_pools: vec![],
            archive: vec![],
            compass: Compass::empty(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorStateInitialActor {
    #[serde(default)]
    pub character_id: String,
    #[serde(default)]
    pub template_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorStateEntry {
    #[serde(default)]
    pub template_id: String,
    #[serde(default)]
    pub fields: std::collections::HashMap<String, ActorFieldValue>,
    #[serde(default)]
    pub traits: Vec<ActorTraitInstance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorFieldValue {
    /// number | string | bool | enum | object | list
    #[serde(default)]
    pub value_type: String,
    /// 修订审计（吞噬 nov-manager update_character_state 语义）：谁/何时/为何改。
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub updated_turn: u64,
    #[serde(default)]
    pub updated_reason: String,
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub display: Option<String>,
    /// ST-26: 该字段在剧情中应如何变化的说明（供 LLM 生成【状态更新】块参考）。
    #[serde(default)]
    pub update_instruction: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorTraitInstance {
    #[serde(default)]
    pub pool_id: String,
    #[serde(default)]
    pub pool_name: Option<String>,
    #[serde(default)]
    pub trait_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub source_turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorTraitPool {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub traits: Vec<ActorTraitDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorTraitDefinition {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub weight: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorArchiveSnapshot {
    #[serde(default)]
    pub character_id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub state: Value,
    /// S6: 归档原因（auto | manual | story），由 archive_actor 写入。
    #[serde(default)]
    pub reason: String,
}

/// ST-26: 一回合中 LLM 输出的角色状态更新块（吞噬 denova actor_state）。
/// 由后端套入 min/max/type 校验后写入 session.actor_states 并落盘。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ActorStateUpdate {
    #[serde(default)]
    pub character_id: String,
    #[serde(default)]
    pub fields: std::collections::HashMap<String, Value>,
    #[serde(default)]
    pub add_traits: Vec<ActorTraitInstance>,
    #[serde(default)]
    pub remove_traits: Vec<String>,
}

// ─── 演出机 S3：规则检定（吞噬 denova TurnCheckRequest，d20 最小闭环）────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleStateBinding {
    #[serde(default)]
    pub field: String, // "敌方.压力" → actor_id.field_id
    #[serde(default)]
    pub on_success: Option<Value>, // "+1" / 数值 / 其它
    #[serde(default)]
    pub on_fail: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleCheck {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub dice: String,
    #[serde(default)]
    pub modifier: f64,
    #[serde(default)]
    pub failure_policy: String,
    #[serde(default)]
    pub difficulty_guidance: String,
    #[serde(default)]
    pub state_effect_guidance: String,
    #[serde(default)]
    pub trigger: String,
    #[serde(default)]
    pub must_check_examples: Vec<String>,
    #[serde(default)]
    pub skip_check_examples: Vec<String>,
    #[serde(default)]
    pub success_hint: String,
    #[serde(default)]
    pub failure_hint: String,
    #[serde(default)]
    pub state_bindings: Vec<RuleStateBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleSystem {
    #[serde(default)]
    pub checks: Vec<RuleCheck>,
}

impl RuleSystem {
    /// 宽容解析 DirectorResolvedSnapshot.rule_system 的 Value；非对象/解析失败 → None
    pub fn from_value(v: &Value) -> Option<RuleSystem> {
        serde_json::from_value(v.clone()).ok()
    }
}

/// ST-27: LLM 输出的规则检定块（吞噬 denova TurnCheckRequest，精简版）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnCheckRequest {
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub intent: String,
    #[serde(default)]
    pub challenge: String,
    #[serde(default)]
    pub cost: String,
    #[serde(default)]
    pub difficulty: String, // very_easy|easy|normal|hard|very_hard
    #[serde(default)]
    pub template_id: Option<String>,
    #[serde(default)]
    pub bonuses: Vec<TurnCheckBonus>,
    #[serde(default)]
    pub outcomes: TurnCheckOutcomes,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnCheckBonus {
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnCheckOutcomes {
    #[serde(default)]
    pub critical_success: TurnCheckOutcome,
    #[serde(default)]
    pub success: TurnCheckOutcome,
    #[serde(default)]
    pub failure: TurnCheckOutcome,
    #[serde(default)]
    pub critical_failure: TurnCheckOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnCheckOutcome {
    #[serde(default)]
    pub result: String,
    #[serde(default)]
    pub state_changes: Vec<TurnStateChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TurnStateChange {
    #[serde(default)]
    pub actor_id: String,
    #[serde(default)]
    pub field_id: String,
    #[serde(default)]
    pub change: f64,
    #[serde(default)]
    pub reason: String,
}

/// 单条检定历史（全量累积，导演台「检定结果」区块展示用）。与 last_check_results
///（仅最近一回合、注入 prompt 约束）不同，此字段跨回合累积所有检定，供人类审阅。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckHistoryEntry {
    #[serde(default)]
    pub action: String,
    /// critical_success|success|failure|critical_failure
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub result_text: String,
    #[serde(default)]
    pub natural: u32,
    #[serde(default)]
    pub total: f64,
    #[serde(default)]
    pub dc: f64,
    /// 掷骰种子（session|turn|action 派生，可复跑验证；吞噬 mnehmos dice 种子复现）。
    #[serde(default)]
    pub seed: String,
    /// 命中角色字段变化（部分角色元数据，可空）。
    #[serde(default)]
    pub state_changes: Vec<TurnStateChange>,
    /// 发生时的回合序号（0 = 开场前）。
    #[serde(default)]
    pub turn: u64,
}

/// "1d20"→(1,20)；"2d6"→(2,6)；非法 → None（支持 XdY，X/Y ≥1）
pub fn parse_dice(spec: &str) -> Option<(usize, usize)> {
    let spec = spec.trim();
    if spec.is_empty() {
        return None;
    }
    let idx = spec.find(['d', 'D'])?;
    let count = spec[..idx].trim().parse::<usize>().ok()?;
    let faces = spec[idx + 1..].trim().parse::<usize>().ok()?;
    if count < 1 || faces < 1 {
        return None;
    }
    Some((count, faces))
}

/// very_easy=5 easy=8 normal=12 hard=15 very_hard=18；未知 → 12
pub fn difficulty_to_dc(difficulty: &str) -> f64 {
    match difficulty.trim().to_ascii_lowercase().as_str() {
        "very_easy" => 5.0,
        "easy" => 8.0,
        "normal" => 12.0,
        "hard" => 15.0,
        "very_hard" => 18.0,
        _ => 12.0,
    }
}

pub struct CheckResult {
    pub natural: u32,
    pub total: f64,
    pub dc: f64,
    pub outcome: String,     // critical_success|success|failure|critical_failure
    pub result_text: String, // 命中档 result；为空则回退 template success_hint/failure_hint；再空则 challenge
    pub state_changes: Vec<TurnStateChange>,
}

/// roll：掷骰子(dice 解析，X 次 Y 面求和取自然骰；d20 只掷 1 次) + bonuses.value 求和 → total。
/// template: 命中 RuleSystem.checks[template_id]（找不到或 template_id 空 → None）。
/// dc = difficulty_to_dc(difficulty) + template.map(|t| t.modifier).unwrap_or(0.0)  // modifier 正数=更难
/// 判定：natural==20→critical_success；natural==1→critical_failure；total>=dc→success；否则 failure。
pub fn roll_check(
    req: &TurnCheckRequest,
    template: Option<&RuleCheck>,
    mut roll_d20: impl FnMut() -> u32,
) -> CheckResult {
    let dice = template
        .map(|t| t.dice.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("1d20");
    let (count, faces) = parse_dice(dice).unwrap_or((1, 20));
    let mut natural = 0u32;
    for _ in 0..count {
        let v = roll_d20();
        natural += if faces < 20 { ((v - 1) % faces as u32) + 1 } else { v };
    }
    let bonus: f64 = req.bonuses.iter().map(|b| b.value).sum();
    let total = natural as f64 + bonus;
    let dc = difficulty_to_dc(&req.difficulty) + template.map(|t| t.modifier).unwrap_or(0.0);
    let outcome = if natural == 20 {
        "critical_success"
    } else if natural == 1 {
        "critical_failure"
    } else if total >= dc {
        "success"
    } else {
        "failure"
    };
    let hit = match outcome {
        "critical_success" => &req.outcomes.critical_success,
        "success" => &req.outcomes.success,
        "failure" => &req.outcomes.failure,
        _ => &req.outcomes.critical_failure,
    };
    let mut result_text = hit.result.trim().to_string();
    if result_text.is_empty() {
        if let Some(t) = template {
            let hint = if matches!(outcome, "success" | "critical_success") {
                &t.success_hint
            } else {
                &t.failure_hint
            };
            if !hint.trim().is_empty() {
                result_text = hint.trim().to_string();
            }
        }
    }
    if result_text.is_empty() {
        result_text = req.challenge.clone();
    }
    let mut state_changes = hit.state_changes.clone();
    if let Some(t) = template {
        let success_side = matches!(outcome, "success" | "critical_success");
        for b in &t.state_bindings {
            let raw = if success_side { &b.on_success } else { &b.on_fail };
            let Some(raw) = raw else { continue };
            let field = b.field.trim();
            if field.is_empty() {
                continue;
            }
            let (actor_id, field_id) = match field.find('.') {
                Some(i) => (&field[..i], &field[i + 1..]),
                None => (field, ""),
            };
            let (change, reason) = match raw {
                Value::Number(n) => (n.as_f64().unwrap_or(0.0), format!("{field} 检定结算")),
                Value::String(s) => match s.trim().parse::<f64>() {
                    Ok(v) => (v, format!("{field} 检定结算")),
                    Err(_) => (0.0, s.trim().to_string()),
                },
                other => (0.0, other.to_string()),
            };
            state_changes.push(TurnStateChange {
                actor_id: actor_id.trim().to_string(),
                field_id: field_id.trim().to_string(),
                change,
                reason,
            });
        }
    }
    CheckResult {
        natural,
        total,
        dc,
        outcome: outcome.to_string(),
        result_text,
        state_changes,
    }
}

impl ActorStateSystem {
    /// 挂载创作罗盘（T2）：写入系统状态的 compass，供 build_context_text 置顶注入。
    pub fn mount_compass(&mut self, compass: Compass) {
        self.compass = compass;
    }

    /// 当前挂载的创作罗盘（只读）。
    pub fn compass(&self) -> &Compass {
        &self.compass
    }

    /// 渲染所有立着角色的当前状态为可读文本（供 system prompt 注入）。
    /// 创作罗盘段置顶输出于角色状态之前：author_intent/current_focus 非空各自输出
    /// 「【全书承诺】…」「【近期目标】…」；空罗盘不干扰既有输出（无角色+空罗盘仍为空串）。
    pub fn build_context_text(&self) -> String {
        let mut out = self.compass.render_block();
        if self.actors.is_empty() {
            return out;
        }
        for (character_id, entry) in &self.actors {
            out.push_str(&format!("# {}\n", character_id));
            for (name, fv) in &entry.fields {
                let val = fv.value.as_ref().map(|v| v.to_string()).unwrap_or_else(|| "∅".into());
                let mut line = format!("- {}: {}", name, val);
                if let (Some(lo), Some(hi)) = (fv.min, fv.max) {
                    line.push_str(&format!(" ({lo}~{hi})"));
                }
                if let Some(instr) = fv.update_instruction.as_deref() {
                    if !instr.is_empty() {
                        line.push_str(&format!(" —— 更新说明: {instr}"));
                    }
                }
                out.push_str(&line);
                out.push('\n');
            }
            for t in &entry.traits {
                let mut line = format!("- trait: {} ({})", t.name, t.trait_id);
                if let Some(s) = &t.summary {
                    line.push_str(&format!(" —— {s}"));
                }
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }

    /// 应用一批状态更新，返回发生变更的字段数。
    /// upsert：character_id 无 entry 时新建；fields 按 update.fields 覆盖。
    /// add_traits append（若同名 trait_id 已存在则忽略）。
    /// remove_traits 按 trait_id 移除。
    pub fn apply_updates(&mut self, updates: &[ActorStateUpdate]) -> usize {
        let mut changed = 0usize;
        for up in updates {
            if up.character_id.is_empty() {
                continue;
            }
            let entry = self.actors.entry(up.character_id.clone()).or_default();
            for (name, val) in &up.fields {
                let f = entry.fields.entry(name.clone()).or_default();
                f.value = Some(val.clone());
                if f.updated_by.is_empty() {
                    f.updated_by = "api".to_string();
                }
                changed += 1;
            }
            for t in &up.add_traits {
                if !entry.traits.iter().any(|e| e.trait_id == t.trait_id) {
                    entry.traits.push(t.clone());
                }
            }
            entry.traits.retain(|t| !up.remove_traits.contains(&t.trait_id));
        }
        changed
    }

    /// 按 actor_id+field_id 定位 number 字段，加 change 写回；找不到数值字段则忽略。
    /// 返回成功更新数。value 为 JSON number 或字符串可解析 f64 均可。
    pub fn apply_state_changes(&mut self, changes: &[TurnStateChange]) -> usize {
        let mut updated = 0usize;
        for ch in changes {
            if ch.actor_id.is_empty() || ch.field_id.is_empty() {
                continue;
            }
            let Some(entry) = self.actors.get_mut(&ch.actor_id) else {
                continue;
            };
            let Some(fv) = entry.fields.get_mut(&ch.field_id) else {
                continue;
            };
            let current = match &fv.value {
                Some(Value::Number(n)) => n.as_f64(),
                Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
                _ => None,
            };
            let Some(cur) = current else {
                continue;
            };
            fv.value = Some(json!(cur + ch.change));
            updated += 1;
        }
        updated
    }

    /// S6: 将角色当前状态归档为快照（吞噬 denova actor_archive）。
    /// 返回新快照；角色不存在返回 None。调用方负责把 system 写回。
    pub fn archive_actor(
        &mut self,
        character_id: &str,
        reason: &str,
        created_at: &str,
    ) -> Option<ActorArchiveSnapshot> {
        let entry = self.actors.get(character_id)?;
        let state = serde_json::to_value(entry).ok()?;
        let snap = ActorArchiveSnapshot {
            character_id: character_id.to_string(),
            created_at: created_at.to_string(),
            state,
            reason: reason.to_string(),
        };
        self.archive.push(snap.clone());
        Some(snap)
    }

    /// S6: 从归档恢复角色状态（取最近一次含该角色的快照覆盖 actors[character_id]）。
    pub fn restore_actor(&mut self, character_id: &str) -> bool {
        for snap in self.archive.iter().rev() {
            if snap.character_id != character_id {
                continue;
            }
            if let Ok(entry) = serde_json::from_value::<ActorStateEntry>(snap.state.clone()) {
                self.actors.insert(character_id.to_string(), entry);
                return true;
            }
        }
        false
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TavernSession {
    pub session_id: String,
    pub pack_id: String,
    #[serde(default)]
    pub pack_missing: bool,
    /// F1: owner user_id of this session. None on legacy data (treated as
    /// restricted — only the creating user can access, falling back to
    /// "first claimant" when unverifiable).
    #[serde(default)]
    pub owner: Option<String>,
    /// P0: 回合生成写作档位（lite/standard/heavy）。默认 lite = 现状单次直出。
    #[serde(default)]
    pub quality: Quality,
    pub playable: Playable,
    pub play_mode: PlayMode,
    /// Frozen at create: min(user, pack/card, global)
    pub content_tier: ContentTier,
    #[serde(default)]
    pub user_tier_request: ContentTier,
    #[serde(default)]
    pub entry: EntryConfig,
    #[serde(default)]
    pub chapter_cursor: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub resume_node_id: Option<String>,
    /// True once opening monologue was seeded (create / ensure-opening / side enter).
    #[serde(default)]
    pub opening_seeded: bool,
    /// Active side-branch node (playMode=side after enter).
    #[serde(default)]
    pub side_branch_node_id: Option<String>,
    /// Display label for active side branch.
    #[serde(default)]
    pub side_branch_label: Option<String>,
    /// 最近一次回档的存档 id（世界线分叉检测用，吸收自 Liyuan worldline.ts）。
    #[serde(default)]
    pub last_restored_save_id: Option<String>,
    /// 当前所在世界线 id（"main"=主线；回档后走出不同路再存档 → 新线）。
    #[serde(default)]
    pub current_worldline_id: Option<String>,
    /// Agent 自建面板（吸收自 Liyuan panels.ts：地图/装备库/线索板等舞台美术层）。
    #[serde(default)]
    pub panels: Vec<TavernPanel>,
    /// MCP 外设工具结果(吸收自 Liyuan mcp.ts, 默认仅本机 stdio server)。
    /// 上一轮【工具】调用的结果, 会在下一轮构建 system prompt 时回填给 LLM。
    #[serde(default)]
    pub mcp_tool_results: Vec<ToolResultBrief>,
    /// skill 工具按需加载结果（吸收自 denova skill.NewMiddleware：模型声明需要完整 SKILL.md 后，下轮构建 system prompt 时注入全文）。
    /// 上轮【技能加载】块触发时写入；下轮 read 到后注入并保持（可被新请求覆盖）。
    #[serde(default)]
    pub skill_load: Option<SkillLoadInfo>,
    #[serde(default = "default_timeline")]
    pub timeline_id: String,
    #[serde(default)]
    pub turn: u32,
    #[serde(default)]
    pub present_character_ids: Vec<String>,
    /// Current focus NPC for multi-speaker turns (ST-10). Rotates among presentCharacterIds.
    #[serde(default)]
    pub focus_character_id: Option<String>,
    /// When true (default for P2), rotate focus after each completed turn.
    #[serde(default = "default_true")]
    pub speaker_rotation: bool,
    #[serde(default)]
    pub player: PlayerState,
    #[serde(default)]
    pub memory_l1: MemoryL1,
    #[serde(default)]
    pub memory_l2: MemoryL2,
    #[serde(default)]
    pub memory_l3: MemoryL3,
    #[serde(default)]
    pub memory_l4: MemoryL4,
    /// P2 (叙界守卫): 生成后多维守卫的违规记录（high=打回阻止推进 / med=仅提示）。
    #[serde(default)]
    pub guard_events: Vec<String>,
    /// L0 recent turns (full transcript retained; prompt uses last N)
    #[serde(default)]
    pub messages: Vec<TavernMessage>,
    #[serde(default)]
    pub active_run_id: Option<String>,
    #[serde(default)]
    pub adult_confirmed: bool,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// Author Zone project this session writes docs into (AZ-1+).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_project_id: Option<String>,
    /// Relative works path for live scroll doc, e.g. projects/{id}/sessions/{sid}/live.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_live_path: Option<String>,
    /// AZ-6: when false, skip live append even if path set.
    #[serde(default = "default_true")]
    pub author_live_enabled: bool,
    /// AZ-6: append live every N completed turns (1 = every turn).
    #[serde(default = "default_live_every_n")]
    pub author_live_every_n: u32,
    /// AZ-6: also write sessions/.../turns/{n}.md per turn (fail-open).
    #[serde(default)]
    pub author_live_write_turns: bool,
    /// Actor 状态机（吞噬 denova actor-state；空 = 未启用）。
    #[serde(default)]
    pub actor_states: ActorStateSystem,
    /// 会话级世界状态（T2 U2/U7）：实体+关系+事件账本。create_from_pack 时从
    /// actor_state_config 播种 Character 实体；叙事事件经 /world-events 应用。
    #[serde(default)]
    pub world: WorldState,
    /// 游戏时钟+天气权威状态（时间/天气约束系统）。旧数据 default 播种（清晨/晴）。
    #[serde(default)]
    pub game_clock: crate::time_clock::GameClock,
    /// [吞噬 Front Porch AI pockets.dart] 口袋与衣物（per-character, per-session）。
    /// per-character 口袋（worn/carrying/setAside），per-session 隔离。key = character_id。
    /// GameClock 的 day 用于 setAside 晨间过期（clothing 次日清晨过期，possessions 永不过期）。
    #[serde(default)]
    pub pockets: std::collections::HashMap<String, crate::pockets::Pockets>,
    /// [P1-B Porch Life À la carte] 口袋开关（默认开）。关时提示词不注入口袋块，
    /// 但数据仍保留（可再打开）。对齐 Front Porch Porch Life "Own switch. Does not
    /// need the Realism Engine."。
    #[serde(default = "default_true")]
    pub pockets_enabled: bool,
    /// [P2+P3 吞噬 Front Porch AI] Needs 六维 + Growth Rings + World Climate（per-character/per-session，默认空）。
    #[serde(default)]
    pub needs: std::collections::HashMap<String, crate::needs::Needs>,
    #[serde(default)]
    pub growth: crate::character_arc::GrowthStore,
    #[serde(default)]
    pub world_climate: crate::world_climate::WorldClimate,
    /// [P4 吞噬 Front Porch AI chaos/tiers/objectives/dreams] Chaos + Tiers + 目标 + 夜梦（默认空/关）。
    #[serde(default)]
    pub chaos: crate::chaos::ChaosState,
    #[serde(default)]
    pub milestones: Vec<crate::relationship_tiers::Milestone>,
    #[serde(default)]
    pub objectives: Vec<crate::objectives::Objective>,
    #[serde(default)]
    pub ambitions: Vec<crate::objectives::Ambition>,
    #[serde(default)]
    pub dream: crate::dreams::DreamState,
    #[serde(default)]
    pub episodes: crate::dreams::EpisodeStore,
    /// [Journal 存量 吞噬 Front Porch AI journal_store] per-session卡片库（热卡常驻，冷卡按需召回）。
    #[serde(default)]
    pub journal: crate::journal_store::JournalStore,
    /// [羁绊活数值 吞噬 Front Porch AI relationship_service] per-character bond/trust 动态。
    #[serde(default)]
    pub relationships: std::collections::HashMap<String, crate::relationship::Bond>,
    /// [Swipe 多备选] 待继承的备选正文（reroll 时旧正文暂存，下条 assistant 消息继承）。
    #[serde(default)]
    pub pending_swipes: Vec<String>,
    /// [承诺债务 吞噬 Front Porch AI promise_debt] open/kept/broken 追踪。
    #[serde(default)]
    pub promises: crate::promise::PromiseStore,
    /// [偏好加权 吞噬 Front Porch AI preference_scoring] per-character likes/dislikes。
    #[serde(default)]
    pub preferences: std::collections::HashMap<String, crate::promise::Prefs>,
    /// [在场推导 吞噬 Front Porch AI presence_derive] per-character occupation/hours/workdays。
    #[serde(default)]
    pub presence: std::collections::HashMap<String, crate::mood_presence::Presence>,
    /// [世界书定时 吞噬 Front Porch AI lorebook_timed_effects] per-session sticky/cooldown（消息序号计）。
    #[serde(default)]
    pub timed_world_info: crate::st_world_info::TimedWorldInfo,
    /// [全自动事件提取] 回合末后台 LLM 提取物品/承诺/成长/羁绊（默认开，小模型低 token）。
    #[serde(default = "default_true")]
    pub event_extract: bool,
    /// S4: 导演计划（吞噬 denova director_plan）。None = 尚未生成。仅为叙事意图，不改写 locked_beats。
    #[serde(default)]
    pub director_plan: Option<DirectorPlan>,
    /// S4: 导演计划手动触发挂起标记：POST director-plan/run 置 true，下一回合 start_turn 附加生成指令。
    #[serde(default)]
    pub director_pending: bool,
    /// G13/G14: 当前后台导演任务 id（如 "director_plan_update" / "opening_plan"）。
    /// 后台任务运行期间登记，GET director-config 可见；任务结束清空为 None。
    #[serde(default)]
    pub director_task: Option<String>,
    /// S5: 最近一回合抽到的事件卡（None = 尚未抽取/无可用卡）。
    #[serde(default)]
    pub last_event: Option<EventLogEntry>,
    /// P12 (2026-08-15): 最近一回合的检定结果文本（含失败 outcome）。下回合构建
    /// system prompt 时注入为「剧情约束」——检定失败（含 critical_failure）必须
    /// 在正文中真实体现（如母亲明确拒绝/拉开距离/尖叫），不再只是事后 append 展示。
    #[serde(default)]
    pub last_check_results: Vec<String>,
    /// 跨回合累积的全部检定历史（导演台「检定结果」区块展示用）。
    #[serde(default)]
    pub check_history: Vec<CheckHistoryEntry>,
    /// P0-1: 回合检查点（cap 30，吸收自梨园 story_command /rewind /reroll）。
    /// 新会话的首个完成回合也保留一个“回合前”快照，确保 reroll 能回退首回合。
    #[serde(default)]
    pub checkpoints: Vec<TurnCheckpoint>,
    /// Chronicle 折叠水位线（吞噬 loreweaver chronicle lag 窗口口径）：
    /// ≤此回合的 L2 事件视为已折叠（只读摘要），回退禁跨水位。
    #[serde(default)]
    pub fold_through_turn: u32,
    /// U11: epoch 压缩代数 —— 上下文窗口阈值驱动压缩（替换机械 turn%8），每次触发 +1。
    #[serde(default)]
    pub epoch: u32,
    /// U11: 最近一次 epoch 压缩发生的回合。
    #[serde(default)]
    pub epoch_last_turn: Option<u32>,
    /// U11: 最近一次 epoch 压缩触发时的估算上下文字符数。
    #[serde(default)]
    pub epoch_last_chars: Option<u32>,
    /// U11: 多轮累计成本/耗时记账（每次回合终态增量累计；serde default 兼容旧会话文件）。
    #[serde(default)]
    pub turn_cost_ledger: TurnCostLedger,
    /// G10: 最近一次回合提交诊断摘要（stop_turn 或回合完成时写入；serde default 兼容旧会话文件）。
    #[serde(default)]
    pub last_turn_diagnostic: Option<TurnDiagnostic>,
    /// Scribe 低语（吞噬 loreweaver scribe 口径）：上回合事件提取报的可检定行动，
    /// 本回合 prompt 注入为建议（KP 自由采纳，非强制）。serde default 兼容旧会话。
    #[serde(default)]
    pub scribe_whispers: Vec<String>,
    /// X3 (吞噬自 xiami skimming.rs): 最近一次正文定稿后的速读质检问题（诊断展示用）。
    #[serde(default)]
    pub skim_issues: Vec<SkimIssue>,
    /// X3: 最近一次速读质检的正文摘录（前 200 字，展示用）。
    #[serde(default)]
    pub skim_sample: String,
    /// [morphling Wave B3 2026-08-16] 章节剧情摘要账本（吸收自 SillyTavern-BakemonoMemory
    /// summary-memory-model）：每章结束/滚动时由 LLM 提炼「本章剧情进展」，注入 system prompt
    /// 供跨章回顾；UI 可查看/编辑。默认模式 = 生成时顺带总结（C2）；fallback 提炼按
    /// diary_config 的回合/事件阈值触发。
    #[serde(default)]
    pub chapter_diaries: Vec<ChapterDiaryEntry>,
    /// [morphling C3 2026-08-16] 章节摘要 fallback 提炼配置（None = 默认 10 回合 / 20 事件）。
    #[serde(default)]
    pub diary_config: Option<ChapterDiaryConfig>,
    /// [morphling ROMA P0 2026-08-19] 当前回合执行阶段进度（崩溃/中断恢复诊断用）。
    /// 由 turn 后台 worker 在阶段边界写入；中断（流断开/服务重启/超时）时保留现场，
    /// U11 resume 据此判断「上回合死在哪一步」并记录在 resumed 诊断里（不改故事流）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_progress: Option<TurnProgress>,
}
