//! SillyTavern Quick Reply 数据格式（用户手搓的斜杠命令工作流库）。
//!
//! 来源字段（v2）：`public/scripts/extensions/quick-reply/QuickReply.js:33-51`
//! （单条 QR）、`QuickReplySet.js:362-374`（set 结构）、
//! v1→v2 迁移映射见 `quick-reply/index.js:69-96`。
//!
//! 兼容红线：
//! - 全部字段 camelCase **双写**（读时 camel/snake 都认，存时两套都写），
//!   与 st_regex / st_chat_import 保持一致。
//! - 缺字段 = 沿用 ST 默认值，不因缺失报错丢条目。
//!
//! **重要**：QR 的 `message` 是 ST 斜杠命令文本（`/newchat`、`/?`、`/bgcol` …），
//! 依赖 ST 自己的斜杠命令系统才能执行。MuseAI 有宏引擎但没有 ST 斜杠系统，
//! 所以这里 **只解析/存储，绝不假装能执行**；`map_command_to_museai_macro`
//! 是一个纯映射函数：能映射成 MuseAI 宏的给宏文本，不能的显式标注
//! `unsupported_command`，绝不静默丢弃原始命令文本。

use serde_json::{json, Map, Value};

/// QuickReplyConfig 的作用域三值（ST: global / chat / character）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickReplyScope {
    Global,
    Chat,
    Character,
}

impl QuickReplyScope {
    pub fn as_str(self) -> &'static str {
        match self {
            QuickReplyScope::Global => "global",
            QuickReplyScope::Chat => "chat",
            QuickReplyScope::Character => "character",
        }
    }

    /// 解析 ST 存盘字符串；未知值退回 `Global`（ST 自身也是这个兜底）。
    pub fn parse(s: &str) -> QuickReplyScope {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" => QuickReplyScope::Chat,
            "character" => QuickReplyScope::Character,
            _ => QuickReplyScope::Global,
        }
    }

    /// 三值枚举，供 UI 下拉使用。
    pub fn all() -> [QuickReplyScope; 3] {
        [
            QuickReplyScope::Global,
            QuickReplyScope::Chat,
            QuickReplyScope::Character,
        ]
    }
}

/// 解析路径的只读查询。
fn str_f(obj: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = obj.get(*k).and_then(|v| v.as_str()) {
            return s.to_string();
        }
    }
    String::new()
}

fn bool_f(obj: &Value, keys: &[&str], d: bool) -> bool {
    for k in keys {
        if let Some(b) = obj.get(*k).and_then(|v| v.as_bool()) {
            return b;
        }
    }
    d
}

