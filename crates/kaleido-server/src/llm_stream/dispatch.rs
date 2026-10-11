//! llm_stream/dispatch —— provider 分派包装（OpenAI 兼容 / Anthropic / Google 路由）。
//! [R6-3② 2026-10-07] 机械迁入。
#![allow(unused_imports)]

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
use crate::llm_provider::{
    ChatMessage, ChatRequest, LLMProvider, ProviderConfig, ProviderKind,
};
use super::*;

pub async fn stream_chat_completions_dispatch(
    base_url: &str,
    api_key: &str,
    model: &str,
    provider_kind: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
    mut on_delta: impl FnMut(&str) -> bool + Send,
) -> Result<String, String> {
    let kind = parse_provider(provider_kind);

    match kind {
        ProviderKind::Anthropic | ProviderKind::Google => {
            let config = ProviderConfig {
                kind,
                base_url: base_url.to_string(),
                api_key: api_key.to_string(),
                model: model.to_string(),
                timeout_secs: timeout_secs.max(30),
            };
            if !config.is_configured() {
                return Err("llm provider not configured (base_url/api_key)".into());
            }
            let provider = crate::llm_provider::create_provider(&config);
            let req = ChatRequest {
                model: model.to_string(),
                messages: vec![
                    ChatMessage { role: "system".into(), content: system.to_string() },
                    ChatMessage { role: "user".into(), content: user.to_string() },
                ],
                temperature,
                max_tokens,
                timeout_secs,
            };
            provider.stream_chat(&req, &mut on_delta).await
        }
        ProviderKind::OpenAI => {
            stream_chat_completions(
                base_url,
                api_key,
                model,
                system,
                user,
                temperature,
                max_tokens,
                timeout_secs,
                on_delta,
            )
            .await
        }
    }
}

/// 多轮版 + provider dispatch（G6）：`messages` 为完整对话数组。
/// Anthropic/Google 走 provider 后端；OpenAI 回退 `stream_chat_completions_msgs`。
pub async fn stream_chat_completions_msgs_dispatch<F>(
    base_url: &str,
    api_key: &str,
    model: &str,
    provider_kind: &str,
    messages: Vec<serde_json::Value>,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
    mut on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> bool + Send,
{
    let kind = parse_provider(provider_kind);
    match kind {
        ProviderKind::Anthropic | ProviderKind::Google => {
            let config = ProviderConfig {
                kind,
                base_url: base_url.to_string(),
                api_key: api_key.to_string(),
                model: model.to_string(),
                timeout_secs: timeout_secs.max(30),
            };
            if !config.is_configured() {
                return Err("llm provider not configured (base_url/api_key)".into());
            }
            let provider = crate::llm_provider::create_provider(&config);
            let msgs = messages
                .iter()
                .map(|m| ChatMessage {
                    role: m["role"].as_str().unwrap_or("user").to_string(),
                    content: m["content"].as_str().unwrap_or_default().to_string(),
                })
                .collect();
            let req = ChatRequest {
                model: model.to_string(),
                messages: msgs,
                temperature,
                max_tokens,
                timeout_secs,
            };
            provider.stream_chat(&req, &mut on_delta).await
        }
        ProviderKind::OpenAI => {
            stream_chat_completions_msgs(
                base_url,
                api_key,
                model,
                messages,
                temperature,
                max_tokens,
                timeout_secs,
                &mut on_delta,
            )
            .await
        }
    }
}

pub async fn chat_completion_dispatch(
    base_url: &str,
    api_key: &str,
    model: &str,
    provider_kind: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
    client: &reqwest::Client,
) -> Result<String, String> {
    let kind = parse_provider(provider_kind);

    match kind {
        ProviderKind::Anthropic | ProviderKind::Google => {
            let config = ProviderConfig {
                kind,
                base_url: base_url.to_string(),
                api_key: api_key.to_string(),
                model: model.to_string(),
                timeout_secs: timeout_secs.max(30),
            };
            if !config.is_configured() {
                return Err("llm provider not configured".into());
            }
            let provider = crate::llm_provider::create_provider(&config);
            let req = ChatRequest {
                model: model.to_string(),
                messages: vec![
                    ChatMessage { role: "system".into(), content: system.to_string() },
                    ChatMessage { role: "user".into(), content: user.to_string() },
                ],
                temperature,
                max_tokens,
                timeout_secs,
            };
            provider.chat(&req).await
        }
        ProviderKind::OpenAI => {
            chat_completion(base_url, api_key, model, system, user, temperature, max_tokens, timeout_secs, client).await
        }
    }
}

// ---------------------------------------------------------------------------
// G6 后置项（F3）：主回合级流式 dispatch —— story_tavern start_turn 的
// 原生 reqwest bytes_stream 循环下沉至此，三家协议统一入口。
//
// 与 stream_chat_completions_dispatch 的差异：
// - 事件面 richer：正文增量 + 推理增量（thinking）分离回调；
// - 结果带 usage（OpenAI total_tokens / Anthropic input+output / Gemini
//   totalTokenCount），随 asst_msg.tokens 落盘；
// - 错误分类 TurnStreamError 与旧内嵌循环一一对应（Connect / Status{body}
//   / Stream / Stopped），调用方的模型回退与 UPSTREAM_* 语义码保持不变；
// - should_stop 钩子在每块网络数据前检查，承载取消（StreamHub）与 U11
//   回合预算看门狗。
// ---------------------------------------------------------------------------
