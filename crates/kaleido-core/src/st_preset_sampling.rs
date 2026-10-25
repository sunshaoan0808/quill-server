//! # ST 预设采样/惩罚参数（Batch I1）
//!
//! `st_preset` 原本只吃 prompt 块 + 连接键，ST 的采样器全���（temp / top_p /
//! rep_pen / DRY / XTC …）一个都没接。结果就是「预设搬过来了，但手感不对」——
//! museai 后端在跑自己的默认采样参数，ST 侧调好的采样器完全没生效。
//!
//! 本模块把 ST `textgenerationwebui_settings`（TT
//! `src/scripts/textgen-settings.js:144-237`）里的采样/惩罚字段结构化。
//!
//! ## 设计约束
//!
//! 1. **全字段 `Option`** —— 缺字段 = 旧语义（调用方继续用自己的默认值），
//!    不改任何存量行为。
//! 2. **camel/snake 双写兼容** —— ST 生态里同一个概念出现过 `freq_pen` /
//!    `frequency_penalty` / `freqPen` 三种拼法。序列化用 ST 规范的 snake 名，
//!    反序列化同时接受 camel 与其它 snake 变体。
//! 3. **不静默丢弃** —— museai 后端不消费的 ST 独有参数（DRY / XTC / 采样器
//!    顺序 / 禁词表）原样保留在结构里，并置 `not_applicable_to_backend`，
//!    调用方据此提示「这批参数这次没生效」而不是以为已经生效。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 采样参数结构。字段全部 `Option`，缺失即「未设置」。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplingParams {
    // ─── 温度 / 分布形状 ───
    /// 温度。ST 键：`temp`（别名 `temperature` / `temperatureLast` 不适用）。
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "temp",
        alias = "temperature"
    )]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typical_p: Option<f64>,

    // ─── 惩罚 ───
    /// ST 键：`freq_pen`（OpenAI 风格写作 `frequency_penalty`）。
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "freq_pen",
        alias = "frequency_penalty",
        alias = "freqPen"
    )]
    pub frequency_penalty: Option<f64>,
    /// ST 键：`presence_pen`（OpenAI 风格写作 `presence_penalty`）。
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "presence_pen",
        alias = "presence_penalty",
        alias = "presencePen"
    )]
    pub presence_penalty: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rep_pen: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rep_pen_range: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rep_pen_decay: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rep_pen_slope: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoder_rep_pen: Option<f64>,

    // ─── 长度预算 ───
    /// ST 键：`openai_max_context`（别名 `truncation_length`）。
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "openai_max_context",
        alias = "truncation_length"
    )]
    pub openai_max_context: Option<i64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "openai_max_tokens",
        alias = "max_tokens"
    )]
    pub openai_max_tokens: Option<i64>,

    // ─── ST 独有：museai 后端不消费，原样保留 ───
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_multiplier: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_base: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_allowed_length: Option<i64>,
    /// TopP 采样器（TT 扩展，非标准参数）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topp_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xtc_threshold: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xtc_probability: Option<f64>,
    /// 采样器执行顺序（ST 数组，形如 `["top_k","top_p",…]`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samplers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samplers_priorities: Option<Vec<String>>,
    /// KoboldCpp 采样器顺序（数字数组）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sampler_order: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub banned_tokens: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_banned_tokens: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_banned_tokens: Option<bool>,

    /// 上述「ST 独有」字段中至少有一个被设置 → true。
    /// 调用方据此提示用户：这部分参数本次未被后端消费。
    #[serde(default, skip_serializing_if = "is_false")]
    pub not_applicable_to_backend: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// ST 规范键名（snake），用于「未消费参数」提示与回写。
const BACKEND_UNSUPPORTED_KEYS: &[&str] = &[
    "dry_multiplier",
    "dry_base",
    "dry_allowed_length",
    "topp",
    "topp_p",
    "xtc_threshold",
    "xtc_probability",
    "samplers",
    "samplers_priorities",
    "sampler_order",
    "banned_tokens",
    "global_banned_tokens",
    "send_banned_tokens",
];

