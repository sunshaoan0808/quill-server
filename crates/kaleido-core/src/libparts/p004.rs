#![allow(unused_imports)]
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::Arc,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;
use super::*;

// --- Settings helpers ---

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicSettings {
    #[serde(default)]
    pub llm_model: String,
    #[serde(default)]
    pub model_interface: String,
    #[serde(default)]
    pub partner_chat_prompt: String,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    /// [酒馆对齐] 采样参数: topP / frequencyPenalty / presencePenalty (与 temperature 同级存储)
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub frequency_penalty: Option<f64>,
    #[serde(default)]
    pub presence_penalty: Option<f64>,
    /// Masked display only — never full secret in API responses.
    #[serde(default)]
    pub llm_api_key: String,
    #[serde(default)]
    pub llm_api_key_configured: bool,
    /// OpenAI-compatible base URL (editable from web).
    #[serde(default)]
    pub llm_base_url: String,
    #[serde(default)]
    pub llm_base_url_configured: bool,
    /// Fanqie crawler switch (S5-W1 T5). Default false — never auto-enable.
    #[serde(default)]
    pub crawler_enabled: bool,
    /// Agent tools bash sandbox gate (S5-W1 T4). Default false.
    #[serde(default)]
    pub bash_sandbox_enabled: bool,
    /// Agent tools read/list/grep/glob gate. Default true.
    #[serde(default = "default_true_flag")]
    pub agent_tools_enabled: bool,
    /// Agent write/edit gate (W10). Default **false** — dangerous tools off until toggled.
    #[serde(default)]
    pub agent_write_enabled: bool,
    /// When true (default), write/edit/bash REST calls require `confirmDangerous: true`.
    #[serde(default = "default_true_flag")]
    pub agent_confirm_dangerous: bool,
    /// W4: token estimate mode for WI budget / estimate API.
    /// `heuristic` (default) | `cl100k_approx`.
    #[serde(default)]
    pub token_estimate_mode: String,
    /// W12: max concurrent auth sessions (optional; live from AuthStore if unset).
    #[serde(default)]
    pub session_max: Option<u64>,
    /// W12: `auto_evict` | `reject`.
    #[serde(default)]
    pub session_cap_policy: String,
    /// Story Tavern adult content confirmation (server-persisted, survives browser reset).
    #[serde(default)]
    pub tavern_adult_ok: bool,
}

/// Runtime LLM credentials resolved from settings-store + secrets + env fallback.
#[derive(Debug, Clone)]
pub struct LlmRuntime {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// P5 call-side: set when the runtime resolved from a managed ai_admin
    /// provider (enabled provider + model). None for settings/env fallback.
    pub provider_id: Option<String>,
    /// G6: protocol of the resolved runtime ("openai"|"anthropic"|"google").
    /// Empty string = env fallback → use the KALEIDO_LLM_PROVIDER default.
    pub provider_kind: String,
    /// P5 call-side: true when the provider's per-minute RPM window is full and
    /// the call must be rejected (base_url is emptied so callers fail fast).
    pub rpm_hit: bool,
    pub rpm_retry_secs: u64,
}

/// In-memory per-provider RPM limiter (sliding 60s window), shared process-wide
/// so all resolve_llm call sites enforce the same budget for a provider.
/// Not persisted by design: the window resets on restart.
#[derive(Clone, Default)]
pub struct RpmLimiter {
    inner: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<std::time::Instant>>>>,
}

pub(crate) static RPM_GLOBAL: std::sync::OnceLock<RpmLimiter> = std::sync::OnceLock::new();

impl RpmLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process-wide singleton used by resolve_llm call-side integration.
    pub fn global() -> &'static RpmLimiter {
        RPM_GLOBAL.get_or_init(RpmLimiter::default)
    }

    /// Try to acquire one slot for `provider_id` within `rpm` per minute.
    /// Ok(remaining) on success; Err(retry_after_secs) when the window is full.
    pub fn try_acquire(&self, provider_id: &str, rpm: u32) -> Result<u32, u64> {
        if rpm == 0 {
            return Ok(0);
        }
        let now = std::time::Instant::now();
        let mut m = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let v = m.entry(provider_id.to_string()).or_default();
        v.retain(|t| now.duration_since(*t).as_secs() < 60);
        if (v.len() as u32) < rpm {
            v.push_back(now);
            Ok(rpm - v.len() as u32)
        } else {
            let oldest = v.front().copied().unwrap_or(now);
            let wait = now.duration_since(oldest).as_secs().saturating_add(1);
            Err(wait)
        }
    }

    /// Drop recorded slots (used when a provider is deleted).
    pub fn reset(&self, provider_id: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(provider_id);
        }
    }
}

