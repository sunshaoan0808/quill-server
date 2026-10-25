//! # SillyTavern 预设兼容引擎
//!
//! 独立模块，负责解析和组装 SillyTavern 预设（prompt builder）JSON。
//!
//! ## 架构
//! ```text
//! PresetAssembler
//!   ├── MacroEngine          ← 变量宏渲染（setvar/addvar/getvar/getglobalvar）
//!   ├── PresetBlock          ← 单个 prompt 块
//!   ├── assemble()           ← 双遍渲染 + 注入位置排序 → Vec<ChatMessage>
//!   └── compress_adjacent()  ← STJS「压缩相邻消息」+ LoRA 注入
//! ```
//!
//! 不依赖 main.rs，纯函数式，可独立单测。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── 数据结构 ───────────────────────────────────────────────────────────────

/// 一条组装好的聊天消息，可直接序列化为 OpenAI messages 数组。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// SillyTavern 预设中的一个 prompt 块。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetBlock {
    /// 块唯一 ID（ST 用 identifier，兼容 id 字段）
    #[serde(default)]
    pub identifier: String,
    /// 块名称（如"梦境思客"、"梦白话文风"）
    #[serde(default)]
    pub name: String,
    /// 原始内容（含未渲染宏）
    #[serde(default)]
    pub content: String,
    /// 是否启用
    #[serde(default)]
    pub enabled: bool,
    /// 注入位置：-1 = 系统槽位（main/aux/post-history），0 = 聊天历史内注入
    #[serde(default, rename = "injection_position")]
    pub injection_position: i32,
    /// 注入深度：从末尾数第 N 条消息处插入（0 = 最末尾）
    #[serde(default, rename = "injection_depth")]
    pub injection_depth: u32,
    /// 同位置内的排序（升序）
    #[serde(default, rename = "injection_order")]
    pub injection_order: i32,
    /// 消息角色：system / user / assistant
    #[serde(default)]
    pub role: String,
    /// 是否标记为系统提示（ST 内部字段）
    #[serde(default)]
    pub system_prompt: bool,
    /// 命名标记（Main Prompt / Auxiliary / Post-History 等）
    #[serde(default)]
    pub marker: bool,
    // ── ST 块级字段（对齐 PromptManager.js:182-199），camel/snake 双读 ──
    /// 禁止被覆写：标记该块为关键 system prompt，下游不得替换其内容
    #[serde(default, rename = "forbid_overrides", alias = "forbidOverrides")]
    pub forbid_overrides: bool,
    /// 把块挂到特定消息角色（而非按 injection_depth）
    #[serde(default, rename = "attach_role", alias = "attachRole")]
    pub attach_role: String,
    /// 挂载索引：目标消息在历史中的序号（仅当 `attach_role` 非空时有意义）
    #[serde(default, rename = "attach_index", alias = "attachIndex")]
    pub attach_index: u32,
    /// 挂载侧：above / below（仅当 `attach_role` 非空时有意义）
    #[serde(default, rename = "attach_side", alias = "attachSide")]
    pub attach_side: String,
    /// 注入触发条件："" / continue / impersonate
    #[serde(default, rename = "injection_trigger", alias = "injectionTrigger")]
    pub injection_trigger: String,
    /// 块归属扩展（ST 的扩展插件标记）
    #[serde(default)]
    pub extension: String,
}

/// 宏引擎运行时上下文。
#[derive(Debug, Clone, Default)]
pub struct MacroContext {
    /// 会话变量（setvar/addvar 操作的目标）
    pub vars: HashMap<String, String>,
    /// 全局变量（跨会话持久）
    pub global_vars: HashMap<String, String>,
    /// 当前用户名（{{user}}）
    pub user_name: String,
    /// 当前角色名（{{char}}）
    pub char_name: String,
    /// 最近一条用户消息（{{lastUserMessage}}）
    pub last_user_message: String,
    /// 记忆字符串（{{MEMORY}}）
    pub memory: String,
}

impl MacroContext {
    pub fn new(user_name: &str, char_name: &str) -> Self {
        Self {
            user_name: user_name.to_string(),
            char_name: char_name.to_string(),
            ..Default::default()
        }
    }
}

// ─── 宏引擎 ─────────────────────────────────────────────────────────────────

/// 渲染一段文本中的所有 SillyTavern 宏。
///
/// # 宏支持
/// - `{{setvar::name::value}}` — 设置变量（value 内可嵌套其他宏），输出空串
/// - `{{addvar::name::value}}` — 追加变量（换行分隔），输出空串
/// - `{{getvar::name}}` — 读取变量并递归渲染
/// - `{{getglobalvar::name}}` — 读取全局变量
/// - `{{if_contains::haystack::needle::then::else?}}` — 条件宏：`haystack` 含有
///   `needle` 时输出 `then`，否则输出 `else`（省略则空串）
/// - `{{if_equals::a::b::then::else?}}` — 条件宏：`a` 等于 `b` 时输出 `then`，
///   否则输出 `else`（省略则空串）
/// - `{{lastUserMessage}}` — 最近用户消息
/// - `{{user}}` / `{{char}}` / `{{charIfNotGroup}}` — 角色名
/// - `{{MEMORY}}` — 记忆
/// - `{{//comment}}` — 注释，输出空串
/// - `{{trim}}` — 标记裁剪（当前实现为去除块首尾空白）
/// - `{{压缩相邻消息::lora_*}}` — STJS 插件宏，在消息组装阶段处理
///
/// # 算法
/// 按字符扫描，遇到 `{{` 时用括号深度计数找到匹配的 `}}`，
/// 正确处理嵌套宏（setvar 值内含 getvar）。
///
/// # DoS 防护
/// 用户可写的 preset 可能包含自指宏（如 `{{setvar::a::{{getvar::a}}}}`），
/// 展开时会无限递归。宏展开受两个上限约束：
/// - 递归深度 ≤ [`MAX_MACRO_DEPTH`]，超限时该宏**原样保留**（与未知宏语义一致）
/// - 输出字节 ≤ [`MAX_MACRO_OUTPUT_BYTES`]，超限时输出被截断
///
/// 两者都只降级（保留/截断）不报错，保持既有 `render_macros -> String` 签名。
pub fn render_macros(input: &str, ctx: &mut MacroContext) -> String {
    let mut budget = MAX_MACRO_OUTPUT_BYTES;
    render_macros_bounded(input, ctx, 0, &mut budget)
}

