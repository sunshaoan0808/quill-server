//! st_world_info/types —— ST World Info 数据类型 + JSON 字段助手（R6③ 机械迁入）。
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SelectiveLogic {
    AndAny = 0,
    NotAll = 1,
    NotAny = 2,
    AndAll = 3,
}

impl SelectiveLogic {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::NotAll,
            2 => Self::NotAny,
            3 => Self::AndAll,
            _ => Self::AndAny,
        }
    }
}

/// ST `world_info_position` (subset we honour in prompt assembly)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum WiPosition {
    Before = 0,
    After = 1,
    AnTop = 2,
    AnBottom = 3,
    AtDepth = 4,
    EmTop = 5,
    EmBottom = 6,
    Outlet = 7,
}

impl WiPosition {
    pub fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::After,
            2 => Self::AnTop,
            3 => Self::AnBottom,
            4 => Self::AtDepth,
            5 => Self::EmTop,
            6 => Self::EmBottom,
            7 => Self::Outlet,
            _ => Self::Before,
        }
    }
}


#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterFilter {
    #[serde(default)]
    pub names: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub is_exclude: bool,
}

/// ST-like global scan context (trigger + persona/char names for filters & macros).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiScanContext {
    /// Generation trigger id (ST: normal, continue, impersonate, ...). Empty = no filter.
    #[serde(default)]
    pub trigger: String,
    /// Active character display name / filename for characterFilter.names
    #[serde(default)]
    pub character_name: String,
    /// Active character tags for characterFilter.tags
    #[serde(default)]
    pub character_tags: Vec<String>,
    /// Macro {{user}} / <USER>
    #[serde(default = "default_user")]
    pub user_name: String,
    /// Macro {{char}} / <BOT> / <CHAR>
    #[serde(default)]
    pub char_name: String,
    /// Optional max context override for budget (0 = caller max_context_tokens)
    #[serde(default)]
    pub max_context_tokens: i32,
    /// Pre-ranked vector hits from server embed path (W5). Keyed activation by world.uid.
    /// When non-empty, entries with `vectorized=true` matching a hit are activated
    /// with reason `vector:{score}` instead of being skipped.
    #[serde(default)]
    pub vector_hits: Vec<crate::VectorHit>,
    /// Optional override for vector activation knobs (threshold/top_k/enabled).
    #[serde(default)]
    pub vector_settings: Option<crate::VectorActivationSettings>,
}

pub(crate) fn default_user() -> String {
    "User".into()
}

/// Multi-slot prompt pieces matching ST PromptManager / extension_prompts layout.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiPromptSlots {
    pub world_info_before: String,
    pub world_info_after: String,
    pub an_before: String,
    pub an_after: String,
    pub em_before: String,
    pub em_after: String,
    pub depth_entries: Vec<WiDepthEntry>,
    pub outlet_entries: Vec<WiOutletEntry>,
    /// Ready-to-merge chat injections: insert as extra messages (role/content/depth).
    /// depth = messages from end (0 = after last). role: system|user|assistant
    #[serde(default)]
    pub chat_injections: Vec<WiChatInjection>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiChatInjection {
    pub role: String,
    pub content: String,
    /// 0 = newest side (append near end); higher = further back in history
    pub depth: i32,
    pub kind: String, // depth | em_before | em_after | outlet
}