fn i32_f(obj: &Value, keys: &[&str], d: i32) -> i32 {
    for k in keys {
        if let Some(n) = obj.get(*k) {
            if let Some(i) = n.as_i64() {
                return i as i32;
            }
            if let Some(f) = n.as_f64() {
                return f as i32;
            }
            if let Some(s) = n.as_str().and_then(|s| s.trim().parse::<i32>().ok()) {
                return s;
            }
        }
    }
    d
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

// ─── 单条 QuickReply ───────────────────────────────────────────────────────

/// 一条 QuickReply（v2 结构）。字段名对齐 QuickReply.js:33-51。
#[derive(Debug, Clone, PartialEq)]
pub struct StQuickReply {
    pub id: String,
    /// 按钮图标（emoji / font-awesome class）。
    pub icon: String,
    /// 是否在按钮上显示文字。
    pub show_label: bool,
    /// 按钮文字。
    pub label: String,
    /// 悬停 title。
    pub title: String,
    /// 斜杠命令 / 消息文本。**原样保留，不执行。**
    pub message: String,
    /// 菜单中显示该 QR 的上下文名列表（空 = 所有上下文）。
    pub context_list: Vec<String>,
    /// 阻止自动执行（按钮点击只填入输入框）。
    pub prevent_auto_execute: bool,
    pub is_hidden: bool,
    pub execute_on_startup: bool,
    pub execute_on_user: bool,
    pub execute_on_ai: bool,
    pub execute_on_chat_change: bool,
    pub execute_on_group_member_draft: bool,
    pub execute_on_new_chat: bool,
    pub execute_before_generation: bool,
    /// 绑定到 ST 自动化（extension-automation）的 automationId。
    pub automation_id: String,
}

impl Default for StQuickReply {
    fn default() -> Self {
        Self {
            id: String::new(),
            icon: String::new(),
            show_label: true,
            label: String::new(),
            title: String::new(),
            message: String::new(),
            context_list: Vec::new(),
            prevent_auto_execute: false,
            is_hidden: false,
            execute_on_startup: false,
            execute_on_user: false,
            execute_on_ai: false,
            execute_on_chat_change: false,
            execute_on_group_member_draft: false,
            execute_on_new_chat: false,
            execute_before_generation: false,
            automation_id: String::new(),
        }
    }
}

impl StQuickReply {
    /// 解析一条 v2 QuickReply。非对象返回 `None`；字段缺失走默认值（不丢条目）。
    pub fn from_value(raw: &Value) -> Option<StQuickReply> {
        if !raw.is_object() {
            return None;
        }
        Some(StQuickReply {
            id: str_f(raw, &["id"]),
            icon: str_f(raw, &["icon"]),
            show_label: bool_f(raw, &["showLabel", "show_label"], true),
            label: str_f(raw, &["label"]),
            title: str_f(raw, &["title"]),
            message: str_f(raw, &["message", "mes"]),
            context_list: str_list(raw.get("contextList").or_else(|| raw.get("context_list"))),
            prevent_auto_execute: bool_f(
                raw,
                &["preventAutoExecute", "prevent_auto_execute"],
                false,
            ),
            is_hidden: bool_f(raw, &["isHidden", "is_hidden", "hidden"], false),
            execute_on_startup: bool_f(raw, &["executeOnStartup", "execute_on_startup"], false),
            execute_on_user: bool_f(raw, &["executeOnUser", "execute_on_user"], false),
            execute_on_ai: bool_f(raw, &["executeOnAi", "execute_on_ai"], false),
            execute_on_chat_change: bool_f(
                raw,
                &["executeOnChatChange", "execute_on_chat_change"],
                false,
            ),
            execute_on_group_member_draft: bool_f(
                raw,
                &[
                    "executeOnGroupMemberDraft",
                    "execute_on_group_member_draft",
                ],
                false,
            ),
            execute_on_new_chat: bool_f(raw, &["executeOnNewChat", "execute_on_new_chat"], false),
            execute_before_generation: bool_f(
                raw,
                &["executeBeforeGeneration", "execute_before_generation"],
                false,
            ),
            automation_id: str_f(raw, &["automationId", "automation_id"]),
        })
    }

    /// 序列化为 v2 结构，camel / snake 双写。
    pub fn to_value(&self) -> Value {
        let ctx = Value::Array(
            self.context_list
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
        );
        json!({
            "id": self.id,
            "icon": self.icon,
            "showLabel": self.show_label,
            "show_label": self.show_label,
            "label": self.label,
            "title": self.title,
            "message": self.message,
            "contextList": ctx,
            "context_list": ctx,
            "preventAutoExecute": self.prevent_auto_execute,
            "prevent_auto_execute": self.prevent_auto_execute,
            "isHidden": self.is_hidden,
            "is_hidden": self.is_hidden,
            "executeOnStartup": self.execute_on_startup,
            "execute_on_startup": self.execute_on_startup,
            "executeOnUser": self.execute_on_user,
            "execute_on_user": self.execute_on_user,
            "executeOnAi": self.execute_on_ai,
            "execute_on_ai": self.execute_on_ai,
            "executeOnChatChange": self.execute_on_chat_change,
            "execute_on_chat_change": self.execute_on_chat_change,
            "executeOnGroupMemberDraft": self.execute_on_group_member_draft,
            "execute_on_group_member_draft": self.execute_on_group_member_draft,
            "executeOnNewChat": self.execute_on_new_chat,
            "execute_on_new_chat": self.execute_on_new_chat,
            "executeBeforeGeneration": self.execute_before_generation,
            "execute_before_generation": self.execute_before_generation,
            "automationId": self.automation_id,
            "automation_id": self.automation_id,
        })
    }

    /// `message` 是否是 ST 斜杠命令（`/xxx` 或 `/?`）。
    pub fn is_slash_command(&self) -> bool {
        let t = self.message.trim_start();
        t.starts_with('/')
    }

    /// 斜杠命令名（小写，无 `/`）。非命令返回 `None`。
    pub fn command_name(&self) -> Option<String> {
        let t = self.message.trim();
        let t = t.strip_prefix('/')?;
        let name: String = t
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if name.is_empty() {
            // `/?` 之类的裸问号命令：整个 token 作为名字
            Some(t.split_whitespace().next().unwrap_or("?").to_ascii_lowercase())
        } else {
            Some(name.to_ascii_lowercase())
        }
    }

    /// 把 `message` 映射到 MuseAI 宏（纯函数，不执行、不改数据）。
    pub fn map_to_museai_macro(&self) -> SlashMapping {
        map_command_to_museai_macro(&self.message)
    }
}

// ─── QuickReplySet v2 ──────────────────────────────────────────────────────

/// 一个 QR 集合（QuickReplySet v2，QuickReplySet.js:362-374）。
#[derive(Debug, Clone, PartialEq)]
pub struct StQuickReplySet {
    /// 固定为 2（v1 集合迁上来时置 2）。
    pub version: i32,
    pub name: String,
    /// 点击后不自动发送（只把 message 塞进输入框）。
    pub disable_send: bool,
    pub place_before_input: bool,
    /// 把 message 直接注入输入框顶部而非末尾。
    pub inject_input: bool,
    /// 按钮颜色（CSS 值 / 内置色名）。
    pub color: String,
    /// 只描边不填充。
    pub only_border_color: bool,
    pub qr_list: Vec<StQuickReply>,
    /// ST 用来生成新 id 的自增计数。
    pub id_index: i32,
}

impl Default for StQuickReplySet {
    fn default() -> Self {
        Self {
            version: 2,
            name: String::new(),
            disable_send: false,
            place_before_input: false,
            inject_input: false,
            color: String::new(),
            only_border_color: false,
            qr_list: Vec::new(),
            id_index: 0,
        }
    }
}

impl StQuickReplySet {
    /// 解析集合。v1 形状（`quickReplySlots`）自动走 v1→v2 迁移。
    pub fn from_value(raw: &Value) -> Option<StQuickReplySet> {
        if !raw.is_object() {
            return None;
        }
        // v1：无 version 或 version<2，且带 quickReplySlots
        let slots = raw
            .get("quickReplySlots")
            .or_else(|| raw.get("quick_reply_slots"));
        if slots.is_some() {
            return Some(migrate_v1_set_to_v2(raw));
        }
        let mut set = StQuickReplySet {
            version: i32_f(raw, &["version"], 2),
            name: str_f(raw, &["name"]),
            disable_send: bool_f(raw, &["disableSend", "disable_send"], false),
            place_before_input: bool_f(raw, &["placeBeforeInput", "place_before_input"], false),
            inject_input: bool_f(raw, &["injectInput", "inject_input"], false),
            color: str_f(raw, &["color"]),
            only_border_color: bool_f(raw, &["onlyBorderColor", "only_border_color"], false),
            qr_list: raw
                .get("qrList")
                .or_else(|| raw.get("qr_list"))
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(StQuickReply::from_value).collect())
                .unwrap_or_default(),
            id_index: i32_f(raw, &["idIndex", "id_index"], 0),
        };
        if set.version < 2 {
            set.version = 2;
        }
        Some(set)
    }

    /// 序列化为 v2 结构，camel / snake 双写。
    pub fn to_value(&self) -> Value {
        let list = Value::Array(self.qr_list.iter().map(|q| q.to_value()).collect());
        json!({
            "version": self.version,
            "name": self.name,
            "disableSend": self.disable_send,
            "disable_send": self.disable_send,
            "placeBeforeInput": self.place_before_input,
            "place_before_input": self.place_before_input,
            "injectInput": self.inject_input,
            "inject_input": self.inject_input,
            "color": self.color,
            "onlyBorderColor": self.only_border_color,
            "only_border_color": self.only_border_color,
            "qrList": list,
            "qr_list": list,
            "idIndex": self.id_index,
            "id_index": self.id_index,
        })
    }

    /// 追加一条并按 ST 语义刷新 `idIndex`。
    pub fn push(&mut self, mut qr: StQuickReply) {
        if qr.id.trim().is_empty() {
            qr.id = self.id_index.to_string();
        }
        self.id_index = self.id_index.max(qr.id.parse::<i32>().unwrap_or(0) + 1);
        self.qr_list.push(qr);
    }
}