/// 宏递归展开的最大嵌套深度（防御自指宏无限递归爆栈）。
pub const MAX_MACRO_DEPTH: usize = 16;

/// 宏展开输出的最大字节数（防御 addvar 累乘式膨胀）。
pub const MAX_MACRO_OUTPUT_BYTES: usize = 256 * 1024;

/// 按 UTF-8 字符边界安全截断到最多 `max` 字节。
fn truncate_at_boundary(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut idx = max;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    &s[..idx]
}

/// 把 `s` 追加到 `out`，受共享字节预算 `budget` 约束。
/// 返回是否已触发截断（预算耗尽）。
fn push_within_budget(out: &mut String, s: &str, budget: &mut usize) -> bool {
    if s.len() > *budget {
        out.push_str(truncate_at_boundary(s, *budget));
        *budget = 0;
        true
    } else {
        out.push_str(s);
        *budget -= s.len();
        false
    }
}

/// 宏展开主体：`depth` 为当前嵌套层数（顶层为 0），`budget` 为跨递归共享的输出字节预算。
fn render_macros_bounded(
    input: &str,
    ctx: &mut MacroContext,
    depth: usize,
    budget: &mut usize,
) -> String {
    let mut output = String::with_capacity(input.len().min(*budget));
    let mut rest = input;
    let mut truncated = false;

    while let Some(start) = rest.find("{{") {
        // 输出 {{ 之前的文本
        truncated |= push_within_budget(&mut output, &rest[..start], budget);
        if truncated {
            return finalize_output(&output, ctx);
        }

        // 找到匹配的 }}，处理嵌套
        let after_open = &rest[start + 2..];
        let close = match find_matching_close(after_open) {
            Some(c) => c,
            None => {
                // 没有匹配的 }}，原样输出剩余
                push_within_budget(&mut output, &rest[start..], budget);
                return finalize_output(&output, ctx);
            }
        };

        let macro_body = &after_open[..close];
        rest = &after_open[close + 2..]; // 跳过 "}}"

        // 处理这个宏
        let expanded = process_macro(macro_body, ctx, depth, budget);
        truncated |= push_within_budget(&mut output, &expanded, budget);
        if truncated {
            return finalize_output(&output, ctx);
        }
    }

    push_within_budget(&mut output, rest, budget);
    finalize_output(&output, ctx)
}