/// 全部被本结构接住的 ST 采样键（不含别名）。
pub const SAMPLING_SETTING_KEYS: &[&str] = &[
    "temp",
    "temperature",
    "top_p",
    "top_k",
    "min_p",
    "typical_p",
    "freq_pen",
    "frequency_penalty",
    "presence_pen",
    "presence_penalty",
    "rep_pen",
    "rep_pen_range",
    "rep_pen_decay",
    "rep_pen_slope",
    "encoder_rep_pen",
    "openai_max_context",
    "openai_max_tokens",
    "dry_multiplier",
    "dry_base",
    "dry_allowed_length",
    "topp",
    "topp_p",
    "xtc_threshold",
    "xtc_probability",
    "samplers",
    "samplers_priorities",
    "sampler_order",
    "banned_tokens",
    "global_banned_tokens",
    "send_banned_tokens",
];

impl SamplingParams {
    /// 从预设 `settings` 对象解析采样参数。
    ///
    /// 未知字段一律忽略（不报错），非对象输入返回全空结构。
    /// 若 settings 整体不是对象，视为「旧语义」，返回默认空参数。
    pub fn from_settings(settings: &Value) -> Self {
        let obj = match settings.as_object() {
            Some(o) => o,
            None => return Self::default(),
        };

        // 数值容错：ST 预设里同一字段可能是数字、也可能是数字字符串。
        let num = |keys: &[&str]| -> Option<f64> { pick(obj, keys).and_then(as_f64) };
        let int = |keys: &[&str]| -> Option<i64> {
            pick(obj, keys).and_then(|v| as_f64(v).map(|f| f.round() as i64))
        };
        let str_ = |keys: &[&str]| -> Option<String> {
            pick(obj, keys).and_then(|v| v.as_str().map(|s| s.to_string()))
        };
        let bool_ = |keys: &[&str]| -> Option<bool> {
            pick(obj, keys).and_then(|v| match v {
                Value::Bool(b) => Some(*b),
                Value::String(s) => match s.trim().to_lowercase().as_str() {
                    "true" | "1" => Some(true),
                    "false" | "0" => Some(false),
                    _ => None,
                },
                _ => None,
            })
        };
        let strs = |keys: &[&str]| -> Option<Vec<String>> {
            pick(obj, keys).and_then(|v| v.as_array().map(|a| {
                a.iter()
                    .map(|x| match x {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect()
            }))
        };
        let ints = |keys: &[&str]| -> Option<Vec<i64>> {
            pick(obj, keys).and_then(|v| {
                v.as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| as_f64(x).map(|f| f.round() as i64))
                        .collect()
                })
            })
        };

        let p = Self {
            temperature: num(&["temp", "temperature"]),
            top_p: num(&["top_p", "topP"]),
            top_k: int(&["top_k", "topK"]),
            min_p: num(&["min_p", "minP"]),
            typical_p: num(&["typical_p", "typicalP"]),
            frequency_penalty: num(&["freq_pen", "frequency_penalty", "freqPen"]),
            presence_penalty: num(&["presence_pen", "presence_penalty", "presencePen"]),
            rep_pen: num(&["rep_pen", "repPen"]),
            rep_pen_range: int(&["rep_pen_range", "repPenRange"]),
            rep_pen_decay: num(&["rep_pen_decay", "repPenDecay"]),
            rep_pen_slope: num(&["rep_pen_slope", "repPenSlope"]),
            encoder_rep_pen: num(&["encoder_rep_pen", "encoderRepPen"]),
            openai_max_context: int(&["openai_max_context", "truncation_length", "openaiMaxContext"]),
            openai_max_tokens: int(&["openai_max_tokens", "max_tokens", "openaiMaxTokens"]),
            dry_multiplier: num(&["dry_multiplier", "dryMultiplier"]),
            dry_base: num(&["dry_base", "dryBase"]),
            dry_allowed_length: int(&["dry_allowed_length", "dryAllowedLength"]),
            topp: num(&["topp", "topP_sampler"]),
            topp_p: num(&["topp_p", "toppP"]),
            xtc_threshold: num(&["xtc_threshold", "xtcThreshold"]),
            xtc_probability: num(&["xtc_probability", "xtcProbability"]),
            samplers: strs(&["samplers"]),
            samplers_priorities: strs(&["samplers_priorities", "samplersPriorities"]),
            sampler_order: ints(&["sampler_order", "samplerOrder"]),
            banned_tokens: str_(&["banned_tokens", "bannedTokens"]),
            global_banned_tokens: str_(&["global_banned_tokens", "globalBannedTokens"]),
            send_banned_tokens: bool_(&["send_banned_tokens", "sendBannedTokens"]),
            not_applicable_to_backend: false,
        };