// ─── v1 → v2 迁移（index.js:69-96） ───────────────────────────────────────

/// v1 单条 slot（`quickReplySlots[]`）字段 → v2 QuickReply。
/// 映射：label→label, title→title, mes→message, hidden→isHidden,
/// contextMenu→contextList, autoExecute_* → executeOn*。
fn migrate_v1_slot(slot: &Value, index: i32) -> Option<StQuickReply> {
    if !slot.is_object() {
        return None;
    }
    let id = {
        let s = str_f(slot, &["id"]);
        if s.is_empty() {
            index.to_string()
        } else {
            s
        }
    };
    Some(StQuickReply {
        id,
        icon: str_f(slot, &["icon"]),
        show_label: bool_f(slot, &["showLabel", "show_label"], true),
        label: str_f(slot, &["label"]),
        title: str_f(slot, &["title"]),
        message: str_f(slot, &["mes", "message"]),
        context_list: str_list(
            slot.get("contextMenu")
                .or_else(|| slot.get("context_menu"))
                .or_else(|| slot.get("contextList"))
                .or_else(|| slot.get("context_list")),
        ),
        prevent_auto_execute: bool_f(
            slot,
            &["preventAutoExecute", "prevent_auto_execute"],
            false,
        ),
        is_hidden: bool_f(slot, &["hidden", "isHidden", "is_hidden"], false),
        execute_on_startup: bool_f(
            slot,
            &[
                "autoExecute_starts",
                "autoExecuteStarts",
                "executeOnStartup",
            ],
            false,
        ),
        execute_on_user: bool_f(
            slot,
            &["autoExecute_user", "autoExecuteUser", "executeOnUser"],
            false,
        ),
        execute_on_ai: bool_f(
            slot,
            &["autoExecute_ai", "autoExecuteAi", "executeOnAi"],
            false,
        ),
        execute_on_chat_change: bool_f(
            slot,
            &[
                "autoExecute_chat_change",
                "autoExecuteChatChange",
                "executeOnChatChange",
            ],
            false,
        ),
        execute_on_group_member_draft: bool_f(
            slot,
            &[
                "autoExecute_group_member_draft",
                "autoExecuteGroupMemberDraft",
                "executeOnGroupMemberDraft",
            ],
            false,
        ),
        execute_on_new_chat: bool_f(
            slot,
            &["autoExecute_new_chat", "autoExecuteNewChat", "executeOnNewChat"],
            false,
        ),
        execute_before_generation: bool_f(
            slot,
            &[
                "executeBeforeGeneration",
                "execute_before_generation",
                "autoExecute_before_generation",
            ],
            false,
        ),
        automation_id: str_f(slot, &["automationId", "automation_id"]),
    })
}