/// 找到第一个顶层 `}}` 的位置（考虑嵌套 `{{...}}`）。
fn find_matching_close(s: &str) -> Option<usize> {
    let mut depth: u32 = 1;
    let mut chars = s.char_indices().peekable();

    while let Some((i, ch)) = chars.next() {
        if ch == '{' {
            if chars.peek().map(|(_, c)| *c) == Some('{') {
                depth += 1;
                chars.next();
            }
        } else if ch == '}' {
            if chars.peek().map(|(_, c)| *c) == Some('}') {
                depth -= 1;
                chars.next();
                if depth == 0 {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// 处理单个宏的内容（不含外层 `{{}}`）。
///
/// `depth` 为当前嵌套层数；超过 [`MAX_MACRO_DEPTH`] 时原样返回宏文本
/// （与未知宏同一语义），从而让自指宏安全降级而不是爆栈。
/// `budget` 与外层共享，保证整棵展开树的输出总量有界。
fn process_macro(
    body: &str,
    ctx: &mut MacroContext,
    depth: usize,
    budget: &mut usize,
) -> String {
    if depth > MAX_MACRO_DEPTH {
        return format!("{{{{{}}}}}", body);
    }
    let depth = depth + 1;
    let body = body.trim();

    // 按 :: 分割，但只分割前 2-3 段
    let parts: Vec<&str> = split_macro_parts(body);
    // 条件宏需要完整的顶层参数表（`::` 可出现在 then/else 内）。
    let parts_all: Vec<&str> = split_top_level(body);

    match parts.first().copied().unwrap_or("") {
        "setvar" if parts.len() >= 3 => {
            let var_name = parts[1].trim();
            let raw_value = parts[2..].join("::");
            // 值内可能含嵌套宏，先渲染
            let rendered = render_macros_bounded(&raw_value, ctx, depth, budget);
            ctx.vars.insert(var_name.to_string(), rendered);
            String::new()
        }
        "addvar" if parts.len() >= 3 => {
            let var_name = parts[1].trim();
            let raw_value = parts[2..].join("::");
            let rendered = render_macros_bounded(&raw_value, ctx, depth, budget);
            let entry = ctx.vars.entry(var_name.to_string()).or_default();
            if !entry.is_empty() && !rendered.trim().is_empty() {
                entry.push('\n');
            }
            entry.push_str(&rendered);
            String::new()
        }
        "getvar" if parts.len() >= 2 => {
            let var_name = parts[1].trim();
            let val = ctx.vars.get(var_name).cloned().unwrap_or_default();
            // 变量值内可能还含宏（递归渲染）
            render_macros_bounded(&val, ctx, depth, budget)
        }
        "getglobalvar" if parts.len() >= 2 => {
            let var_name = parts[1].trim();
            // 全局变量值同样内嵌宏，保持与 getvar 一致的深度/预算传递
            let val = ctx.global_vars.get(var_name).cloned().unwrap_or_default();
            render_macros_bounded(&val, ctx, depth, budget)
        }
        "if_contains" | "if_equals" if parts_all.len() >= 4 => {
            // 条件宏：`if_contains::haystack::needle::then[::else]`
            //           `if_equals::a::b::then[::else]`
            // 三/四/五段参数用顶层 `::` 切（不能复用只切前两段的
            // split_macro_parts，那会把 then/else 里的 `::` 一起吞掉）。
            let kind = parts_all[0];
            let hay = render_macros_bounded(parts_all[1], ctx, depth, budget);
            let needle = render_macros_bounded(parts_all[2], ctx, depth, budget);
            let hit = if kind == "if_contains" {
                !needle.is_empty() && hay.contains(&needle)
            } else {
                hay == needle
            };
            let branch = if hit { Some(parts_all[3]) } else { parts_all.get(4).copied() };
            match branch {
                Some(b) => render_macros_bounded(b, ctx, depth, budget),
                // 省略 else 分支 ⇒ 空串（调用方可据此产出 Continue）。
                None => String::new(),
            }
        }
        "lastUserMessage" => ctx.last_user_message.clone(),
        "user" | "User" | "USER" => ctx.user_name.clone(),
        "char" | "Char" | "CHAR" => ctx.char_name.clone(),
        "charIfNotGroup" => ctx.char_name.clone(),
        "MEMORY" => ctx.memory.clone(),
        "trim" => {
            // 标记：由 finalize_output 处理（在块级别 trim）
            String::new()
        }
        "setglobalvar" if parts.len() >= 3 => {
            let var_name = parts[1].trim();
            let raw_value = parts[2..].join("::");
            let rendered = render_macros_bounded(&raw_value, ctx, depth, budget);
            ctx.global_vars.insert(var_name.to_string(), rendered);
            String::new()
        }
        "addglobalvar" if parts.len() >= 3 => {
            let var_name = parts[1].trim();
            let raw_value = parts[2..].join("::");
            let rendered = render_macros_bounded(&raw_value, ctx, depth, budget);
            let entry = ctx.global_vars.entry(var_name.to_string()).or_default();
            if !entry.is_empty() && !rendered.trim().is_empty() {
                entry.push('\n');
            }
            entry.push_str(&rendered);
            String::new()
        }
        other if other.starts_with("//") => {
            // {{// comment}} → 空
            String::new()
        }
        other if other.starts_with("压缩相邻消息") => {
            // STJS 插件宏，在消息后处理阶段处理；文本渲染阶段保留标记
            // 返回空串（不产生可见文本），实际压缩在 compress_adjacent_messages 中做
            String::new()
        }
        _ => {
            // 未知宏：原样保留（避免吞内容）
            format!("{{{{{}}}}}", body)
        }
    }
}

/// 按 `::` 分割宏参数，但保留值内的 `::`。
/// `setvar::name::value::with::colons` → `["setvar", "name", "value::with::colons"]`
fn split_macro_parts(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut last = 0;
    let mut found = 0;

    for (i, _) in body.match_indices("::") {
        if found < 2 {
            parts.push(&body[last..i]);
            last = i + 2;
            found += 1;
        }
    }
    parts.push(&body[last..]);
    parts
}

/// 按**顶层** `::` 分割宏体：嵌套 `{{...}}` 内部的 `::` 不算分隔符。
///
/// 条件宏（`if_contains` / `if_equals`）需要保留 then/else 分支里的 `::`：
/// `if_contains::a::b::{{getvar::c}}::none`
///   → `["if_contains", "a", "b", "{{getvar::c}}", "none"]`
fn split_top_level(body: &str) -> Vec<&str> {
    let bytes = body.as_bytes();
    let mut parts = Vec::new();
    let mut last = 0usize;
    let mut i = 0usize;
    let mut depth = 0u32;
    while i < bytes.len() {
        if depth == 0 && body[i..].starts_with("::") {
            parts.push(&body[last..i]);
            i += 2;
            last = i;
            continue;
        }
        if body[i..].starts_with("{{") {
            depth += 1;
            i += 2;
            continue;
        }
        if body[i..].starts_with("}}") && depth > 0 {
            depth -= 1;
            i += 2;
            continue;
        }
        // 按字符边界推进，避免切在多字节字符中间。
        i += body[i..].chars().next().map(char::len_utf8).unwrap_or(1);
    }
    parts.push(&body[last..]);
    parts
}

/// 输出后处理：处理 `{{trim}}` 的语义（裁剪块首尾空白）+ ST 尖括号角色名替换。
fn finalize_output(output: &str, ctx: &MacroContext) -> String {
    // trim 标记在 process_macro 中已删除，这里统一 trim 首尾空白
    let trimmed = output.trim().to_string();
    // ST 同时支持 <USER>/<BOT>/<CHAR> 尖括号格式（大小写不敏感）
    substitute_angle_names(&trimmed, &ctx.user_name, &ctx.char_name)
}

/// 替换 `<user>`/`<char>`/`<bot>` 尖括号角色名（ST 兼容）。
fn substitute_angle_names(input: &str, user: &str, char: &str) -> String {
    // 简单大小写不敏感替换（避免引入 regex 依赖）
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            // 尝试匹配 <user> / <char> / <bot>
            let rest = &input[i + 1..];
            let lower = rest.to_lowercase();
            if let Some(rep) = if lower.starts_with("user>") {
                Some((5, user))
            } else if lower.starts_with("char>") || lower.starts_with("bot>") {
                Some((5, char))
            } else {
                None
            } {
                out.push_str(rep.1);
                i += 1 + rep.0;
                continue;
            }
        }
        // 安全按 UTF-8 字符推进
        let ch_len = input[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        out.push_str(&input[i..i + ch_len]);
        i += ch_len;
    }
    out
}

// ─── 预设组装器 ─────────────────────────────────────────────────────────────

/// 预设组装结果。
#[derive(Debug, Clone)]
pub struct AssemblyResult {
    /// 最终消息列表（可直接发给 OpenAI 兼容 API）
    pub messages: Vec<ChatMessage>,
    /// 渲染后的变量表（调试/审计用）
    pub vars: HashMap<String, String>,
    /// 渲染后的全局变量
    pub global_vars: HashMap<String, String>,
    /// 系统槽位内容（Main Prompt / Post-History 等）
    pub system_pre: String,
    /// Post-history 系统内容
    pub system_post: String,
}

/// 从 SillyTavern 预设 JSON 解析块列表。
pub fn parse_preset_blocks(json_str: &str) -> Result<Vec<PresetBlock>, serde_json::Error> {
    let v: serde_json::Value = serde_json::from_str(json_str)?;
    let prompts = v
        .get("prompts")
        .and_then(|p| p.as_array())
        .cloned()
        .unwrap_or_default();

    let blocks: Vec<PresetBlock> = prompts
        .into_iter()
        .filter_map(|p| serde_json::from_value(p).ok())
        .collect();

    Ok(blocks)
}

/// 组装预设为消息列表。
///
/// # 流程
/// 1. 筛选 enabled 块
/// 2. 按 `(injection_position, injection_order)` 稳定排序
/// 3. **第一遍**：逐块渲染宏，setvar/addvar 修改变量表（不输出到消息）
/// 4. **第二遍**：重新渲染所有块，此时 getvar 已有值，收集可见内容
/// 5. 按 injection_position/depth 分配到消息槽位
/// 6. 压缩相邻同角色消息（STJS 压缩相邻消息）
pub fn assemble(
    blocks: &[PresetBlock],
    ctx: &mut MacroContext,
    chat_history: &[ChatMessage],
    lora_constant: Option<&str>,
) -> AssemblyResult {
    // 1. 筛选 + 排序
    let mut enabled: Vec<&PresetBlock> = blocks.iter().filter(|b| b.enabled).collect();
    enabled.sort_by_key(|b| (b.injection_position, b.injection_order));

    // 2. 第一遍：执行 setvar/addvar（使用块的原始内容渲染，填充变量表）
    for block in &enabled {
        let _ = render_macros(&block.content, ctx);
    }

    // 3. 第二遍：重新渲染，收集可见输出
    //    需要重新创建一个临时 ctx 副本吗？不——变量已在第一遍填充。
    //    但 setvar 输出空串，getvar 输出值，所以第二遍会得到可见内容。
    //    问题：第一遍已经改变了 ctx.vars。第二遍 setvar 会再次设置相同值（幂等），
    //    getvar 会正确输出。所以直接再跑一遍即可。
    let mut system_pre_parts: Vec<String> = Vec::new();
    let mut system_post_parts: Vec<String> = Vec::new();
    let mut in_chat_blocks: Vec<(u32, String, String)> = Vec::new(); // (depth, role, content)

    for block in &enabled {
        let rendered = render_macros(&block.content, ctx);
        if rendered.trim().is_empty() {
            continue;
        }

        if block.injection_position < 0 {
            // 系统槽位
            match block.name.as_str() {
                "Post-History Instructions" => system_post_parts.push(rendered),
                _ => system_pre_parts.push(rendered),
            }
        } else {
            // 聊天历史内注入
            let role = if block.role.is_empty() {
                "system".to_string()
            } else {
                block.role.clone()
            };
            in_chat_blocks.push((block.injection_depth, role, rendered));
        }
    }

    let system_pre = system_pre_parts.join("\n\n");
    let system_post = system_post_parts.join("\n\n");

    // 4. 构建消息列表
    let mut messages: Vec<ChatMessage> = Vec::new();

    // 4a. 系统前缀（Main Prompt + char card + enhance definitions）
    if !system_pre.is_empty() {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_pre.clone(),
        });
    }

    // 4b. 聊天历史 + in-chat 注入
    //     in_chat_blocks 按 depth 从大到小插入（depth=4 比 depth=0 更早插入）
    let history_len = chat_history.len() as i64;
    let mut working: Vec<ChatMessage> = chat_history.to_vec();

    // 按 depth 降序排列（先插深的，再插浅的，避免索引偏移）
    let mut sorted_injects = in_chat_blocks;
    sorted_injects.sort_by(|a, b| b.0.cmp(&a.0));

    for (depth, role, content) in sorted_injects {
        let insert_pos = if depth == 0 {
            working.len()
        } else {
            let pos = history_len - depth as i64;
            if pos < 0 {
                0
            } else {
                pos as usize
            }
        };
        let insert_pos = insert_pos.min(working.len());
        working.insert(
            insert_pos,
            ChatMessage {
                role,
                content,
            },
        );
    }

    // 4c. LoRA 常量注入（STJS 压缩相邻消息的 lora_constant）
    if let Some(constant) = lora_constant {
        if !constant.trim().is_empty() {
            working.insert(
                0,
                ChatMessage {
                    role: "system".to_string(),
                    content: constant.trim().to_string(),
                },
            );
        }
    }

    messages.extend(working);

    // 4d. Post-history 系统指令
    if !system_post.is_empty() {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: system_post.clone(),
        });
    }

    // 5. 压缩相邻同角色消息
    let messages = compress_adjacent_messages(&messages);

    AssemblyResult {
        messages,
        vars: ctx.vars.clone(),
        global_vars: ctx.global_vars.clone(),
        system_pre,
        system_post,
    }
}

