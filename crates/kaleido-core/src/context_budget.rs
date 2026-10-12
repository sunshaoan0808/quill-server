//! # Context Budget — 槽位化 system prompt 预算器
//!
//! 设计参考 DEEIX-Chat `application/conversation/context_assembler.go`（Go 原版
//! 仅作设计蓝本，实现自研）：把 system prompt 拆成带优先级的槽位（slot），
//! 在给定 token 预算内按优先级裁掉可选块，必选块永不裁。
//!
//! 关键纪律：
//! - **插序拼接**：保留的块按原插入顺序用 `\n\n` 拼接 —— 预算未触顶时输出与
//!   旧版逐字节一致（默认 `budget = 0` 即关闭，行为零变化）。
//! - **Base 永不裁**：基础系统提示（世界观/人物/硬规则）是 required，预算再紧也保留；
//!   因此最终长度在极端情况下可能略超预算——这是有意取舍（宁可超也不丢地基）。
//! - **裁序 = 优先级升序，同优先级后插先裁**：越晚拼进去的块越先被裁。
//!
//! token 估算复用 `memory_weaver::estimate_tokens`（中文 3.5 字符/token、
//! 英文 4 字符/token 的快速估算），与记忆压缩同一把尺子。
//!
//! 零依赖、纯函数，可脱离服务器单测。

use serde::{Deserialize, Serialize};

/// 槽位类型 —— 与 `build_turn_system_prompt` 的注入块一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SlotKind {
    /// 基础系统提示（`build_tavern_system_prompt` 整体）—— required，永不裁。
    Base,
    /// 写作风格指南（参考库注入）。
    StyleGuide,
    /// 吞噬资产块（账本/情感曲线/角色弧/关系演化）。
    AssetBlock,
    /// 原著剖析大纲片段（outline 牵引）。
    OutlineClip,
    /// 导演计划指令（ST-26）。
    DirectorInstruction,
    /// S7 语义召回的旧剧情片段。
    RecallNote,
    /// 交互式组件目录（吞噬 DEEIX uicomponent 的 `<catalog>` 注入）。
    UiCatalog,
}

impl SlotKind {
    /// 保留优先级：越大越先保留。Base 走 `is_required`，此值仅作排序兜底。
    pub fn priority(self) -> u8 {
        match self {
            Self::Base => 100,
            Self::DirectorInstruction => 90,
            Self::RecallNote => 80,
            Self::StyleGuide => 70,
            Self::UiCatalog => 65,
            Self::OutlineClip => 60,
            Self::AssetBlock => 50,
        }
    }

    /// required 槽位（永不裁）。
    pub fn is_required(self) -> bool {
        matches!(self, Self::Base)
    }
}

/// 一个待组装的 system prompt 块。
#[derive(Debug, Clone)]
pub struct PromptSlot {
    pub kind: SlotKind,
    pub content: String,
}

impl PromptSlot {
    pub fn new(kind: SlotKind, content: impl Into<String>) -> Self {
        Self { kind, content: content.into() }
    }
}

/// 组装结果的可观察痕迹（日志/测试用）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AssemblyTrace {
    /// 预算（0 = 关闭）。
    pub budget_tokens: usize,
    /// 组装前的估算总 token（含 Base）。
    pub total_tokens: usize,
    /// 组装后实际保留的估算 token。
    pub kept_tokens: usize,
    /// 被裁掉的槽位（按裁切顺序）。
    pub trimmed: Vec<SlotKind>,
}

impl AssemblyTrace {
    /// 是否发生过裁切（用于调用方打 warn 日志）。
    pub fn had_trims(&self) -> bool {
        !self.trimmed.is_empty()
    }
}

/// 预算关闭的约定值。
pub const BUDGET_OFF: usize = 0;