/// v1 集合 → v2 集合。v1 顶层键：name / disableSend / placeBeforeInput /
/// injectInput / color / onlyBorderColor / quickReplySlots[]。
pub fn migrate_v1_set_to_v2(raw: &Value) -> StQuickReplySet {
    let slots = raw
        .get("quickReplySlots")
        .or_else(|| raw.get("quick_reply_slots"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let qr_list: Vec<StQuickReply> = slots
        .iter()
        .enumerate()
        .filter_map(|(i, s)| migrate_v1_slot(s, i as i32))
        .collect();
    let id_index = i32_f(raw, &["idIndex", "id_index"], 0)
        .max(qr_list.len() as i32)
        .max(
            qr_list
                .iter()
                .filter_map(|q| q.id.parse::<i32>().ok())
                .max()
                .map(|m| m + 1)
                .unwrap_or(0),
        );
    StQuickReplySet {
        version: 2,
        name: str_f(raw, &["name"]),
        disable_send: bool_f(raw, &["disableSend", "disable_send"], false),
        place_before_input: bool_f(raw, &["placeBeforeInput", "place_before_input"], false),
        inject_input: bool_f(raw, &["injectInput", "inject_input"], false),
        color: str_f(raw, &["color"]),
        only_border_color: bool_f(raw, &["onlyBorderColor", "only_border_color"], false),
        qr_list,
        id_index,
    }
}

// ─── QuickReplyConfig ──────────────────────────────────────────────────────

/// 哪个作用域下的 QR 库启用（ST 三值：global / chat / character）。
#[derive(Debug, Clone, PartialEq)]
pub struct StQuickReplyConfig {
    pub scope: QuickReplyScope,
    /// 该作用域下选中的 set 名字（ST 存 `selectedSetName`）。
    pub selected_set: String,
    /// 该作用域下全部 set 名字（ST 的 list）。
    pub sets: Vec<String>,
    /// 该作用域是否启用 QR 按钮条。
    pub enabled: bool,
}

impl StQuickReplyConfig {
    pub fn from_value(raw: &Value) -> Option<StQuickReplyConfig> {
        if !raw.is_object() {
            return None;
        }
        Some(StQuickReplyConfig {
            scope: QuickReplyScope::parse(&str_f(raw, &["scope"])),
            selected_set: str_f(
                raw,
                &["selectedSetName", "selected_set_name", "selectedSet"],
            ),
            sets: str_list(raw.get("sets").or_else(|| raw.get("list"))),
            enabled: bool_f(raw, &["enabled"], true),
        })
    }

    pub fn to_value(&self) -> Value {
        let sets = Value::Array(self.sets.iter().map(|s| Value::String(s.clone())).collect());
        json!({
            "scope": self.scope.as_str(),
            "selectedSetName": self.selected_set,
            "selected_set_name": self.selected_set,
            "sets": sets,
            "enabled": self.enabled,
        })
    }
}

// ─── ST 斜杠命令 → MuseAI 宏（纯映射，不执行） ────────────────────────────

/// 一条命令的映射结果。
#[derive(Debug, Clone, PartialEq)]
pub struct SlashMapping {
    /// 原始命令文本，**永远原样保留**。
    pub original: String,
    /// 解析出的 ST 命令名（小写）。非命令时为 `None`。
    pub command: Option<String>,
    /// 可映射时给出 MuseAI 宏文本。
    pub museai_macro: Option<String>,
    /// 不可映射时的稳定标注（ST 斜杠系统没有对应物）。
    pub unsupported: Option<UnsupportedCommand>,
}

/// 不可映射的原因分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedCommand {
    /// 认识这是 ST 斜杠命令，但 MuseAI 没有等价命令。
    UnsupportedCommand,
    /// 根本不是斜杠命令：普通文本模板，可直接进宏渲染。
    PlainText,
}

impl UnsupportedCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            UnsupportedCommand::UnsupportedCommand => "unsupported_command",
            UnsupportedCommand::PlainText => "plain_text",
        }
    }
}

/// ST 斜杠命令 → MuseAI 宏的映射表（纯数据，穷举 ST 里常见的变量类命令）。
///
/// 判定规则：只有能确定等价语义的才映射；其余一律 `unsupported_command`。
/// `/newchat`、`/?`、`/bgcol` 这类由 ST 运行时/UI 实现的命令，**不映射**。
fn map_st_command(cmd: &str, args: &str) -> Option<String> {
    let args = args.trim();
    let (head, first_arg) = match args.split_once(char::is_whitespace) {
        Some((h, r)) => (h.trim(), r.trim()),
        None => (args, ""),
    };
    let quoted = if first_arg.starts_with('"') && first_arg.ends_with('"') && first_arg.len() >= 2 {
        &first_arg[1..first_arg.len() - 1]
    } else {
        first_arg
    };
    Some(match cmd {
        "setvar" | "set" => {
            if first_arg.is_empty() {
                return None;
            }
            format!("{{{{setvar::{head}::{quoted}}}}}")
        }
        "addvar" | "add" => {
            if first_arg.is_empty() {
                return None;
            }
            format!("{{{{addvar::{head}::{quoted}}}}}")
        }
        "getvar" | "get" => {
            if head.is_empty() {
                return None;
            }
            format!("{{{{getvar::{head}}}}}")
        }
        "getglobalvar" => {
            if head.is_empty() {
                return None;
            }
            format!("{{{{getglobalvar::{head}}}}}")
        }
        "user" | "username" => "{{user}}".to_string(),
        "char" | "botname" => "{{char}}".to_string(),
        "lastmessage" | "lastuser" => "{{lastUserMessage}}".to_string(),
        "memory" => "{{MEMORY}}".to_string(),
        "trim" => "{{trim}}".to_string(),
        // ST UI/运行时命令，MuseAI 无等价物
        _ => return None,
    })
}