/// 压缩相邻同角色消息（STJS「压缩相邻消息」插件核心逻辑）。
///
/// 将连续相同 role 的消息合并为一条，用换行分隔。
/// 同时在每条用户消息前注入 lora_key 边界标记（如果提供）。
pub fn compress_adjacent_messages(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    if messages.is_empty() {
        return Vec::new();
    }

    let mut result: Vec<ChatMessage> = Vec::new();

    for msg in messages {
        if let Some(last) = result.last_mut() {
            if last.role == msg.role {
                // 合并
                if !last.content.is_empty() {
                    last.content.push('\n');
                }
                last.content.push_str(&msg.content);
                continue;
            }
        }
        result.push(msg.clone());
    }

    result
}

// ─── 跨 provider 类型默认回填 ────────────────────────────────────────────────

/// 预设 settings 里「按 provider 类型分槽」的连接来源。
///
/// ST/TT 生态里一份预设 JSON 会同时携带多个来源的连接键
/// （`openai_model` / `claude_model` / `google_model` / `azure_deployment_name` …），
/// 但同一时刻只有当前来源的键有意义。切换来源时必须先把旧来源的键清干净，
/// 再把新来源的键回填进去，否则上一次来源的残留会跟着发出去。
///
/// 键名沿用 ST 既有命名（`google_model` 对应 Makersuite），
/// 使 museai 导出的 settings 可被 ST/TT 直接识别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptSource {
    /// OpenAI 兼容 chat completions（`openai_model`）
    OpenAi,
    /// Anthropic Claude Messages（`claude_model`）
    Claude,
    /// Google Gemini（`google_model`）
    Google,
    /// 自定义 / OpenAI 兼容第三方端点（`custom_model`）
    Custom,
}

impl PromptSource {
    /// 全部来源，顺序即「清键」时的遍历顺序。
    pub const ALL: &'static [Self] = &[Self::OpenAi, Self::Claude, Self::Google, Self::Custom];