pub(crate) fn mask_api_key(key: &str) -> String {
    let k = key.trim();
    if k.is_empty() {
        return String::new();
    }
    if k == "[server]" || k.contains('•') {
        return k.to_string();
    }
    let chars: Vec<char> = k.chars().collect();
    if chars.len() <= 8 {
        return format!("{}•••", chars.first().map(|c| c.to_string()).unwrap_or_default());
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars.iter().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}•••{tail}")
}

pub(crate) fn default_true_flag() -> bool {
    true
}

impl AppStateStore {
    pub fn load_settings_public(&self) -> CoreResult<PublicSettings> {
        let raw = self.load("settings-store")?;
        let v: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
        let state = v.get("state").cloned().unwrap_or(v);
        let partner = state
            .get("partnerChatPrompt")
            .and_then(|x| x.as_str())
            .unwrap_or("你是一个体贴温和的伴侣，请用温暖、真实而细节丰富的语言与用户交谈，避免机器感。")
            .to_string();
        let mut model = state
            .get("llmModel")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let agent = state
            .get("agentConfigs")
            .and_then(|x| x.get("partnerChat"));
        let mut base_url = state
            .get("llmBaseUrl")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let secret_path = self.data_root().root().join("secrets").join("llm_api_key.txt");
        let secret_key = fs::read_to_string(&secret_path)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let mut key_configured = !secret_key.is_empty();
        let mut key_display = if key_configured {
            mask_api_key(&secret_key)
        } else {
            String::new()
        };
        // [酒馆对齐] LLM 连接字段以 active provider 为准 (settings-store 退役,
        // 仅在无 provider 时回退 legacy 值)。避免重复打开 sqlite: 一次派生。
        {
            let derived = crate::ai_admin_store::AiAdminStore::open(&self.data_root().root().join("plot.sqlite"))
                .and_then(|s| s.active_provider())
                .ok()
                .flatten();
            if let Some(p) = derived {
                if !p.base_url.is_empty() {
                    // keep legacy base_url when provider base empty (rare)
                    base_url = p.base_url;
                }
                let d_model = p.default_model_id.clone();
                let d_model_name = if !d_model.is_empty() {
                    crate::ai_admin_store::AiAdminStore::open(&self.data_root().root().join("plot.sqlite"))
                        .and_then(|s| s.get_model(&d_model))
                        .map(|m| m.model_id)
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                if !d_model_name.is_empty() {
                    model = d_model_name;
                }
                if p.configured {
                    key_configured = true;
                    key_display = p.key_hint.clone();
                }
            }
        }
        Ok(PublicSettings {
            llm_model: model,
            model_interface: state
                .get("modelInterface")
                .and_then(|x| x.as_str())
                .unwrap_or("OpenAI")
                .to_string(),
            partner_chat_prompt: partner,
            temperature: agent
                .and_then(|a| a.get("temperature"))
                .and_then(|x| x.as_f64()),
            max_output_tokens: agent
                .and_then(|a| a.get("maxOutputTokens"))
                .and_then(|x| x.as_u64()),
            // [酒馆对齐] 采样参数 (与 temperature 同级存储于 agentConfigs.partnerChat)
            top_p: agent
                .and_then(|a| a.get("topP"))
                .and_then(|x| x.as_f64()),
            frequency_penalty: agent
                .and_then(|a| a.get("frequencyPenalty"))
                .and_then(|x| x.as_f64()),
            presence_penalty: agent
                .and_then(|a| a.get("presencePenalty"))
                .and_then(|x| x.as_f64()),
            llm_api_key: key_display,
            llm_api_key_configured: key_configured,
            llm_base_url: base_url.clone(),
            llm_base_url_configured: !base_url.is_empty(),
            crawler_enabled: state
                .get("crawlerEnabled")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            bash_sandbox_enabled: state
                .get("bashSandboxEnabled")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            agent_tools_enabled: state
                .get("agentToolsEnabled")
                .and_then(|x| x.as_bool())
                .unwrap_or(true),
            agent_write_enabled: state
                .get("agentWriteEnabled")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
            agent_confirm_dangerous: state
                .get("agentConfirmDangerous")
                .and_then(|x| x.as_bool())
                .unwrap_or(true),
            token_estimate_mode: state
                .get("tokenEstimateMode")
                .and_then(|x| x.as_str())
                .unwrap_or("heuristic")
                .to_string(),
            session_max: state
                .get("sessionMax")
                .or_else(|| state.get("maxSessions"))
                .and_then(|x| x.as_u64()),
            session_cap_policy: state
                .get("sessionCapPolicy")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            tavern_adult_ok: state
                .get("tavernAdultOk")
                .and_then(|x| x.as_bool())
                .unwrap_or(false),
        })
    }

    /// Resolve LLM endpoint for chat/jobs.
    /// Priority: active managed provider (ai_providers.active=1) → 懒迁移
    /// (settings-store 残留 llmBaseUrl → 建 active provider) → env fallbacks.
    /// [酒馆对齐] settings-store 的 llmBaseUrl/llmModel/llmApiKey 不再直接消费。
    pub fn resolve_llm(
        &self,
        env_base: Option<&str>,
        env_key: Option<&str>,
        env_model: &str,
    ) -> LlmRuntime {
        // P5: managed provider (active/enabled provider + enabled model) wins so
        // the admin UI can switch providers instantly (酒馆对齐: active 指针)。
        if let Some(rt) = self.resolve_from_provider() {
            return rt;
        }
        // 懒迁移: 老安装 settings-store 还残留 llmBaseUrl 时, 迁移成 active
        // provider 再重试一次 (成功后后续都走 provider 分支)。
        if self.migrate_legacy_llm() {
            if let Some(rt) = self.resolve_from_provider() {
                return rt;
            }
        }
        LlmRuntime {
            base_url: env_base
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default(),
            api_key: env_key
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_default(),
            model: env_model.trim().to_string(),
            provider_id: None,
            provider_kind: String::new(),
            rpm_hit: false,
            rpm_retry_secs: 0,
        }
    }

    /// Try the managed-provider path (active/enabled) with per-provider RPM gating.
    fn resolve_from_provider(&self) -> Option<LlmRuntime> {
        let rt = crate::ai_admin_store::AiAdminStore::open(&self.data_root().root().join("plot.sqlite"))
            .and_then(|s| s.resolve_call_runtime())
            .ok()
            .flatten()?;
        // P5 RPM: enforce per-provider per-minute budget across every
        // resolve_llm call site. When full, empty the base_url so callers
        // fail fast, and surface the retry window for friendly 429s.
        let (rpm_hit, rpm_retry_secs) =
            match RpmLimiter::global().try_acquire(&rt.provider_id, rt.rpm_limit) {
                Ok(_) => (false, 0),
                Err(wait) => {
                    tracing::warn!(
                        provider_id = %rt.provider_id,
                        rpm = rt.rpm_limit,
                        retry_after_secs = wait,
                        "LLM provider RPM limit hit"
                    );
                    (true, wait)
                }
            };
        Some(LlmRuntime {
            base_url: if rpm_hit { String::new() } else { rt.base_url },
            api_key: rt.api_key,
            model: rt.model,
            provider_id: Some(rt.provider_id),
            provider_kind: rt.protocol,
            rpm_hit,
            rpm_retry_secs,
        })
    }

    /// [酒馆对齐] 懒迁移: settings-store 残留 llmBaseUrl/llmModel/llmApiKey 且
    /// 无 active provider 时, 自动建成 active provider (只做一次——成功后走
    /// provider 分支)。有 provider 但无 active 时, 挑第一个 enabled 激活。
    fn migrate_legacy_llm(&self) -> bool {
        let store = match crate::ai_admin_store::AiAdminStore::open(&self.data_root().root().join("plot.sqlite")) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let providers = match store.list_providers() {
            Ok(p) => p,
            Err(_) => return false,
        };
        if providers.iter().any(|p| p.active) {
            return false; // 已有 active, 无需迁移
        }
        if !providers.is_empty() {
            // 无 active 但有 provider: 挑第一个 enabled (否则第一个) 激活
            let pick = providers
                .iter()
                .find(|p| p.status == "enabled")
                .or_else(|| providers.first());
            if let Some(p) = pick {
                return store.set_active_provider(&p.id).is_ok();
            }
        }
        // 无 provider: 从 settings-store 迁移
        let raw = self.load("settings-store").unwrap_or_default();
        let v: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
        let state = v.get("state").cloned().unwrap_or(v);
        let base = state
            .get("llmBaseUrl")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let Some(base) = base else { return false };
        let secret_path = self.data_root().root().join("secrets").join("llm_api_key.txt");
        let key = fs::read_to_string(&secret_path)
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let model = state
            .get("llmModel")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_default();
        let iface = state
            .get("modelInterface")
            .and_then(|x| x.as_str())
            .unwrap_or("OpenAI");
        let proto = if iface.to_ascii_lowercase().contains("anthropic") {
            "anthropic"
        } else if iface.to_ascii_lowercase().contains("google") {
            "google"
        } else {
            "openai"
        };
        let p = match store.create_provider("默认供应商", &base, proto, &key, 10, 60, 32000, "从设置页自动迁移") {
            Ok(p) => p,
            Err(_) => return false,
        };
        if store.set_active_provider(&p.id).is_err() {
            return false;
        }
        if !model.is_empty() {
            if let Ok(m) = store.create_model(&p.id, &model, &model, &["chat"], 128000, true, true, "设置页指定") {
                let _ = store.set_default_model(&p.id, &m.id);
            }
        }
        true
    }

    pub fn patch_settings_public(&self, patch: &Value) -> CoreResult<PublicSettings> {
        let raw = self.load("settings-store")?;
        let mut root: Value = serde_json::from_str(&raw).unwrap_or(json!({"state":{}}));
        if !root.get("state").map(|s| s.is_object()).unwrap_or(false) {
            root = json!({"state": root});
        }
        let state = root
            .get_mut("state")
            .and_then(|s| s.as_object_mut())
            .ok_or_else(|| CoreError::BadRequest("settings state missing".into()))?;

        if let Some(p) = patch.get("partnerChatPrompt").and_then(|x| x.as_str()) {
            state.insert("partnerChatPrompt".into(), json!(p));
        }
        // [酒馆对齐] LLM 连接字段 (llmModel/llmBaseUrl/llmApiKey/modelInterface)
        // 转发到 active provider —— settings-store 不再持有连接配置。
        let llm_conn_touched = ["llmModel", "llmBaseUrl", "llmApiKey", "modelInterface"]
            .iter()
            .any(|k| patch.get(*k).is_some());
        if llm_conn_touched {
            self.apply_llm_patch_to_provider(patch)?;
        }
        if let Some(temp) = patch.get("temperature").and_then(|x| x.as_f64()) {
            let agents = state
                .entry("agentConfigs".to_string())
                .or_insert_with(|| json!({}));
            let obj = agents.as_object_mut().unwrap();
            let pc = obj
                .entry("partnerChat".to_string())
                .or_insert_with(|| json!({}));
            pc.as_object_mut()
                .unwrap()
                .insert("temperature".into(), json!(temp));
        }
        if let Some(mt) = patch.get("maxOutputTokens").and_then(|x| x.as_u64()) {
            let agents = state
                .entry("agentConfigs".to_string())
                .or_insert_with(|| json!({}));
            let obj = agents.as_object_mut().unwrap();
            let pc = obj
                .entry("partnerChat".to_string())
                .or_insert_with(|| json!({}));
            pc.as_object_mut()
                .unwrap()
                .insert("maxOutputTokens".into(), json!(mt));
        }
        // [酒馆对齐] 采样参数: topP / frequencyPenalty / presencePenalty
        for name in ["topP", "frequencyPenalty", "presencePenalty"] {
            if let Some(v) = patch.get(name).and_then(|x| x.as_f64()) {
                let agents = state
                    .entry("agentConfigs".to_string())
                    .or_insert_with(|| json!({}));
                let obj = agents.as_object_mut().unwrap();
                let pc = obj
                    .entry("partnerChat".to_string())
                    .or_insert_with(|| json!({}));
                pc.as_object_mut().unwrap().insert(name.into(), json!(v));
            }
        }
        if let Some(en) = patch.get("crawlerEnabled").and_then(|x| x.as_bool()) {
            state.insert("crawlerEnabled".into(), json!(en));
        }
        // S5-W1 T4 agent tools flags
        if let Some(b) = patch.get("bashSandboxEnabled").and_then(|x| x.as_bool()) {
            state.insert("bashSandboxEnabled".into(), json!(b));
        }
        if let Some(b) = patch.get("agentToolsEnabled").and_then(|x| x.as_bool()) {
            state.insert("agentToolsEnabled".into(), json!(b));
        }
        if let Some(b) = patch.get("agentWriteEnabled").and_then(|x| x.as_bool()) {
            state.insert("agentWriteEnabled".into(), json!(b));
        }
        if let Some(b) = patch.get("agentConfirmDangerous").and_then(|x| x.as_bool()) {
            state.insert("agentConfirmDangerous".into(), json!(b));
        }
        if let Some(m) = patch.get("tokenEstimateMode").and_then(|x| x.as_str()) {
            let mode = crate::TokenEstimateMode::parse(m);
            state.insert("tokenEstimateMode".into(), json!(mode.as_str()));
        }
        if let Some(n) = patch
            .get("sessionMax")
            .or_else(|| patch.get("maxSessions"))
            .and_then(|x| x.as_u64())
        {
            if n >= 1 {
                state.insert("sessionMax".into(), json!(n.min(10_000)));
            }
        }
        if let Some(p) = patch.get("sessionCapPolicy").and_then(|x| x.as_str()) {
            let pol = match p.trim().to_ascii_lowercase().as_str() {
                "reject" | "hard" | "fail" => "reject",
                _ => "auto_evict",
            };
            state.insert("sessionCapPolicy".into(), json!(pol));
        }
        if let Some(b) = patch.get("tavernAdultOk").and_then(|x| x.as_bool()) {
            state.insert("tavernAdultOk".into(), json!(b));
        }
        self.save("settings-store", &serde_json::to_string_pretty(&root)?)?;
        self.load_settings_public()
    }

    /// [酒馆对齐] settings 的 llm* 字段 → active provider 薄封装。
    /// 目标 provider: active → 第一个 enabled → 新建 (auto-active)。
    /// llmModel 匹配 provider 下 model_id; 无匹配则自动建模型并设为默认。
    fn apply_llm_patch_to_provider(&self, patch: &Value) -> CoreResult<()> {
        let base = patch
            .get("llmBaseUrl")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string());
        let key = patch
            .get("llmApiKey")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string());
        let model = patch
            .get("llmModel")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string());
        let iface = patch
            .get("modelInterface")
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string());
        let store = crate::ai_admin_store::AiAdminStore::open(&self.data_root().root().join("plot.sqlite"))
            .map_err(|e| CoreError::BadRequest(format!("ai store: {e}")))?;

        // 目标 provider: active → 第一个 enabled → 新建
        let pid = match store.active_provider() {
            Ok(Some(p)) => p.id,
            _ => {
                let existing = store
                    .list_providers()
                    .map_err(|e| CoreError::BadRequest(format!("{e}")))?;
                let pick = existing
                    .iter()
                    .find(|p| p.status == "enabled")
                    .or_else(|| existing.first());
                match pick {
                    Some(p) => p.id.clone(),
                    None => {
                        let url = base.clone().unwrap_or_default();
                        if url.is_empty() {
                            return Ok(()); // 无可建内容
                        }
                        let proto = if iface.as_deref().map(|i| i.to_ascii_lowercase().contains("anthropic")).unwrap_or(false) {
                            "anthropic"
                        } else if iface.as_deref().map(|i| i.to_ascii_lowercase().contains("google")).unwrap_or(false) {
                            "google"
                        } else {
                            "openai"
                        };
                        let p = store
                            .create_provider(
                                "默认供应商",
                                &url,
                                proto,
                                key.as_deref().unwrap_or(""),
                                10,
                                60,
                                32000,
                                "设置页创建",
                            )
                            .map_err(|e| CoreError::BadRequest(format!("{e}")))?;
                        let _ = store.set_active_provider(&p.id);
                        p.id
                    }
                }
            }
        };

        // base_url / api_key 更新 (SSRF 校验在 store.update_provider 内)
        if base.is_some() || key.is_some() {
            let cur = store
                .get_provider(&pid)
                .map_err(|e| CoreError::BadRequest(format!("{e}")))?;
            let new_url = base.clone().unwrap_or(cur.base_url.clone());
            let new_key = match key.as_deref() {
                Some(k) if !k.is_empty() && k != "[server]" && !k.contains('•') => k.to_string(),
                _ => "keep".to_string(),
            };
            store
                .update_provider(&pid, None, Some(&new_url), Some(&new_key), None, None, None, None)
                .map_err(|e| CoreError::BadRequest(format!("{e}")))?;
        }
        // llmModel → provider 下 model_id; 无匹配则建模型并设为默认
        if let Some(m) = model {
            if !m.is_empty() {
                let models = store
                    .list_models(&pid)
                    .map_err(|e| CoreError::BadRequest(format!("{e}")))?;
                match models.iter().find(|mm| mm.model_id == m) {
                    Some(mm) => {
                        store
                            .set_default_model(&pid, &mm.id)
                            .map_err(|e| CoreError::BadRequest(format!("{e}")))?;
                    }
                    None => {
                        if let Ok(nm) = store.create_model(&pid, &m, &m, &["chat"], 128000, true, true, "设置页指定") {
                            let _ = store.set_default_model(&pid, &nm.id);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn partner_chat_prompt(&self) -> String {
        self.load_settings_public()
            .map(|s| s.partner_chat_prompt)
            .unwrap_or_else(|_| {
                "你是一个体贴温和的伴侣，请用温暖、真实而细节丰富的语言与用户交谈，避免机器感。"
                    .into()
            })
    }
}

// --- Works filesystem (S4): path-jailed text tree under works/{workspace_id} ---

/// Max text file size accepted by WorksFs (2 MiB).
pub const WORKS_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Max image payload for works image-data-url (same cap as text).
pub const WORKS_MAX_IMAGE_BYTES: u64 = WORKS_MAX_FILE_BYTES;
/// Max list recursion depth exposed by GET /works (server clamps to this).
pub const WORKS_MAX_LIST_DEPTH: u32 = 8;
/// Default list depth when query omits depth.
pub const WORKS_DEFAULT_LIST_DEPTH: u32 = 2;

/// Public limits + directory convention snapshot (W11).
pub fn works_limits_public() -> Value {
    json!({
        "ok": true,
        "maxFileBytes": WORKS_MAX_FILE_BYTES,
        "maxImageBytes": WORKS_MAX_IMAGE_BYTES,
        "maxListDepth": WORKS_MAX_LIST_DEPTH,
        "defaultListDepth": WORKS_DEFAULT_LIST_DEPTH,
        "jail": "works/{workspaceId}",
        "textOnly": true,
        "parentsMustExist": true,
        "conventions": {
            "book-travel": "BookTravel pipeline artifacts (md/json under book-travel/…)",
            "book-travel/steps": "Per-step BookTravel dumps",
            "outline": "Outline module drafts",
            "crawler": "Crawler scratch under works (novel_workspace is separate root)",
            "author": "Author-zone drafts when present",
        },
        "codes": [
            "WORKS_FILE_TOO_LARGE",
            "WORKS_CONTENT_TOO_LARGE",
            "WORKS_APPEND_TOO_LARGE",
            "WORKS_PARENT_MISSING",
            "WORKS_BINARY_REJECTED",
            "WORKS_NOT_UTF8",
            "WORKS_NOT_FILE",
            "WORKS_IS_DIR",
            "WORKS_PATH_ESCAPE",
            "WORKS_PATH_TRAVERSAL",
            "WORKS_ABSOLUTE_PATH",
            "WORKS_NOT_FOUND",
            "WORKS_DIR_NOT_EMPTY",
            "WORKS_ROOT_FORBIDDEN",
            "WORKS_LIST_NOT_DIR",
            "WORKS_BINARY_CONTENT",
            "WORKS_INVALID_PATH",
            "WORKS_INVALID_WORKSPACE",
        ]
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorksEntry {
    pub path: String,
    pub name: String,
    pub kind: String, // "file" | "dir"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<WorksEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorksStat {
    pub path: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<u64>,
    pub is_text: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorksFileBody {
    pub path: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

/// Path-jailed filesystem rooted at `$KALEIDO_DATA/works/{workspace_id}`.
#[derive(Clone)]
pub struct WorksFs {
    data: DataRoot,
}

impl WorksFs {
    pub fn new(data: DataRoot) -> Self {
        Self { data }
    }

    pub fn workspace_root(&self, workspace_id: &str) -> CoreResult<PathBuf> {
        validate_workspace_id(workspace_id)?;
        let root = self.data.root().join("works").join(workspace_id);
        fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn jail_canonical(&self, workspace_id: &str) -> CoreResult<PathBuf> {
        let root = self.workspace_root(workspace_id)?;
        fs::canonicalize(&root).map_err(|e| {
            CoreError::Io(io::Error::new(
                e.kind(),
                format!("canonicalize jail: {e}"),
            ))
        })
    }

    /// Resolve a relative works path into an absolute path that is inside the jail.
    /// `for_create`: when true, the leaf may not exist yet (parent must exist or be creatable).
    pub fn resolve(
        &self,
        workspace_id: &str,
        rel: &str,
        for_create: bool,
    ) -> CoreResult<PathBuf> {
        let rel = normalize_rel_path(rel)?;
        let jail = self.jail_canonical(workspace_id)?;
        if rel.is_empty() || rel == "." {
            return Ok(jail);
        }

        let candidate = jail.join(&rel);
        if for_create {
            // Canonicalize the deepest existing ancestor, then re-join remaining components.
            let mut existing = candidate.as_path();
            let mut missing: Vec<std::ffi::OsString> = Vec::new();
            loop {
                if existing.exists() {
                    break;
                }
                match existing.file_name() {
                    Some(name) => {
                        missing.push(name.to_os_string());
                        existing = existing
                            .parent()
                            .ok_or_else(|| CoreError::works_path_escape("path escape"))?;
                    }
                    None => {
                        return Err(CoreError::works_path_escape("path escape"));
                    }
                }
            }
            let mut resolved = fs::canonicalize(existing).map_err(|e| {
                CoreError::Io(io::Error::new(e.kind(), format!("canonicalize parent: {e}")))
            })?;
            ensure_under_jail(&resolved, &jail)?;
            for part in missing.into_iter().rev() {
                // Reject sneaky components even after join
                let s = part.to_string_lossy();
                if s == ".." || s == "." || s.contains('\0') {
                    return Err(CoreError::coded(
                        "WORKS_INVALID_PATH",
                        "invalid path component",
                    ));
                }
                resolved.push(part);
            }
            ensure_under_jail(&resolved, &jail)?;
            Ok(resolved)
        } else {
            if !candidate.exists() {
                // Still reject escapes even when missing (use parent resolution)
                let _ = self.resolve(workspace_id, &rel, true)?;
                return Err(CoreError::works_not_found(rel));
            }
            let resolved = fs::canonicalize(&candidate).map_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    CoreError::works_not_found(rel)
                } else {
                    CoreError::Io(e)
                }
            })?;
            ensure_under_jail(&resolved, &jail)?;
            Ok(resolved)
        }
    }

    pub fn list(
        &self,
        workspace_id: &str,
        path: &str,
        depth: u32,
    ) -> CoreResult<WorksEntry> {
        let jail = self.jail_canonical(workspace_id)?;
        let abs = self.resolve(workspace_id, path, false)?;
        let meta = fs::metadata(&abs)?;
        if !meta.is_dir() {
            return Err(CoreError::coded(
                "WORKS_LIST_NOT_DIR",
                "list requires a directory",
            ));
        }
        let rel = display_rel(&abs, &jail)?;
        self.list_entry(&abs, &rel, depth, &jail)
    }

    fn list_entry(
        &self,
        abs: &Path,
        rel: &str,
        depth: u32,
        jail: &Path,
    ) -> CoreResult<WorksEntry> {
        // Resolve through symlinks; reject escapes.
        let abs = if abs.exists() {
            let canon = fs::canonicalize(abs).map_err(CoreError::Io)?;
            ensure_under_jail(&canon, jail)?;
            canon
        } else {
            return Err(CoreError::works_not_found(rel));
        };
        let name = if rel.is_empty() {
            "".into()
        } else {
            abs.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        let meta = fs::metadata(&abs)?;
        let mut entry = WorksEntry {
            path: if rel.is_empty() {
                "".into()
            } else {
                rel.into()
            },
            name,
            kind: if meta.is_dir() {
                "dir".into()
            } else {
                "file".into()
            },
            size: if meta.is_file() {
                Some(meta.len())
            } else {
                None
            },
            modified_ms: system_time_ms(meta.modified().ok()),
            children: Vec::new(),
        };
        if meta.is_dir() && depth > 0 {
            let mut children = Vec::new();
            let mut entries: Vec<_> = fs::read_dir(&abs)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let child_name = e.file_name().to_string_lossy().to_string();
                if child_name == "." || child_name == ".." {
                    continue;
                }
                let child_abs = e.path();
                // Symlink out of jail → skip
                if let Ok(canon) = fs::canonicalize(&child_abs) {
                    if ensure_under_jail(&canon, jail).is_err() {
                        continue;
                    }
                } else {
                    continue;
                }
                let child_rel = if rel.is_empty() {
                    child_name
                } else {
                    format!("{rel}/{child_name}")
                };
                match self.list_entry(&child_abs, &child_rel, depth.saturating_sub(1), jail) {
                    Ok(c) => children.push(c),
                    Err(_) => continue,
                }
            }
            entry.children = children;
        }
        Ok(entry)
    }

    pub fn stat(&self, workspace_id: &str, path: &str) -> CoreResult<WorksStat> {
        let abs = self.resolve(workspace_id, path, false)?;
        let meta = fs::metadata(&abs)?;
        let jail = self.jail_canonical(workspace_id)?;
        let rel = display_rel(&abs, &jail)?;
        let name = if rel.is_empty() {
            "".into()
        } else {
            abs.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        let is_file = meta.is_file();
        let is_text = if is_file {
            is_probably_text(&abs, meta.len())
        } else {
            true
        };
        Ok(WorksStat {
            path: rel,
            name,
            kind: if meta.is_dir() {
                "dir".into()
            } else if meta.is_file() {
                "file".into()
            } else {
                "other".into()
            },
            size: if is_file { Some(meta.len()) } else { None },
            modified_ms: system_time_ms(meta.modified().ok()),
            is_text,
        })
    }

    pub fn read_text(&self, workspace_id: &str, path: &str) -> CoreResult<WorksFileBody> {
        let abs = self.resolve(workspace_id, path, false)?;
        let meta = fs::metadata(&abs)?;
        if !meta.is_file() {
            return Err(CoreError::works_not_file());
        }
        if meta.len() > WORKS_MAX_FILE_BYTES {
            return Err(CoreError::works_too_large("file", meta.len(), WORKS_MAX_FILE_BYTES));
        }
        if !is_probably_text(&abs, meta.len()) {
            return Err(CoreError::works_binary());
        }
        let content = fs::read_to_string(&abs).map_err(|e| {
            if e.kind() == io::ErrorKind::InvalidData {
                CoreError::works_not_utf8()
            } else {
                CoreError::Io(e)
            }
        })?;
        let jail = self.jail_canonical(workspace_id)?;
        let rel = display_rel(&abs, &jail)?;
        Ok(WorksFileBody {
            path: rel,
            content,
            size: Some(meta.len()),
        })
    }

    pub fn write_text(
        &self,
        workspace_id: &str,
        path: &str,
        content: &str,
    ) -> CoreResult<WorksFileBody> {
        if content.len() as u64 > WORKS_MAX_FILE_BYTES {
            return Err(CoreError::works_too_large(
                "write",
                content.len() as u64,
                WORKS_MAX_FILE_BYTES,
            ));
        }
        if content.bytes().any(|b| b == 0) {
            return Err(CoreError::coded(
                "WORKS_BINARY_CONTENT",
                "binary content rejected",
            ));
        }
        let rel = normalize_rel_path(path)?;
        if rel.is_empty() || rel == "." {
            return Err(CoreError::coded(
                "WORKS_ROOT_FORBIDDEN",
                "cannot write to workspace root",
            ));
        }
        let abs = self.resolve(workspace_id, &rel, true)?;
        // Parents are not auto-created (use mkdir). Parent must already exist inside jail.
        if let Some(parent) = abs.parent() {
            if !parent.exists() {
                return Err(CoreError::works_parent_missing());
            }
            let parent_canon = fs::canonicalize(parent)?;
            let jail = self.jail_canonical(workspace_id)?;
            ensure_under_jail(&parent_canon, &jail)?;
        }
        // If path exists as a dir, reject
        if abs.exists() {
            let meta = fs::metadata(&abs)?;
            if meta.is_dir() {
                return Err(CoreError::works_is_dir());
            }
            // Re-check jail after existence (symlink race)
            let canon = fs::canonicalize(&abs)?;
            let jail = self.jail_canonical(workspace_id)?;
            ensure_under_jail(&canon, &jail)?;
        }
        fs::write(&abs, content.as_bytes())?;
        let jail = self.jail_canonical(workspace_id)?;
        // After write, canonicalize and verify still in jail
        let canon = fs::canonicalize(&abs)?;
        ensure_under_jail(&canon, &jail)?;
        let out_rel = display_rel(&canon, &jail)?;
        Ok(WorksFileBody {
            path: out_rel,
            content: content.to_string(),
            size: Some(content.len() as u64),
        })
    }

    /// Append UTF-8 text to a file (create if missing). Parents must exist.
    /// Fail-open friendly for live docs.
    pub fn append_text(
        &self,
        workspace_id: &str,
        path: &str,
        chunk: &str,
    ) -> CoreResult<WorksFileBody> {
        if chunk.bytes().any(|b| b == 0) {
            return Err(CoreError::coded(
                "WORKS_BINARY_CONTENT",
                "binary content rejected",
            ));
        }
        let existing = match self.read_text(workspace_id, path) {
            Ok(b) => b.content,
            Err(CoreError::NotFound(_)) => String::new(),
            Err(e) => return Err(e),
        };
        let combined = if existing.is_empty() {
            chunk.to_string()
        } else if existing.ends_with('\n') {
            format!("{existing}{chunk}")
        } else {
            format!("{existing}\n{chunk}")
        };
        if combined.len() as u64 > WORKS_MAX_FILE_BYTES {
            return Err(CoreError::works_too_large(
                "append",
                combined.len() as u64,
                WORKS_MAX_FILE_BYTES,
            ));
        }
        // Ensure parent for first create
        let rel = normalize_rel_path(path)?;
        if let Some(parent) = Path::new(&rel).parent() {
            let parent_s = parent.to_string_lossy();
            if !parent_s.is_empty() && parent_s != "." {
                let _ = self.mkdir(workspace_id, &parent_s);
            }
        }
        self.write_text(workspace_id, path, &combined)
    }

    pub fn mkdir(&self, workspace_id: &str, path: &str) -> CoreResult<WorksStat> {
        let rel = normalize_rel_path(path)?;
        if rel.is_empty() || rel == "." {
            return Err(CoreError::coded(
                "WORKS_ROOT_FORBIDDEN",
                "cannot mkdir workspace root",
            ));
        }
        let abs = self.resolve(workspace_id, &rel, true)?;
        fs::create_dir_all(&abs)?;
        let jail = self.jail_canonical(workspace_id)?;
        let canon = fs::canonicalize(&abs)?;
        ensure_under_jail(&canon, &jail)?;
        self.stat(workspace_id, &rel)
    }

    pub fn rename(&self, workspace_id: &str, from: &str, to: &str) -> CoreResult<WorksStat> {
        let from_rel = normalize_rel_path(from)?;
        let to_rel = normalize_rel_path(to)?;
        if from_rel.is_empty() || to_rel.is_empty() {
            return Err(CoreError::coded(
                "WORKS_ROOT_FORBIDDEN",
                "cannot rename workspace root",
            ));
        }
        let from_abs = self.resolve(workspace_id, &from_rel, false)?;
        let to_abs = self.resolve(workspace_id, &to_rel, true)?;
        if let Some(parent) = to_abs.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(&from_abs, &to_abs)?;
        let jail = self.jail_canonical(workspace_id)?;
        let canon = fs::canonicalize(&to_abs)?;
        ensure_under_jail(&canon, &jail)?;
        self.stat(workspace_id, &to_rel)
    }

    pub fn delete(
        &self,
        workspace_id: &str,
        path: &str,
        recursive: bool,
    ) -> CoreResult<()> {
        let rel = normalize_rel_path(path)?;
        if rel.is_empty() || rel == "." {
            return Err(CoreError::coded(
                "WORKS_ROOT_FORBIDDEN",
                "cannot delete workspace root",
            ));
        }
        let abs = self.resolve(workspace_id, &rel, false)?;
        let meta = fs::metadata(&abs)?;
        if meta.is_dir() {
            if recursive {
                fs::remove_dir_all(&abs)?;
            } else {
                fs::remove_dir(&abs).map_err(|e| {
                    if e.kind() == io::ErrorKind::DirectoryNotEmpty {
                        CoreError::coded(
                            "WORKS_DIR_NOT_EMPTY",
                            "directory not empty; pass recursive=true",
                        )
                    } else {
                        CoreError::Io(e)
                    }
                })?;
            }
        } else {
            fs::remove_file(&abs)?;
        }
        Ok(())
    }
}