/// 把一条 QR `message` 映射为 MuseAI 宏。
///
/// 纯函数：不执行任何命令、不修改输入、无副作用。
/// 非斜杠命令（普通文本模板）原样返回，标注 `plain_text`。
/// ST 斜杠命令能映射的给 `macro`，否则标 `unsupported_command`。
pub fn map_command_to_museai_macro(message: &str) -> SlashMapping {
    let original = message.to_string();
    let trimmed = message.trim();
    if !trimmed.starts_with('/') {
        return SlashMapping {
            original,
            command: None,
            museai_macro: Some(trimmed.to_string()),
            unsupported: Some(UnsupportedCommand::PlainText),
        };
    }
    let body = &trimmed[1..];
    let (name_raw, args) = match body.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r),
        None => (body, ""),
    };
    let cmd = name_raw.to_ascii_lowercase();
    let cmd = if cmd.is_empty() { "?" } else { cmd.as_str() };
    match map_st_command(cmd, args) {
        Some(m) => SlashMapping {
            original,
            command: Some(cmd.to_string()),
            museai_macro: Some(m),
            unsupported: None,
        },
        None => SlashMapping {
            original,
            command: Some(cmd.to_string()),
            museai_macro: None,
            unsupported: Some(UnsupportedCommand::UnsupportedCommand),
        },
    }
}

/// 批量统计一份 set 里的斜杠命令可映射性（导入报告用）。
/// 返回 `(可映射, 不可映射, 非命令文本)`。
pub fn count_slash_mappability(qrs: &[StQuickReply]) -> (usize, usize, usize) {
    let mut mapped = 0;
    let mut unsupported = 0;
    let mut plain = 0;
    for q in qrs {
        match q.map_to_museai_macro().unsupported {
            Some(UnsupportedCommand::PlainText) => plain += 1,
            Some(UnsupportedCommand::UnsupportedCommand) => unsupported += 1,
            None => mapped += 1,
        }
    }
    (mapped, unsupported, plain)
}

/// 从 ST 的 `extension_settings` 里抠出 quick_reply 相关的键名（供 archive 查询用）。
pub fn quick_reply_setting_keys() -> Vec<String> {
    vec![
        "quick_reply".to_string(),
        "quickReply".to_string(),
        "quick_replies".to_string(),
        "quickReplies".to_string(),
        "quick_reply_sets".to_string(),
        "quickReplySets".to_string(),
    ]
}

/// 把一个 JSON 字符串解析成 set；解析失败返回 `None`（不 panic）。
pub fn parse_set_json(s: &str) -> Option<StQuickReplySet> {
    let v: Value = serde_json::from_str(s).ok()?;
    StQuickReplySet::from_value(&v)
}

/// 从 `extension_settings` 根对象里收集所有 QR set（逐个键尝试解析，坏键跳过）。
pub fn sets_from_extension_settings(root: &Value) -> Vec<StQuickReplySet> {
    let mut out = Vec::new();
    if !root.is_object() {
        return out;
    }
    for key in quick_reply_setting_keys() {
        let Some(v) = root.get(&key) else { continue };
        collect_sets(v, &mut out);
    }
    out
}

fn collect_sets(v: &Value, out: &mut Vec<StQuickReplySet>) {
    match v {
        Value::Array(a) => {
            for item in a {
                collect_sets(item, out);
            }
        }
        Value::Object(o) => {
            if o.contains_key("qrList")
                || o.contains_key("qr_list")
                || o.contains_key("quickReplySlots")
                || o.contains_key("quick_reply_slots")
            {
                if let Some(set) = StQuickReplySet::from_value(v) {
                    out.push(set);
                }
                return;
            }
            // `{ setName: {qrList:...}, ... }` 形状
            for (_, sub) in o {
                collect_sets(sub, out);
            }
        }
        _ => {}
    }
}

/// 便捷：从 set 里挑出可映射为宏的 QR（`macro` 非空且非 unsupported）。
pub fn mappable_macros(set: &StQuickReplySet) -> Vec<(String, String)> {
    set.qr_list
        .iter()
        .filter_map(|q| {
            q.map_to_museai_macro()
                .museai_macro
                .map(|m| (q.label.clone(), m))
        })
        .collect()
}