#[derive(Debug, Clone)]
pub struct WiEntry {
    pub uid: String,
    pub world: String,
    pub keys: Vec<String>,
    pub keysecondary: Vec<String>,
    pub content: String,
    pub comment: String,
    pub constant: bool,
    pub disable: bool,
    pub selective: bool,
    pub selective_logic: SelectiveLogic,
    pub order: i32,
    pub position: WiPosition,
    pub depth: i32,
    pub probability: f64,
    pub use_probability: bool,
    pub scan_depth: Option<i32>,
    pub case_sensitive: Option<bool>,
    pub match_whole_words: Option<bool>,
    pub group: String,
    pub group_override: bool,
    pub group_weight: f64,
    pub use_group_scoring: Option<bool>,
    pub exclude_recursion: bool,
    pub prevent_recursion: bool,
    pub delay_until_recursion: bool,
    pub ignore_budget: bool,
    /// Stay active for N chat messages after activation (ST extensions.sticky).
    pub sticky: Option<i32>,
    /// Suppress re-activation for N messages (ST extensions.cooldown).
    pub cooldown: Option<i32>,
    /// Do not activate until chat has at least N messages (ST extensions.delay).
    pub delay: Option<i32>,
    /// Parsed from content lines starting with @@ (ST parseDecorators).
    pub decorators: Vec<String>,
    /// Named outlet when position=outlet (ST extensions.outlet_name).
    pub outlet_name: String,
    pub character_filter: Option<CharacterFilter>,
    /// ST extensions.triggers — generation type allow-list (empty = all).
    pub triggers: Vec<String>,
    /// ST extensions.vectorized — skipped in keyword scanner (needs embed path).
    pub vectorized: bool,
    /// ST extensions.automation_id — stored for extensions; not executed here.
    pub automation_id: String,
    /// atDepth message role (0=system,1=user,2=assistant) ST extension_prompt_roles
    pub role: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiSettings {
    /// Messages (from newest) to scan. ST default 2.
    #[serde(default = "default_depth")]
    pub depth: i32,
    /// Budget as % of max_context (ST default 25).
    #[serde(default = "default_budget_pct")]
    pub budget_pct: f64,
    /// Hard cap in *tokens* (0 = off).
    #[serde(default)]
    pub budget_cap: i32,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub match_whole_words: bool,
    #[serde(default)]
    pub use_group_scoring: bool,
    /// Min activations (0 = off).
    #[serde(default)]
    pub min_activations: i32,
    #[serde(default)]
    pub min_activations_depth_max: i32,
    /// Max recursion steps (0 = unlimited when recursive).
    #[serde(default)]
    pub max_recursion_steps: i32,
    /// W4: token estimate mode — `heuristic` (default) | `cl100k_approx`.
    #[serde(default)]
    pub token_estimate_mode: String,
}

pub(crate) fn default_depth() -> i32 {
    2
}
pub(crate) fn default_budget_pct() -> f64 {
    25.0
}

impl Default for WiSettings {
    fn default() -> Self {
        Self {
            depth: 2,
            budget_pct: 25.0,
            budget_cap: 0,
            recursive: false,
            case_sensitive: false,
            match_whole_words: false,
            use_group_scoring: false,
            min_activations: 0,
            min_activations_depth_max: 0,
            max_recursion_steps: 0,
            token_estimate_mode: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WiScanResult {
    pub world_info_before: String,
    pub world_info_after: String,
    /// Author's Note top (position ANTop=2)
    #[serde(default)]
    pub an_before: String,
    /// Author's Note bottom (position ANBottom=3)
    #[serde(default)]
    pub an_after: String,
    /// atDepth entries grouped: [{depth, role, content}]
    #[serde(default)]
    pub depth_entries: Vec<WiDepthEntry>,
    /// Example Message anchor — before (EMTop=5)
    #[serde(default)]
    pub em_before: String,
    /// Example Message anchor — after (EMBottom=6)
    #[serde(default)]
    pub em_after: String,
    /// Named outlets: [{name, content}]
    #[serde(default)]
    pub outlet_entries: Vec<WiOutletEntry>,
    pub activated: Vec<ActivatedEntry>,
    pub budget_tokens: i32,
    /// W4: which estimator was used for budget accounting.
    #[serde(default)]
    pub token_estimate_mode: String,
    pub overflowed: bool,
    /// Updated timed-effect metadata to persist (sticky/cooldown maps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_world_info: Option<TimedWorldInfo>,
    /// Structured multi-slot view (same data as fields above + chat_injections).
    #[serde(default)]
    pub prompt_slots: WiPromptSlots,
    #[serde(default)]
    pub skipped_vectorized: i32,
    /// How many vectorized entries were activated via embed path (W5).
    #[serde(default)]
    pub vector_activated: i32,
    #[serde(default)]
    pub skipped_filter: i32,
    #[serde(default)]
    pub skipped_trigger: i32,
    /// automationId values from activated entries (ST extension hook surface).
    #[serde(default)]
    pub automation_ids: Vec<String>,
    /// Parsed EM example message pairs (role/content), ST mesExamples style.
    #[serde(default)]
    pub example_messages: Vec<WiExampleMessage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiExampleMessage {
    pub role: String,
    pub content: String,
    /// before = EMTop, after = EMBottom
    #[serde(default)]
    pub anchor: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiOutletEntry {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiDepthEntry {
    pub depth: i32,
    pub role: String,
    pub content: String,
}

/// One sticky/cooldown interval — ST WITimedEffect (message-index based).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WiTimedEffect {
    pub key: String,
    pub start: i32,
    pub end: i32,
    #[serde(default)]
    pub protected: bool,
}

/// Per-chat timed WI state (ST chat_metadata.timedWorldInfo).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimedWorldInfo {
    #[serde(default)]
    pub sticky: std::collections::HashMap<String, WiTimedEffect>,
    #[serde(default)]
    pub cooldown: std::collections::HashMap<String, WiTimedEffect>,
}

impl TimedWorldInfo {
    pub fn entry_key(world: &str, uid: &str) -> String {
        format!("{world}.{uid}")
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivatedEntry {
    pub uid: String,
    pub world: String,
    pub comment: String,
    pub order: i32,
    pub position: i32,
    pub content: String,
    pub reason: String,
    /// ST extensions.automationId when present on the activated entry.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub automation_id: String,
}

// --- parsing helpers ---

pub(crate) fn as_str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect(),
        Some(Value::String(s)) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn bool_field(obj: &Value, keys: &[&str], default: bool) -> bool {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if let Some(b) = v.as_bool() {
                return b;
            }
            if let Some(n) = v.as_i64() {
                return n != 0;
            }
        }
    }
    default
}

pub(crate) fn i32_field(obj: &Value, keys: &[&str], default: i32) -> i32 {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if let Some(n) = v.as_i64() {
                return n as i32;
            }
            if let Some(n) = v.as_f64() {
                return n as i32;
            }
            if let Some(s) = v.as_str() {
                if let Ok(n) = s.parse::<i32>() {
                    return n;
                }
            }
        }
    }
    default
}

pub(crate) fn f64_field(obj: &Value, keys: &[&str], default: f64) -> f64 {
    for k in keys {
        if let Some(v) = obj.get(*k) {
            if let Some(n) = v.as_f64() {
                return n;
            }
            if let Some(n) = v.as_i64() {
                return n as f64;
            }
            if let Some(s) = v.as_str() {
                if let Ok(n) = s.parse::<f64>() {
                    return n;
                }
            }
        }
    }
    default
}

pub(crate) fn str_field(obj: &Value, keys: &[&str]) -> String {
    for k in keys {
        if let Some(s) = obj.get(*k).and_then(|v| v.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    String::new()
}