        let mut out = p;
        out.not_applicable_to_backend = !out.backend_unsupported_keys().is_empty();
        out
    }

    /// 便捷入口：直接从 settings 的 JSON 文本解析。
    ///
    /// # Errors
    /// 文本不是合法 JSON 时返回 serde 错误。
    pub fn from_settings_str(json_str: &str) -> Result<Self, serde_json::Error> {
        let v: Value = serde_json::from_str(json_str)?;
        Ok(Self::from_settings(&v))
    }

    /// 当前结构里被设置了、但 museai 后端不消费的 ST 键名。
    pub fn backend_unsupported_keys(&self) -> Vec<&'static str> {
        let has = |present: bool| present;
        let mut out = Vec::new();
        for key in BACKEND_UNSUPPORTED_KEYS {
            let present = match *key {
                "dry_multiplier" => has(self.dry_multiplier.is_some()),
                "dry_base" => has(self.dry_base.is_some()),
                "dry_allowed_length" => has(self.dry_allowed_length.is_some()),
                "topp" => has(self.topp.is_some()),
                "topp_p" => has(self.topp_p.is_some()),
                "xtc_threshold" => has(self.xtc_threshold.is_some()),
                "xtc_probability" => has(self.xtc_probability.is_some()),
                "samplers" => has(self.samplers.is_some()),
                "samplers_priorities" => has(self.samplers_priorities.is_some()),
                "sampler_order" => has(self.sampler_order.is_some()),
                "banned_tokens" => has(self.banned_tokens.is_some()),
                "global_banned_tokens" => has(self.global_banned_tokens.is_some()),
                "send_banned_tokens" => has(self.send_banned_tokens.is_some()),
                _ => false,
            };
            if present {
                out.push(*key);
            }
        }
        out
    }

    /// 是否全部为空（可用于「旧预设没有采样参数」判定）。
    pub fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.top_k.is_none()
            && self.min_p.is_none()
            && self.typical_p.is_none()
            && self.frequency_penalty.is_none()
            && self.presence_penalty.is_none()
            && self.rep_pen.is_none()
            && self.rep_pen_range.is_none()
            && self.rep_pen_decay.is_none()
            && self.rep_pen_slope.is_none()
            && self.encoder_rep_pen.is_none()
            && self.openai_max_context.is_none()
            && self.openai_max_tokens.is_none()
            && self.backend_unsupported_keys().is_empty()
    }

    /// 序列化为 ST 规范的 settings 片段（只含已设置项）。
    pub fn to_settings_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// 按候选键顺序取第一个存在的值。
fn pick<'a>(obj: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| obj.get(*k))
}