/// 便于测试/调试：set 的 to_value 追加一个自定义键（验证 snake 双写不被覆盖）。
#[allow(dead_code)]
fn inject_extra(map: &mut Map<String, Value>, k: &str, v: Value) {
    map.insert(k.to_string(), v);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_v2_quick_reply_full() {
        let raw = json!({
            "id": "1",
            "icon": "fa-solid fa-star",
            "showLabel": false,
            "label": "新对话",
            "title": "开新聊天",
            "message": "/newchat",
            "contextList": ["alone", "group"],
            "preventAutoExecute": true,
            "isHidden": true,
            "executeOnStartup": true,
            "executeOnUser": true,
            "executeOnAi": true,
            "executeOnChatChange": true,
            "executeOnGroupMemberDraft": true,
            "executeOnNewChat": true,
            "executeBeforeGeneration": true,
            "automationId": "auto-1"
        });
        let q = StQuickReply::from_value(&raw).expect("parse");
        assert_eq!(q.id, "1");
        assert_eq!(q.icon, "fa-solid fa-star");
        assert!(!q.show_label);
        assert_eq!(q.label, "新对话");
        assert_eq!(q.title, "开新聊天");
        assert_eq!(q.message, "/newchat");
        assert_eq!(q.context_list, vec!["alone", "group"]);
        assert!(q.prevent_auto_execute && q.is_hidden);
        assert!(q.execute_on_startup && q.execute_on_user && q.execute_on_ai);
        assert!(q.execute_on_chat_change && q.execute_on_group_member_draft);
        assert!(q.execute_on_new_chat && q.execute_before_generation);
        assert_eq!(q.automation_id, "auto-1");
    }

    #[test]
    fn missing_fields_use_st_defaults() {
        // 只有 message 一个字段 → 其余全部默认值，不丢条目
        let q = StQuickReply::from_value(&json!({"message": "hi"})).expect("parse");
        assert!(q.show_label, "ST 默认 showLabel=true");
        assert!(q.label.is_empty() && q.title.is_empty() && q.id.is_empty());
        assert!(q.context_list.is_empty());
        assert!(!q.prevent_auto_execute && !q.is_hidden);
        assert!(!q.execute_on_startup && !q.execute_on_user && !q.execute_on_ai);
        assert!(!q.execute_on_chat_change && !q.execute_on_group_member_draft);
        assert!(!q.execute_on_new_chat && !q.execute_before_generation);
        assert!(q.automation_id.is_empty());
    }

    #[test]
    fn non_object_is_none() {
        assert!(StQuickReply::from_value(&Value::Null).is_none());
        assert!(StQuickReply::from_value(&json!("str")).is_none());
        assert!(StQuickReplySet::from_value(&Value::Bool(true)).is_none());
    }

    #[test]
    fn camel_snake_dual_write_roundtrip() {
        let q = StQuickReply {
            id: "7".into(),
            label: "L".into(),
            message: "/sysinfo".into(),
            context_list: vec!["a".into(), "b".into()],
            execute_on_ai: true,
            ..Default::default()
        };
        let v = q.to_value();
        // 两套键都写出且值一致
        assert_eq!(v["showLabel"], v["show_label"]);
        assert_eq!(v["executeOnAi"], v["execute_on_ai"]);
        assert_eq!(v["isHidden"], v["is_hidden"]);
        assert_eq!(v["automationId"], v["automation_id"]);
        assert_eq!(v["contextList"], v["context_list"]);
        assert_eq!(v["contextList"], json!(["a", "b"]));
        // 再解析回来（含只带 snake 键的输入）
        let back = StQuickReply::from_value(&v).unwrap();
        assert_eq!(back, q);
        let snake_only = json!({
            "id": "7", "label": "L", "message": "/sysinfo",
            "context_list": ["a", "b"], "execute_on_ai": true,
            "show_label": true, "prevent_auto_execute": false
        });
        assert_eq!(StQuickReply::from_value(&snake_only).unwrap(), q);
    }

    #[test]
    fn set_v2_parse_and_dual_write() {
        let raw = json!({
            "version": 2,
            "name": "我的工作流",
            "disableSend": true,
            "placeBeforeInput": true,
            "injectInput": true,
            "color": "#ff00ff",
            "onlyBorderColor": true,
            "qrList": [{"id":"1","label":"a","message":"/newchat"}],
            "idIndex": 2
        });
        let s = StQuickReplySet::from_value(&raw).expect("parse");
        assert_eq!(s.version, 2);
        assert_eq!(s.name, "我的工作流");
        assert!(s.disable_send && s.place_before_input && s.inject_input);
        assert_eq!(s.color, "#ff00ff");
        assert!(s.only_border_color);
        assert_eq!(s.qr_list.len(), 1);
        assert_eq!(s.id_index, 2);
        let v = s.to_value();
        assert_eq!(v["disableSend"], v["disable_send"]);
        assert_eq!(v["placeBeforeInput"], v["place_before_input"]);
        assert_eq!(v["injectInput"], v["inject_input"]);
        assert_eq!(v["onlyBorderColor"], v["only_border_color"]);
        assert_eq!(v["idIndex"], v["id_index"]);
        assert_eq!(v["qrList"], v["qr_list"]);
        assert_eq!(StQuickReplySet::from_value(&v).unwrap(), s);
    }

    #[test]
    fn set_missing_fields_defaults() {
        let s = StQuickReplySet::from_value(&json!({"name": "x"})).expect("parse");
        assert_eq!(s.version, 2, "缺 version 视为 v2");
        assert_eq!(s.name, "x");
        assert!(!s.disable_send && !s.place_before_input && !s.inject_input);
        assert!(s.color.is_empty() && !s.only_border_color);
        assert!(s.qr_list.is_empty() && s.id_index == 0);
    }

    #[test]
    fn migrate_v1_set_to_v2_field_mapping() {
        let v1 = json!({
            "name": "老库",
            "disableSend": true,
            "placeBeforeInput": true,
            "color": "red",
            "onlyBorderColor": true,
            "quickReplySlots": [
                {
                    "label": "L1",
                    "title": "T1",
                    "mes": "/newchat",
                    "hidden": true,
                    "contextMenu": ["alone"],
                    "autoExecute_starts": true,
                    "autoExecute_user": true,
                    "autoExecute_ai": true,
                    "autoExecute_chat_change": true,
                    "autoExecute_group_member_draft": true,
                    "autoExecute_new_chat": true
                },
                {
                    "label": "L2",
                    "mes": "/bgcol blue",
                    "hidden": false
                }
            ]
        });
        let s = migrate_v1_set_to_v2(&v1);
        assert_eq!(s.version, 2, "迁移后必须置 v2");
        assert_eq!(s.name, "老库");
        assert!(s.disable_send && s.place_before_input && s.only_border_color);
        assert_eq!(s.color, "red");
        assert_eq!(s.qr_list.len(), 2);
        let a = &s.qr_list[0];
        assert_eq!(a.id, "0", "无 id 时用下标");
        assert_eq!(a.label, "L1");
        assert_eq!(a.title, "T1");
        assert_eq!(a.message, "/newchat", "mes → message");
        assert!(a.is_hidden, "hidden → isHidden");
        assert_eq!(a.context_list, vec!["alone"], "contextMenu → contextList");
        assert!(a.execute_on_startup, "autoExecute_starts → executeOnStartup");
        assert!(a.execute_on_user);
        assert!(a.execute_on_ai);
        assert!(a.execute_on_chat_change);
        assert!(a.execute_on_group_member_draft);
        assert!(a.execute_on_new_chat);
        let b = &s.qr_list[1];
        assert_eq!(b.id, "1");
        assert_eq!(b.message, "/bgcol blue");
        assert!(!b.is_hidden);
        assert_eq!(s.id_index, 2);
        // 迁移结果可直接当 v2 解析
        assert_eq!(StQuickReplySet::from_value(&s.to_value()).unwrap(), s);
    }

    #[test]
    fn set_from_value_dispatches_to_v1_migration() {
        let v1 = json!({
            "name": "v1",
            "quickReplySlots": [{"label": "x", "mes": "/newchat", "hidden": true}]
        });
        let s = StQuickReplySet::from_value(&v1).expect("parse v1");
        assert_eq!(s.version, 2);
        assert_eq!(s.qr_list.len(), 1);
        assert_eq!(s.qr_list[0].message, "/newchat");
        assert!(s.qr_list[0].is_hidden);
    }

    #[test]
    fn v1_slot_tolerates_camel_and_garbage() {
        let v1 = json!({
            "quickReplySlots": [
                {"label": "a", "autoExecuteAi": true, "contextMenu": []},
                "not-an-object",
                {"title": "b", "mes": "/?"}
            ],
            "idIndex": 9
        });
        let s = StQuickReplySet::from_value(&v1).expect("parse");
        assert_eq!(s.qr_list.len(), 2, "非对象 slot 跳过，不整体失败");
        assert!(s.qr_list[0].execute_on_ai, "camel autoExecuteAi 兼容");
        assert!(s.qr_list[1].context_list.is_empty());
        assert_eq!(s.qr_list[1].message, "/?");
        assert_eq!(s.id_index, 9);
    }

    #[test]
    fn set_push_assigns_ids_and_bumps_index() {
        let mut s = StQuickReplySet::default();
        s.push(StQuickReply {
            label: "a".into(),
            message: "/newchat".into(),
            ..Default::default()
        });
        assert_eq!(s.qr_list[0].id, "0");
        assert_eq!(s.id_index, 1);
        s.push(StQuickReply {
            id: "42".into(),
            ..Default::default()
        });
        assert_eq!(s.id_index, 43);
    }

    #[test]
    fn scope_three_values() {
        for s in QuickReplyScope::all() {
            let raw = json!({"scope": s.as_str()});
            assert_eq!(StQuickReplyConfig::from_value(&raw).unwrap().scope, s);
        }
        assert_eq!(QuickReplyScope::parse("GLOBAL"), QuickReplyScope::Global);
        assert_eq!(QuickReplyScope::parse(" Chat "), QuickReplyScope::Chat);
        assert_eq!(QuickReplyScope::parse("character"), QuickReplyScope::Character);
        // 未知/缺失 → Global（不报错）
        assert_eq!(QuickReplyScope::parse("weird"), QuickReplyScope::Global);
        assert_eq!(QuickReplyScope::parse(""), QuickReplyScope::Global);
        let c = StQuickReplyConfig::from_value(&json!({})).expect("parse");
        assert_eq!(c.scope, QuickReplyScope::Global);
        assert!(c.enabled);
        assert!(c.selected_set.is_empty() && c.sets.is_empty());
        assert!(StQuickReplyConfig::from_value(&json!([])).is_none());
    }

    #[test]
    fn config_roundtrip_dual_write() {
        let c = StQuickReplyConfig {
            scope: QuickReplyScope::Chat,
            selected_set: "我的工作流".into(),
            sets: vec!["a".into(), "b".into()],
            enabled: false,
        };
        let v = c.to_value();
        assert_eq!(v["scope"], "chat");
        assert_eq!(v["selectedSetName"], v["selected_set_name"]);
        assert_eq!(StQuickReplyConfig::from_value(&v).unwrap(), c);
    }

    #[test]
    fn slash_mapping_supported() {
        let m = map_command_to_museai_macro("/setvar style 简洁");
        assert_eq!(m.command.as_deref(), Some("setvar"));
        assert_eq!(m.museai_macro.as_deref(), Some("{{setvar::style::简洁}}"));
        assert!(m.unsupported.is_none());

        assert_eq!(
            map_command_to_museai_macro("/getvar style").museai_macro.as_deref(),
            Some("{{getvar::style}}")
        );
        assert_eq!(
            map_command_to_museai_macro("/getglobalvar g").museai_macro.as_deref(),
            Some("{{getglobalvar::g}}")
        );
        assert_eq!(
            map_command_to_museai_macro("/addvar log 一行").museai_macro.as_deref(),
            Some("{{addvar::log::一行}}")
        );
        assert_eq!(
            map_command_to_museai_macro("/user").museai_macro.as_deref(),
            Some("{{user}}")
        );
        assert_eq!(
            map_command_to_museai_macro("/char").museai_macro.as_deref(),
            Some("{{char}}")
        );
        assert_eq!(
            map_command_to_museai_macro("/lastmessage").museai_macro.as_deref(),
            Some("{{lastUserMessage}}")
        );
        assert_eq!(
            map_command_to_museai_macro("/MEMORY").museai_macro.as_deref(),
            Some("{{MEMORY}}")
        );
        // 大小写不敏感
        assert_eq!(
            map_command_to_museai_macro("/SetVar k v").command.as_deref(),
            Some("setvar")
        );
    }

    #[test]
    fn slash_mapping_unsupported_keeps_original() {
        for cmd in ["/newchat", "/?", "/bgcol blue", "/sysinfo", "/clear"] {
            let m = map_command_to_museai_macro(cmd);
            assert_eq!(m.museai_macro, None, "{cmd} 不该被伪造映射");
            assert_eq!(
                m.unsupported,
                Some(UnsupportedCommand::UnsupportedCommand),
                "{cmd} 应标 unsupported"
            );
            assert_eq!(
                m.unsupported.unwrap().as_str(),
                "unsupported_command"
            );
            assert_eq!(m.original, cmd, "原始文本必须保留，不静默丢弃");
            assert!(m.command.is_some());
        }
        assert_eq!(
            map_command_to_museai_macro("/?").command.as_deref(),
            Some("?")
        );
        assert_eq!(
            map_command_to_museai_macro("/bgcol blue").command.as_deref(),
            Some("bgcol")
        );
    }

    #[test]
    fn slash_mapping_missing_args_is_unsupported() {
        // 缺参数 → 不映射，绝不产出畸形宏
        for cmd in ["/setvar", "/addvar", "/getvar", "/getglobalvar"] {
            let m = map_command_to_museai_macro(cmd);
            assert_eq!(m.museai_macro, None, "{cmd} 缺参数不映射");
            assert_eq!(m.unsupported, Some(UnsupportedCommand::UnsupportedCommand));
        }
    }

    #[test]
    fn slash_mapping_plain_text() {
        let m = map_command_to_museai_macro("  记得 {{char}} 喝茶  ");
        assert!(m.command.is_none());
        assert_eq!(m.museai_macro.as_deref(), Some("记得 {{char}} 喝茶"));
        assert_eq!(m.unsupported, Some(UnsupportedCommand::PlainText));
    }

    #[test]
    fn qr_command_helpers() {
        let q = StQuickReply {
            message: "/bgcol #fff".into(),
            ..Default::default()
        };
        assert!(q.is_slash_command());
        assert_eq!(q.command_name().as_deref(), Some("bgcol"));
        assert_eq!(q.map_to_museai_macro().unsupported, Some(UnsupportedCommand::UnsupportedCommand));
        let plain = StQuickReply {
            message: "文本".into(),
            ..Default::default()
        };
        assert!(!plain.is_slash_command());
        assert_eq!(plain.command_name(), None);
    }

    #[test]
    fn count_and_extract_from_extension_settings() {
        let root = json!({
            "quick_reply": {
                "sets": {
                    "工作流": {
                        "version": 2,
                        "name": "工作流",
                        "qrList": [
                            {"label": "a", "message": "/getvar style"},
                            {"label": "b", "message": "/newchat"},
                            {"label": "c", "message": "普通文本"}
                        ]
                    }
                }
            },
            "other": {"x": 1}
        });
        let sets = sets_from_extension_settings(&root);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].name, "工作流");
        let (mapped, unsup, plain) = count_slash_mappability(&sets[0].qr_list);
        assert_eq!((mapped, unsup, plain), (1, 1, 1));
        // 非命令文本也可作宏模板直通；不可映射的 /newchat 不产出宏
        let mac = mappable_macros(&sets[0]);
        assert_eq!(
            mac,
            vec![
                ("a".to_string(), "{{getvar::style}}".to_string()),
                ("c".to_string(), "普通文本".to_string()),
            ]
        );
        // 非对象根 / 缺失键 → 空
        assert!(sets_from_extension_settings(&Value::Null).is_empty());
        assert!(sets_from_extension_settings(&json!({"unrelated": 1})).is_empty());
    }

    #[test]
    fn parse_set_json_robust() {
        let s = parse_set_json(r#"{"version":2,"name":"n","qrList":[{"message":"/newchat"}]}"#)
            .expect("parse");
        assert_eq!(s.name, "n");
        assert_eq!(s.qr_list[0].message, "/newchat");
        assert!(parse_set_json("not json").is_none());
        assert!(parse_set_json("[]").is_none());
    }
}