    /// 解析来源标识。接受 `ProviderKind` 的 camelCase 序列化形式
    /// （`openAI` / `anthropic` / `google`）以及 ST 的 `chat_completion_source` 值。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "" | "openai" | "open_ai" | "openai_compat" | "openai_responses" => Some(Self::OpenAi),
            "anthropic" | "claude" | "claude_messages" => Some(Self::Claude),
            "google" | "gemini" | "makersuite" | "google_generative_ai" => Some(Self::Google),
            "custom" | "openrouter" | "open_router" | "deepseek" | "groq" | "cohere"
            | "moonshot" | "siliconflow" | "xai" | "zhipu" | "minimax" | "pollinations" => {
                Some(Self::Custom)
            }
            _ => None,
        }
    }

    /// 该来源在预设 settings 里的模型名键（ST 命名）。
    pub const fn model_setting_key(self) -> &'static str {
        match self {
            Self::OpenAi => "openai_model",
            Self::Claude => "claude_model",
            Self::Google => "google_model",
            Self::Custom => "custom_model",
        }
    }

    /// 该来源的 ST `chat_completion_source` 字符串值。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Claude => "claude",
            Self::Google => "makersuite",
            Self::Custom => "custom",
        }
    }
}

/// 与来源无关的连接键（TT `CONNECTION_PAYLOAD_KEYS` 对应子集）。
pub const CONNECTION_SETTING_KEYS: &[&str] = &[
    "chat_completion_source",
    "custom_api_format",
    "model",
    "custom_url",
    "reverse_proxy",
    "proxy_password",
    "custom_prompt_post_processing",
    "custom_include_headers",
    "custom_include_body",
    "custom_exclude_body",
    "custom_claude_prompt_caching",
];

/// 按来源分槽的连接键（TT `PROMPT_CONNECTION_EXTRA_SETTING_KEYS` + source-specific 键）。
pub const SOURCE_SPECIFIC_SETTING_KEYS: &[(&str, PromptSource)] = &[
    ("openrouter_use_fallback", PromptSource::Custom),
    ("openrouter_providers", PromptSource::Custom),
    ("openrouter_quantizations", PromptSource::Custom),
    ("openrouter_allow_fallbacks", PromptSource::Custom),
    ("openrouter_middleout", PromptSource::Custom),
    ("azure_base_url", PromptSource::OpenAi),
    ("azure_deployment_name", PromptSource::OpenAi),
    ("azure_api_version", PromptSource::OpenAi),
    ("gemini_api_version", PromptSource::Google),
    ("vertexai_endpoint", PromptSource::Google),
    ("additional_parameters_by_source", PromptSource::Custom),
];

/// 全部「连接相关」键 = 来源无关键 + 各来源模型名键 + 按来源分槽键。
pub fn connection_setting_keys() -> impl Iterator<Item = &'static str> {
    CONNECTION_SETTING_KEYS
        .iter()
        .copied()
        .chain(PromptSource::ALL.iter().map(|s| s.model_setting_key()))
        .chain(SOURCE_SPECIFIC_SETTING_KEYS.iter().map(|(k, _)| *k))
}

