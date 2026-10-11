//! llm_stream/turn_proto —— 回合流 Anthropic / Google 协议实现。
//! [R6-3② 2026-10-07] 机械迁入。
#![allow(unused_imports)]

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
use crate::llm_provider::{
    ChatMessage, ChatRequest, LLMProvider, ProviderConfig, ProviderKind,
};
use super::*;

pub(crate) async fn stream_chat_turn_anthropic(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
    on_event: TurnEventCb<'_>,
    should_stop: StopCb<'_>,
) -> Result<TurnStreamOutcome, TurnStreamError> {
    let client = turn_http_client(timeout_secs)?;
    let url = format!("{}/v1/messages", base_url.trim_end_matches('/'));
    let mut body = json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "temperature": temperature,
        "messages": [{"role": "user", "content": user}],
    });
    if !system.trim().is_empty() {
        body["system"] = json!(system);
    }
    let resp = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| TurnStreamError::Connect(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(TurnStreamError::Status { status, body: body_text });
    }

    let mut outcome = TurnStreamOutcome::default();
    let mut input_tokens: Option<u64> = None;
    let mut output_tokens: Option<u64> = None;
    let mut splitter = SseLineSplitter::new();
    let mut pending_event = String::new();
    let mut byte_stream = resp.bytes_stream();

    'outer: while let Some(item) = byte_stream.next().await {
        if should_stop() {
            return Err(TurnStreamError::Stopped);
        }
        let bytes = match item {
            Ok(b) => b,
            Err(e) => return Err(TurnStreamError::Stream(e.to_string())),
        };
        splitter.push(&bytes);
        while let Some(raw) = splitter.next_line() {
            let line = raw.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix("event:") {
                pending_event = name.trim().to_string();
                continue;
            }
            let Some(data_str) = line.strip_prefix("data:") else {
                continue;
            };
            let data_str = data_str.trim_start();
            let event = std::mem::take(&mut pending_event);
            let Ok(v) = serde_json::from_str::<Value>(data_str) else {
                continue;
            };
            match event.as_str() {
                "message_start" => {
                    input_tokens = v
                        .pointer("/message/usage/input_tokens")
                        .and_then(|t| t.as_u64())
                        .or(input_tokens);
                }
                "content_block_delta" => {
                    if let Some(text) = v.pointer("/delta/text").and_then(|t| t.as_str()) {
                        if !text.is_empty() {
                            outcome.text.push_str(text);
                            if !on_event(TurnStreamEvent::Delta(text.to_string())) {
                                return Err(TurnStreamError::Stopped);
                            }
                        }
                    } else if let Some(th) =
                        v.pointer("/delta/thinking").and_then(|t| t.as_str())
                    {
                        if !th.is_empty() {
                            outcome.thinking.push_str(th);
                            if !on_event(TurnStreamEvent::Thinking(th.to_string())) {
                                return Err(TurnStreamError::Stopped);
                            }
                        }
                    }
                }
                "message_delta" => {
                    output_tokens = v
                        .pointer("/usage/output_tokens")
                        .and_then(|t| t.as_u64())
                        .or(output_tokens);
                }
                "message_stop" => break 'outer,
                _ => {} // ping / content_block_start / content_block_stop / error
            }
        }
    }
    match (input_tokens, output_tokens) {
        (Some(i), Some(o)) => outcome.total_tokens = Some(i + o),
        (Some(i), None) => outcome.total_tokens = Some(i),
        (None, Some(o)) => outcome.total_tokens = Some(o),
        (None, None) => {}
    }
    Ok(outcome)
}

/// Google Gemini 分支：streamGenerateContent 默认返回按行分块的 JSON 对象
/// （与 llm_provider/google.rs 同款解析）。part.thought==true 的部分为
/// 思维摘要 → Thinking；usageMetadata.totalTokenCount → total_tokens。
pub(crate) async fn stream_chat_turn_google(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
    on_event: TurnEventCb<'_>,
    should_stop: StopCb<'_>,
) -> Result<TurnStreamOutcome, TurnStreamError> {
    if base_url.trim().is_empty() || api_key.trim().is_empty() {
        return Err(TurnStreamError::Connect("llm base_url/api_key empty".into()));
    }
    let client = turn_http_client(timeout_secs)?;
    let url = format!(
        "{}/models/{}:streamGenerateContent?key={}",
        base_url.trim_end_matches('/'),
        model,
        api_key
    );
    let mut body = json!({
        "contents": [{"role": "user", "parts": [{"text": user}]}],
        "generationConfig": {
            "temperature": temperature,
            "maxOutputTokens": max_tokens,
        },
    });
    if !system.trim().is_empty() {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
    }
    let resp = client
        .post(&url)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| TurnStreamError::Connect(e.to_string()))?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body_text = resp.text().await.unwrap_or_default();
        return Err(TurnStreamError::Status { status, body: body_text });
    }

    let mut outcome = TurnStreamOutcome::default();
    let mut splitter = SseLineSplitter::new();
    let mut byte_stream = resp.bytes_stream();

    while let Some(item) = byte_stream.next().await {
        if should_stop() {
            return Err(TurnStreamError::Stopped);
        }
        let bytes = match item {
            Ok(b) => b,
            Err(e) => return Err(TurnStreamError::Stream(e.to_string())),
        };
        splitter.push(&bytes);
        while let Some(line) = splitter.next_line() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // 跳过 JSON 数组包装符（streamGenerateContent 流式形态）
            if line == "[" || line == "]" || line == "," || line.starts_with(']') {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            if let Some(t) = v.pointer("/usageMetadata/totalTokenCount").and_then(|t| t.as_u64())
            {
                outcome.total_tokens = Some(t);
            }
            let Some(parts) = v
                .pointer("/candidates/0/content/parts")
                .and_then(|p| p.as_array())
            else {
                continue;
            };
            for part in parts {
                let Some(text) = part["text"].as_str() else {
                    continue;
                };
                if text.is_empty() {
                    continue;
                }
                let is_thought =
                    part["thought"].as_bool().unwrap_or(false);
                let ev = if is_thought {
                    outcome.thinking.push_str(text);
                    TurnStreamEvent::Thinking(text.to_string())
                } else {
                    outcome.text.push_str(text);
                    TurnStreamEvent::Delta(text.to_string())
                };
                if !on_event(ev) {
                    return Err(TurnStreamError::Stopped);
                }
            }
        }
    }
    Ok(outcome)
}