/// 数值容错解析：Number 或可解析的字符串。
fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn st_full_settings_parses_mapped_params() {
        let settings = json!({
            "temp": 0.85,
            "top_p": 0.92,
            "top_k": 40,
            "min_p": 0.05,
            "typical_p": 0.98,
            "freq_pen": 0.3,
            "presence_pen": 0.2,
            "rep_pen": 1.15,
            "rep_pen_range": 256,
            "rep_pen_decay": 0.9,
            "rep_pen_slope": 1.2,
            "encoder_rep_pen": 1.05,
            "openai_max_context": 8192,
            "openai_max_tokens": 512,
            "chat_completion_source": "openai",
            "openai_model": "gpt-x"
        });
        let p = SamplingParams::from_settings(&settings);
        assert_eq!(p.temperature, Some(0.85));
        assert_eq!(p.top_p, Some(0.92));
        assert_eq!(p.top_k, Some(40));
        assert_eq!(p.min_p, Some(0.05));
        assert_eq!(p.typical_p, Some(0.98));
        assert_eq!(p.frequency_penalty, Some(0.3));
        assert_eq!(p.presence_penalty, Some(0.2));
        assert_eq!(p.rep_pen, Some(1.15));
        assert_eq!(p.rep_pen_range, Some(256));
        assert_eq!(p.rep_pen_decay, Some(0.9));
        assert_eq!(p.rep_pen_slope, Some(1.2));
        assert_eq!(p.encoder_rep_pen, Some(1.05));
        assert_eq!(p.openai_max_context, Some(8192));
        assert_eq!(p.openai_max_tokens, Some(512));
        assert!(!p.not_applicable_to_backend);
        assert!(!p.is_empty());
    }

    #[test]
    fn missing_fields_means_old_semantics() {
        // 空对象 / 只有连接键的旧预设 → 全 None，不改存量行为
        let p = SamplingParams::from_settings(&json!({
            "chat_completion_source": "openai",
            "openai_model": "gpt-x"
        }));
        assert!(p.is_empty());
        assert!(!p.not_applicable_to_backend);
        assert_eq!(p.temperature, None);
        assert_eq!(p.top_k, None);

        // 非对象输入也不报错，退回旧语义
        let p2 = SamplingParams::from_settings(&json!(["not", "an", "object"]));
        assert!(p2.is_empty());
    }

    #[test]
    fn camel_and_snake_both_accepted() {
        let snake = SamplingParams::from_settings(&json!({
            "temperature": 0.7, "topP": 0.9, "topK": 30,
            "frequency_penalty": 0.1, "presence_penalty": 0.2
        }));
        let camel = SamplingParams::from_settings(&json!({
            "temp": 0.7, "top_p": 0.9, "top_k": 30,
            "freq_pen": 0.1, "presence_pen": 0.2
        }));
        assert_eq!(snake, camel);
        assert_eq!(snake.temperature, Some(0.7));
        assert_eq!(snake.top_p, Some(0.9));
        assert_eq!(snake.top_k, Some(30));
        assert_eq!(snake.frequency_penalty, Some(0.1));
        assert_eq!(snake.presence_penalty, Some(0.2));

        // 显式 serde 层的别名同样生效
        let via_serde: SamplingParams =
            serde_json::from_value(json!({"temp": 0.7, "freqPen": 0.1})).unwrap();
        assert_eq!(via_serde.temperature, Some(0.7));
        assert_eq!(via_serde.frequency_penalty, Some(0.1));
    }

    #[test]
    fn dry_xtc_samplers_retained_and_flagged() {
        let settings = json!({
            "temp": 1.0,
            "dry_multiplier": 0.8,
            "dry_base": 1.75,
            "dry_allowed_length": 2,
            "xtc_threshold": 0.1,
            "xtc_probability": 0.5,
            "samplers": ["top_k", "top_p", "temperature"],
            "samplers_priorities": ["top_p", "top_k"],
            "sampler_order": [6, 0, 1, 3],
            "banned_tokens": "abc",
            "global_banned_tokens": "xyz",
            "send_banned_tokens": true
        });
        let p = SamplingParams::from_settings(&settings);
        // 保留，不丢
        assert_eq!(p.dry_multiplier, Some(0.8));
        assert_eq!(p.dry_base, Some(1.75));
        assert_eq!(p.dry_allowed_length, Some(2));
        assert_eq!(p.xtc_threshold, Some(0.1));
        assert_eq!(p.xtc_probability, Some(0.5));
        assert_eq!(
            p.samplers.as_deref(),
            Some(["top_k".to_string(), "top_p".to_string(), "temperature".to_string()].as_slice())
        );
        assert_eq!(p.sampler_order.as_deref(), Some([6, 0, 1, 3].as_slice()));
        assert_eq!(p.banned_tokens.as_deref(), Some("abc"));
        assert_eq!(p.global_banned_tokens.as_deref(), Some("xyz"));
        assert_eq!(p.send_banned_tokens, Some(true));
        // 标注
        assert!(p.not_applicable_to_backend);
        let keys = p.backend_unsupported_keys();
        for k in [
            "dry_multiplier",
            "dry_base",
            "dry_allowed_length",
            "xtc_threshold",
            "xtc_probability",
            "samplers",
            "samplers_priorities",
            "sampler_order",
            "banned_tokens",
            "global_banned_tokens",
            "send_banned_tokens",
        ] {
            assert!(keys.contains(&k), "missing {k} in {keys:?}");
        }
    }

    #[test]
    fn roundtrip_loses_no_fields() {
        let settings = json!({
            "temp": 0.85, "top_p": 0.92, "top_k": 40, "min_p": 0.05, "typical_p": 0.98,
            "freq_pen": 0.3, "presence_pen": 0.2,
            "rep_pen": 1.15, "rep_pen_range": 256, "rep_pen_decay": 0.9,
            "rep_pen_slope": 1.2, "encoder_rep_pen": 1.05,
            "openai_max_context": 8192, "openai_max_tokens": 512,
            "dry_multiplier": 0.8, "dry_base": 1.75, "dry_allowed_length": 2,
            "topp": 0.5, "topp_p": 0.6,
            "xtc_threshold": 0.1, "xtc_probability": 0.5,
            "samplers": ["top_k", "top_p"],
            "samplers_priorities": ["top_p"],
            "sampler_order": [6, 0, 1],
            "banned_tokens": "abc", "global_banned_tokens": "xyz", "send_banned_tokens": true
        });
        let a = SamplingParams::from_settings(&settings);
        let value = a.to_settings_value();
        let b = SamplingParams::from_settings(&value);
        assert_eq!(a, b, "roundtrip lost fields: {value}");
        assert!(b.not_applicable_to_backend);
        // 序列化沿用 ST 规范 snake 键
        assert_eq!(value.get("temp"), Some(&json!(0.85)));
        assert_eq!(value.get("freq_pen"), Some(&json!(0.3)));
        assert!(value.get("not_applicable_to_backend") == Some(&json!(true)));
        // None 字段不落盘
        assert!(value.get("min_length").is_none());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let p = SamplingParams::from_settings(&json!({
            "temp": 0.6,
            "some_future_sampler": 42,
            "nested": {"a": 1},
            "logit_bias": [{"token": 1}]
        }));
        assert_eq!(p.temperature, Some(0.6));
        assert!(!p.not_applicable_to_backend);
        // 未知键不进序列化产物
        let v = p.to_settings_value();
        assert!(v.get("some_future_sampler").is_none());
        // serde 层也宽容
        let _: SamplingParams = serde_json::from_value(json!({"whatever": true})).unwrap();
    }

    #[test]
    fn numeric_strings_tolerated() {
        let p = SamplingParams::from_settings(&json!({
            "temp": "0.75", "top_k": "20", "send_banned_tokens": "true"
        }));
        assert_eq!(p.temperature, Some(0.75));
        assert_eq!(p.top_k, Some(20));
        assert_eq!(p.send_banned_tokens, Some(true));
    }

    #[test]
    fn from_settings_str_parses() {
        let p = SamplingParams::from_settings_str(r#"{"temp":0.5,"top_p":0.8}"#).unwrap();
        assert_eq!(p.temperature, Some(0.5));
        assert!(SamplingParams::from_settings_str("not json").is_err());
    }
}