/// 把 `defaults`（当前连接快照 / 默认值）按 provider 类型回填进 `settings`。
///
/// 语义与 TauriTavern `apply_current_model_connection_snapshot_to_prompt_settings` 一致：
/// **先清后填** —— 先移除 `settings` 中所有已知的连接键（跨来源残留一并清掉），
/// 再从 `defaults` 中把已知键写回，最后盖上 `chat_completion_source` 标记。
///
/// 与 TT 的差异：TT 直接把快照里命中的键原样插入，本实现在插入前先按白名单过滤，
/// 避免快照里的任意字段渗进预设 settings（预设是用户资产，不该被连接配置污染）。
///
/// 未在快照中出现的键不会被凭空造出来 —— 缺失即缺失，由调用方决定是否再走
/// 「恢复默认预设」路径。
///
/// # Errors
/// `settings` 不是 JSON 对象，或 `source` 无法识别时返回 `Err`。
pub fn apply_source_scoped_backfill(
    settings: &mut serde_json::Value,
    source: &str,
    defaults: &serde_json::Value,
) -> Result<(), String> {
    let src = PromptSource::parse(source)
        .ok_or_else(|| format!("prompt_source_unsupported: unknown source `{source}`"))?;
    let object = settings
        .as_object_mut()
        .ok_or_else(|| "preset_settings_invalid: preset settings must be a JSON object".to_string())?;

    // 先清：所有已知连接键，含其他来源的槽位。
    for key in connection_setting_keys() {
        object.remove(key);
    }

    // 再填：只接受白名单内的键。
    if let Some(defaults) = defaults.as_object() {
        for key in connection_setting_keys() {
            if let Some(value) = defaults.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
    }

    // 来源标记最后盖，保证不会被快照里的旧值覆盖。
    object.insert(
        "chat_completion_source".to_string(),
        serde_json::Value::String(src.as_str().to_string()),
    );
    Ok(())
}

// ─── 测试 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> MacroContext {
        MacroContext::new("梦客", "梦鲸思客")
    }

    #[test]
    fn test_setvar_getvar_basic() {
        let mut c = ctx();
        let out = render_macros("{{setvar::x::hello}}[{{getvar::x}}]", &mut c);
        assert_eq!(out, "[hello]");
        assert_eq!(c.vars.get("x"), Some(&"hello".to_string()));
    }

    #[test]
    fn test_addvar_appends() {
        let mut c = ctx();
        let out = render_macros(
            "{{setvar::x::aaa}}{{addvar::x::bbb}}[{{getvar::x}}]",
            &mut c,
        );
        assert_eq!(out, "[aaa\nbbb]");
    }

    #[test]
    fn test_nested_getvar_in_setvar() {
        let mut c = ctx();
        let input = "{{setvar::a::world}}{{setvar::b::hello {{getvar::a}}}}[{{getvar::b}}]";
        let out = render_macros(input, &mut c);
        assert_eq!(out, "[hello world]");
    }

    #[test]
    fn test_user_char_macros() {
        let mut c = ctx();
        let out = render_macros("{{user}}和{{char}}", &mut c);
        assert_eq!(out, "梦客和梦鲸思客");
    }

    #[test]
    fn test_comment_macro() {
        let mut c = ctx();
        let out = render_macros("可见{{//这是注释}}文本", &mut c);
        assert_eq!(out, "可见文本");
    }

    #[test]
    fn test_last_user_message() {
        let mut c = ctx();
        c.last_user_message = "你好世界".to_string();
        let out = render_macros("[{{lastUserMessage}}]", &mut c);
        assert_eq!(out, "[你好世界]");
    }

    #[test]
    fn test_memory_macro() {
        let mut c = ctx();
        c.memory = "记忆内容".to_string();
        let out = render_macros("[{{MEMORY}}]", &mut c);
        assert_eq!(out, "[记忆内容]");
    }

    #[test]
    fn test_compress_adjacent() {
        let msgs = vec![
            ChatMessage { role: "user".into(), content: "a".into() },
            ChatMessage { role: "user".into(), content: "b".into() },
            ChatMessage { role: "assistant".into(), content: "c".into() },
            ChatMessage { role: "assistant".into(), content: "d".into() },
            ChatMessage { role: "user".into(), content: "e".into() },
        ];
        let compressed = compress_adjacent_messages(&msgs);
        assert_eq!(compressed.len(), 3);
        assert_eq!(compressed[0].content, "a\nb");
        assert_eq!(compressed[1].content, "c\nd");
        assert_eq!(compressed[2].content, "e");
    }

    #[test]
    fn test_xml_schema_in_setvar() {
        // XML Schema 内含 `</xs:sequence>` 等但不含 `}}`，应正确提取
        let mut c = ctx();
        let xml = r#"<?xml version="1.0"?>
<xs:schema>
  <xs:element name="dream_plot">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="dream_body" type="xs:string"/>
      </xs:sequence>
    </xs:complexType>
  </xs:element>
</xs:schema>"#;
        let input = format!("{{{{setvar::schema::{}}}}}[{{{{getvar::schema}}}}]", xml);
        let out = render_macros(&input, &mut c);
        assert!(out.contains("dream_plot"));
        assert!(out.contains("dream_body"));
    }

    #[test]
    fn test_globalvar() {
        let mut c = ctx();
        c.global_vars.insert("g1".into(), "global_val".into());
        let out = render_macros("[{{getglobalvar::g1}}]", &mut c);
        assert_eq!(out, "[global_val]");
    }

    #[test]
    fn test_trim_macro() {
        let mut c = ctx();
        let out = render_macros("  {{trim}}  hello  ", &mut c);
        assert_eq!(out, "hello");
    }

    #[test]
    fn test_preset_parse_and_assemble() {
        let json = r#"{
            "prompts": [
                {
                    "identifier": "a",
                    "name": "Main Prompt",
                    "content": "你是{{char}}。{{setvar::style::简洁}}",
                    "enabled": true,
                    "injection_position": -1,
                    "injection_depth": 0,
                    "injection_order": 100,
                    "role": "system"
                },
                {
                    "identifier": "b",
                    "name": "写作模式",
                    "content": "风格：{{getvar::style}}。用户是{{user}}。",
                    "enabled": true,
                    "injection_position": 0,
                    "injection_depth": 0,
                    "injection_order": 200,
                    "role": "user"
                }
            ]
        }"#;
        let blocks = parse_preset_blocks(json).unwrap();
        assert_eq!(blocks.len(), 2);

        let mut c = ctx();
        let history = vec![ChatMessage {
            role: "user".into(),
            content: "你好".into(),
        }];
        let result = assemble(&blocks, &mut c, &history, None);

        // system_pre 应含 Main Prompt
        assert!(result.system_pre.contains("你是梦鲸思客"));
        // 写作模式应在 in-chat 中
        assert!(result.messages.iter().any(|m| m.content.contains("风格：简洁")));
        // 变量应被填充
        assert_eq!(result.vars.get("style"), Some(&"简洁".to_string()));
    }

    #[test]
    fn test_empty_blocks_skipped() {
        let json = r#"{
            "prompts": [
                {"identifier":"a","name":"empty","content":"","enabled":true,"injection_position":-1,"injection_depth":0,"injection_order":100,"role":"system"},
                {"identifier":"b","name":"disabled","content":"hidden","enabled":false,"injection_position":-1,"injection_depth":0,"injection_order":100,"role":"system"},
                {"identifier":"c","name":"visible","content":"hello","enabled":true,"injection_position":-1,"injection_depth":0,"injection_order":100,"role":"system"}
            ]
        }"#;
        let blocks = parse_preset_blocks(json).unwrap();
        let mut c = ctx();
        let result = assemble(&blocks, &mut c, &[], None);
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].content, "hello");
    }

    #[test]
    fn test_injection_depth() {
        let json = r#"{
            "prompts": [
                {"identifier":"sys","name":"Main","content":"SYS","enabled":true,"injection_position":-1,"injection_depth":0,"injection_order":0,"role":"system"},
                {"identifier":"inject","name":"Inject","content":"INJECTED","enabled":true,"injection_position":0,"injection_depth":2,"injection_order":100,"role":"user"}
            ]
        }"#;
        let blocks = parse_preset_blocks(json).unwrap();
        let mut c = ctx();
        let history = vec![
            ChatMessage { role: "user".into(), content: "u1".into() },
            ChatMessage { role: "assistant".into(), content: "a1".into() },
            ChatMessage { role: "user".into(), content: "u2".into() },
            ChatMessage { role: "assistant".into(), content: "a2".into() },
        ];
        let result = assemble(&blocks, &mut c, &history, None);
        // depth=2: inject at position len-2 = 2 (0-indexed), so:
        // [SYS, u1, a1, INJECTED(user), u2(user), a2]
        // compress_adjacent merges INJECTED+u2 (both user) → "INJECTED\nu2"
        let contents: Vec<&str> = result.messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(contents, vec!["SYS", "u1", "a1", "INJECTED\nu2", "a2"]);
    }

    #[test]
    fn test_unknown_macro_preserved() {
        let mut c = ctx();
        let out = render_macros("{{unknown_macro::x}}", &mut c);
        assert!(out.contains("unknown_macro"));
    }

    #[test]
    fn test_angle_bracket_names() {
        let mut c = ctx();
        let out = render_macros("角色是<user>和<char>还有<BOT>", &mut c);
        assert_eq!(out, "角色是梦客和梦鲸思客还有梦鲸思客");
    }

    #[test]
    fn test_setvar_with_colons_in_value() {
        let mut c = ctx();
        // 值内有 :: 但应该整体保留
        let out = render_macros("{{setvar::url::http://x::8080::path}}[{{getvar::url}}]", &mut c);
        assert_eq!("[http://x::8080::path]", out);
    }

    // ─── 跨 provider 类型默认回填 ───────────────────────────────────────────

    #[test]
    fn test_prompt_source_parse_variants() {
        // ProviderKind camelCase 序列化形式
        assert_eq!(PromptSource::parse("openAI"), Some(PromptSource::OpenAi));
        assert_eq!(PromptSource::parse("Anthropic"), Some(PromptSource::Claude));
        assert_eq!(PromptSource::parse("google"), Some(PromptSource::Google));
        // ST chat_completion_source 值
        assert_eq!(PromptSource::parse("claude"), Some(PromptSource::Claude));
        assert_eq!(PromptSource::parse("makersuite"), Some(PromptSource::Google));
        // 第三方中转归 Custom
        assert_eq!(PromptSource::parse("openrouter"), Some(PromptSource::Custom));
        assert_eq!(PromptSource::parse("deepseek"), Some(PromptSource::Custom));
        // 空串默认 OpenAI（TT ChatCompletionSource::parse 同语义）
        assert_eq!(PromptSource::parse(""), Some(PromptSource::OpenAi));
        assert_eq!(PromptSource::parse("  OpenAI  "), Some(PromptSource::OpenAi));
        assert_eq!(PromptSource::parse("not-a-source"), None);
    }

    #[test]
    fn test_backfill_clears_stale_source_slots() {
        // 预设里同时残留了上一次 Claude 的模型键和 azure 槽位
        let mut settings = json!({
            "prompts": [{"identifier": "main", "content": "hi"}],
            "openai_model": "gpt-4o",
            "claude_model": "claude-3-5-sonnet",
            "google_model": "gemini-2.0",
            "azure_deployment_name": "old-deploy",
            "temperature": 0.7
        });
        let defaults = json!({
            "openai_model": "gpt-4o-mini",
            "azure_deployment_name": "new-deploy",
            "model": "gpt-4o-mini"
        });

        apply_source_scoped_backfill(&mut settings, "openai", &defaults).unwrap();

        // 当前来源的键被回填
        assert_eq!(settings["openai_model"], json!("gpt-4o-mini"));
        assert_eq!(settings["azure_deployment_name"], json!("new-deploy"));
        assert_eq!(settings["model"], json!("gpt-4o-mini"));
        // 旧来源的残留被清干净
        assert!(settings.get("claude_model").is_none());
        assert!(settings.get("google_model").is_none());
        // 快照里没有的键不会被凭空造出来
        assert!(settings.get("custom_model").is_none());
        // 来源标记
        assert_eq!(settings["chat_completion_source"], json!("openai"));
        // 预设自身资产不受影响
        assert_eq!(settings["temperature"], json!(0.7));
        assert_eq!(settings["prompts"][0]["identifier"], json!("main"));
    }

    #[test]
    fn test_backfill_switches_across_all_three_provider_kinds() {
        for (source, expected_key, expected_stale) in [
            ("openai", "openai_model", "claude_model"),
            ("anthropic", "claude_model", "google_model"),
            ("google", "google_model", "openai_model"),
        ] {
            let mut settings = json!({
                "openai_model": "a", "claude_model": "b", "google_model": "c"
            });
            let mut defaults = json!({});
            defaults[expected_key] = json!("fresh");

            apply_source_scoped_backfill(&mut settings, source, &defaults).unwrap();

            assert_eq!(settings[expected_key], json!("fresh"), "source={source}");
            assert!(settings.get(expected_stale).is_none(), "source={source}");
        }
    }

    #[test]
    fn test_backfill_source_marker_overrides_stale_snapshot_value() {
        let mut settings = json!({"openai_model": "old"});
        // 快照里带着过期的 chat_completion_source，必须被本次来源覆盖
        let defaults = json!({
            "chat_completion_source": "claude",
            "openai_model": "new"
        });

        apply_source_scoped_backfill(&mut settings, "google", &defaults).unwrap();

        assert_eq!(settings["chat_completion_source"], json!("makersuite"));
        assert_eq!(settings["openai_model"], json!("new"));
    }

    #[test]
    fn test_backfill_filters_non_whitelisted_snapshot_fields() {
        // 连接快照里的非白名单字段不得渗进用户预设
        let mut settings = json!({"name": "我的预设"});
        let defaults = json!({
            "openai_model": "gpt-4o",
            "api_key": "sk-should-not-leak",
            "secret_id": "also-secret",
            "some_internal_flag": true
        });

        apply_source_scoped_backfill(&mut settings, "openai", &defaults).unwrap();

        assert_eq!(settings["openai_model"], json!("gpt-4o"));
        assert!(settings.get("api_key").is_none());
        assert!(settings.get("secret_id").is_none());
        assert!(settings.get("some_internal_flag").is_none());
    }

    #[test]
    fn test_backfill_empty_defaults_clears_everything() {
        // 快照为空：清掉全部连接键，仅保留来源标记与用户字段
        let mut settings = json!({
            "openai_model": "a", "claude_model": "b", "model": "c",
            "custom_url": "https://x", "name": "keep-me"
        });

        apply_source_scoped_backfill(&mut settings, "claude", &json!({})).unwrap();

        assert_eq!(settings["name"], json!("keep-me"));
        assert_eq!(settings["chat_completion_source"], json!("claude"));
        for key in ["openai_model", "claude_model", "model", "custom_url"] {
            assert!(settings.get(key).is_none(), "{key} should be cleared");
        }
    }

    #[test]
    fn test_backfill_rejects_bad_input() {
        let mut not_object = json!("i am a string");
        let err = apply_source_scoped_backfill(&mut not_object, "openai", &json!({})).unwrap_err();
        assert!(err.contains("preset_settings_invalid"), "{err}");

        let mut settings = json!({});
        let err = apply_source_scoped_backfill(&mut settings, "totally-bogus", &json!({})).unwrap_err();
        assert!(err.contains("prompt_source_unsupported"), "{err}");
    }

    #[test]
    fn test_connection_setting_keys_cover_every_source_slot() {
        let keys: Vec<&str> = connection_setting_keys().collect();
        for source in PromptSource::ALL {
            assert!(
                keys.contains(&source.model_setting_key()),
                "{} slot missing",
                source.model_setting_key()
            );
        }
        for (key, _) in SOURCE_SPECIFIC_SETTING_KEYS {
            assert!(keys.contains(key), "{key} missing");
        }
        for key in CONNECTION_SETTING_KEYS {
            assert!(keys.contains(key), "{key} missing");
        }
    }

    // ─── 宏展开 DoS 防护（递归深度 / 输出长度上限）───

    #[test]
    fn test_self_referential_macro_does_not_overflow_stack() {
        // a 已经持有指向自身的值（模拟第二遍渲染 / 导入的变量表），
        // 此时 {{setvar::a::{{getvar::a}}}} 会真正无限递归。必须在深度上限处原样保留。
        let mut c = ctx();
        c.vars.insert("a".into(), "{{getvar::a}}".into());
        let out = render_macros("{{setvar::a::{{getvar::a}}}}[{{getvar::a}}]", &mut c);
        assert!(out.starts_with('['), "{out}");
        assert!(out.ends_with(']'), "{out}");
        let inner = &out[1..out.len() - 1];
        assert_eq!(
            inner, "{{getvar::a}}",
            "深度超限应原样保留宏文本而不是清空或继续递归"
        );
        // 关键：输出有界，不是指数级膨胀
        assert!(out.len() < 4096, "输出应被深度上限约束，实际 {} 字节", out.len());
    }

    #[test]
    fn test_self_referential_setvar_on_fresh_context_is_bounded() {
        // 冷启动上下文：a 在自身值渲染完成前还不存在，因此不递归，输出为空——不 panic 即达标
        let mut c = ctx();
        let out = render_macros("{{setvar::a::{{getvar::a}}}}[{{getvar::a}}]", &mut c);
        assert_eq!(out, "[]");
    }

    #[test]
    fn test_self_referential_globalvar_does_not_overflow_stack() {
        let mut c = ctx();
        c.global_vars.insert("g".into(), "{{getglobalvar::g}}".into());
        let out = render_macros("[{{getglobalvar::g}}]", &mut c);
        assert!(out.len() < 4096, "输出应被约束，实际 {} 字节", out.len());
    }

    #[test]
    fn test_depth_limit_preserves_macro_text_instead_of_emptying() {
        // getvar 链 v0 -> v1 -> ... -> vN：N+1 层展开在深度上限处停下，
        // 越界那一层的宏被**原样保留**，而不是被清空成空串（与未知宏同一语义）。
        //
        // 已知：17 层（n=16）完整展开为 "leaf"；18 层（n=17）在末层保留宏文本。
        let build = |n: usize| -> MacroContext {
            let mut c = ctx();
            for i in 0..n {
                let val = if i + 1 < n {
                    format!("{{{{getvar::v{}}}}}", i + 1)
                } else {
                    "leaf".to_string()
                };
                c.vars.insert(format!("v{i}"), val);
            }
            c
        };

        // 上限之内：正常展开
        let mut c = build(16);
        assert_eq!(render_macros("{{getvar::v0}}", &mut c), "leaf");

        // 超出上限：原样保留越界那一层的宏文本
        let mut c = build(18);
        let out = render_macros("{{getvar::v0}}", &mut c);
        assert_eq!(
            out, "{{getvar::v17}}",
            "深度超限应原样保留宏文本而非清空"
        );
    }

    #[test]
    fn test_nested_macros_within_depth_limit_still_render() {
        // 正常多层嵌套（深度远小于上限）必须照常渲染
        let mut c = ctx();
        let out = render_macros(
            "{{setvar::a::v1}}{{setvar::b::[{{getvar::a}}]}}{{addvar::c::x}}{{addvar::c::y}}{{setvar::d::{{getvar::c}}}}{{getvar::b}}{{getvar::d}}",
            &mut c,
        );
        assert_eq!(out, "[v1]x\ny");
    }

    #[test]
    fn test_output_budget_truncates_huge_expansion() {
        // 单个变量值 512KB > MAX_MACRO_OUTPUT_BYTES(256KB)，应被截断且不 panic
        let mut c = ctx();
        let big = "A".repeat(512 * 1024);
        c.vars.insert("big".into(), big);
        let out = render_macros("{{getvar::big}}", &mut c);
        assert!(
            out.len() <= MAX_MACRO_OUTPUT_BYTES,
            "输出 {} 字节超过上限",
            out.len()
        );
    }

    #[test]
    fn test_output_budget_truncates_multiplication_via_addvar() {
        // addvar 自增殖：a 引用自身，累加 N 次 → 2^N 级别输出，必须被预算截断
        let mut c = ctx();
        let mut input = String::from("{{setvar::a::seed}}");
        for _ in 0..24 {
            input.push_str("{{addvar::a::{{getvar::a}}{{getvar::a}}}}");
        }
        let out = render_macros(&input, &mut c);
        assert!(
            out.is_empty(),
            "setvar/addvar 本身输出空串（预算在变量值里被消耗）"
        );
        assert!(
            c.vars.get("a").map(|v| v.len()).unwrap_or(0) <= MAX_MACRO_OUTPUT_BYTES,
            "变量值应被预算截断，实际 {} 字节",
            c.vars.get("a").map(|v| v.len()).unwrap_or(0)
        );
    }

    #[test]
    fn test_truncation_respects_utf8_char_boundary() {
        // 多字节字符落在预算边界上时不得 panic（不得切在字符中间）
        let mut c = ctx();
        let big = "梦".repeat(MAX_MACRO_OUTPUT_BYTES); // 3 bytes/char
        c.vars.insert("u".into(), big);
        let out = render_macros("{{getvar::u}}", &mut c);
        assert!(out.len() <= MAX_MACRO_OUTPUT_BYTES);
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
    }
}