/// 组装 system prompt：`base` + `blocks`，可选 token 预算内按优先级裁剪。
///
/// - `budget_tokens == 0`：不裁，输出 = `base` + 逐块 `\n\n` 拼接（旧版行为）。
/// - `budget_tokens > 0` 且总估算未超：同上，零裁剪。
/// - 超预算：按优先级从低到高裁可选块（同优先级后插先裁），直到 `kept_tokens <= budget`；
///   Base 永不裁，极端情况下结果可能略超预算。
/// - 空白块一律跳过（与旧版 `if !x.trim().is_empty()` 守卫一致）。
pub fn assemble_system_prompt(
    base: &str,
    blocks: &[PromptSlot],
    budget_tokens: usize,
) -> (String, AssemblyTrace) {
    let base_tokens = crate::memory_weaver::estimate_tokens(base);
    // 过滤空白块，并保留插入序索引
    let kept_slots: Vec<&PromptSlot> = blocks
        .iter()
        .filter(|s| !s.content.trim().is_empty())
        .collect();

    let slot_tokens: Vec<usize> = kept_slots
        .iter()
        .map(|s| crate::memory_weaver::estimate_tokens(&s.content))
        .collect();
    let total_tokens = base_tokens + slot_tokens.iter().sum::<usize>();

    let mut trace = AssemblyTrace {
        budget_tokens,
        total_tokens,
        kept_tokens: total_tokens,
        trimmed: Vec::new(),
    };

    let join_parts = |parts: Vec<String>| parts.join("\n\n");

    if budget_tokens == BUDGET_OFF || total_tokens <= budget_tokens {
        let mut parts = Vec::with_capacity(kept_slots.len() + 1);
        parts.push(base.to_string());
        for s in &kept_slots {
            parts.push(s.content.clone());
        }
        return (join_parts(parts), trace);
    }

    // 超预算：裁切序列 = 可选块按 (优先级升序, 插入序降序)
    let mut cut_order: Vec<usize> = (0..kept_slots.len())
        .filter(|&i| !kept_slots[i].kind.is_required())
        .collect();
    cut_order.sort_by_key(|&i| {
        let kind = kept_slots[i].kind;
        // 优先级升序（低者先裁）；同优先级插入序降序（后插先裁）
        (kind.priority(), std::cmp::Reverse(i))
    });

    let mut keep = vec![true; kept_slots.len()];
    let mut kept_tokens = total_tokens;
    for &i in &cut_order {
        if kept_tokens <= budget_tokens {
            break;
        }
        kept_tokens -= slot_tokens[i];
        keep[i] = false;
        trace.trimmed.push(kept_slots[i].kind);
    }
    trace.kept_tokens = kept_tokens;

    let mut parts = Vec::with_capacity(kept_slots.len() + 1);
    parts.push(base.to_string());
    for (i, s) in kept_slots.iter().enumerate() {
        if keep[i] {
            parts.push(s.content.clone());
        }
    }
    (join_parts(parts), trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> String {
        "地基".repeat(50)
    }

    #[test]
    fn budget_off_output_is_byte_identical_to_legacy_concat() {
        let base = base();
        let blocks = vec![
            PromptSlot::new(SlotKind::StyleGuide, "风格指南块"),
            PromptSlot::new(SlotKind::AssetBlock, "资产块"),
            PromptSlot::new(SlotKind::OutlineClip, "大纲片段"),
        ];
        let expected = format!(
            "{}\n\n风格指南块\n\n资产块\n\n大纲片段",
            base
        );
        let (out, trace) = assemble_system_prompt(&base, &blocks, BUDGET_OFF);
        assert_eq!(out, expected);
        assert!(trace.trimmed.is_empty());
        assert_eq!(trace.budget_tokens, BUDGET_OFF);
    }

    #[test]
    fn under_budget_no_trims_same_order() {
        let base = base();
        let blocks = vec![
            PromptSlot::new(SlotKind::DirectorInstruction, "导演指令"),
            PromptSlot::new(SlotKind::RecallNote, "召回片段"),
        ];
        let (out, trace) = assemble_system_prompt(&base, &blocks, 100_000);
        assert_eq!(out, format!("{}\n\n导演指令\n\n召回片段", base));
        assert!(trace.trimmed.is_empty());
        assert_eq!(trace.kept_tokens, trace.total_tokens);
    }

    #[test]
    fn over_budget_trims_lowest_priority_first() {
        // 预算只够 base + 一个块 → 最低优先级的 AssetBlock 先被裁
        let base = base();
        let base_tokens = crate::memory_weaver::estimate_tokens(&base);
        let blocks = vec![
            PromptSlot::new(SlotKind::AssetBlock, "资产块"),
            PromptSlot::new(SlotKind::OutlineClip, "大纲片段"),
            PromptSlot::new(SlotKind::StyleGuide, "风格指南"),
            PromptSlot::new(SlotKind::RecallNote, "召回"),
            PromptSlot::new(SlotKind::DirectorInstruction, "导演指令"),
        ];
        let budget = base_tokens + crate::memory_weaver::estimate_tokens("导演指令")
            + crate::memory_weaver::estimate_tokens("召回");
        let (out, trace) = assemble_system_prompt(&base, &blocks, budget);

        // 裁切顺序：AssetBlock → OutlineClip → StyleGuide（裁到预算内为止）
        assert_eq!(
            trace.trimmed,
            vec![SlotKind::AssetBlock, SlotKind::OutlineClip, SlotKind::StyleGuide]
        );
        // 保留：base + 召回 + 导演指令，顺序保持插入序
        assert_eq!(out, format!("{}\n\n召回\n\n导演指令", base));
        assert!(trace.kept_tokens <= budget);
        assert!(trace.had_trims());
    }

    #[test]
    fn required_base_survives_impossible_budget() {
        let base = base();
        let blocks = vec![PromptSlot::new(SlotKind::StyleGuide, "风格")];
        // 预算小于 base 本身 → 仍不裁 base，只裁可选块
        let (out, trace) = assemble_system_prompt(&base, &blocks, 1);
        assert!(out.starts_with(&base));
        assert_eq!(trace.trimmed, vec![SlotKind::StyleGuide]);
        // 文档化取舍：required 永不裁，kept 可能 > budget
        assert!(trace.kept_tokens > 1);
    }

    #[test]
    fn blank_blocks_skipped() {
        let base = base();
        let blocks = vec![
            PromptSlot::new(SlotKind::StyleGuide, ""),
            PromptSlot::new(SlotKind::AssetBlock, "   \n  "),
            PromptSlot::new(SlotKind::OutlineClip, "有效块"),
        ];
        let (out, _) = assemble_system_prompt(&base, &blocks, 0);
        assert_eq!(out, format!("{}\n\n有效块", base));
    }
}
