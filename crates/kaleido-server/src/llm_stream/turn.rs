//! llm_stream/turn —— 回合流：事件/结果/错误类型 + 分派入口 + HTTP client + SSE 分行器
//! + OpenAI 协议实现。Anthropic/Google 见 turn_proto.rs。
//! [R6-3② 2026-10-07] 机械迁入。
#![allow(unused_imports)]

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
use crate::llm_provider::{
    ChatMessage, ChatRequest, LLMProvider, ProviderConfig, ProviderKind,
};
use super::*;

#[derive(Debug, Clone)]
pub enum TurnStreamEvent {
    /// 正文 delta（OpenAI choices[].delta.content / Anthropic text_delta /
    /// Gemini part.text）
    Delta(String),
    /// 推理/思考增量（OpenAI reasoning_content|reasoning /
    /// Anthropic thinking_delta / Gemini thought part）
    Thinking(String),
}

/// 主回合流结果。空 text 不算错误——空响应重试是调用方策略。
#[derive(Debug, Default, Clone)]
pub struct TurnStreamOutcome {
    pub text: String,
    pub thinking: String,
    /// OpenAI usage.total_tokens；Anthropic input+output tokens；Gemini
    /// usageMetadata.totalTokenCount。上游未给则 None。
    pub total_tokens: Option<u64>,
}

/// 主回合流错误分类（对应旧内嵌循环的 UPSTREAM_CONNECT / UPSTREAM_STATUS
/// (+模型回退) / UPSTREAM_STREAM / 中途停止）。
#[derive(Debug, Clone)]
pub enum TurnStreamError {
    Connect(String),
    Status { status: u16, body: String },
    Stream(String),
    /// should_stop() 命中或下游放弃消费（on_event 返回 false）；调用方按
    /// 自身状态区分「取消」与「超时看门狗」终态。
    Stopped,
}

impl TurnStreamError {
    /// 人类可读消息（调用方直接放进 error event 的 message 字段）。
    pub fn message(&self) -> String {
        match self {
            Self::Connect(e) => format!("upstream connect: {e}"),
            Self::Status { status, body } => format!(
                "upstream {status}: {}",
                body.chars().take(300).collect::<String>()
            ),
            Self::Stream(e) => e.clone(),
            Self::Stopped => "stopped".into(),
        }
    }
}

/// 上游 4xx 是否为「模型不存在/不可用」类错误 —— 主回合模型回退判据，
/// 从 start_turn 内联逻辑原样迁出（含中文「不可用」，zen 网关实测形态）。
pub fn is_model_rejection(status: u16, body: &str) -> bool {
    status >= 400
        && status < 500
        && (body.contains("invalid_model")
            || body.contains("model_not_found")
            || body.contains("model not found")
            || body.contains("model_not_supported")
            || body.contains("不可用"))
}

/// 主回合级流式 dispatch：system+user 进 → 增量事件出 → outcome 返回。
/// temperature/max_tokens 由调用方给定（主回合 0.75/32768）。
pub async fn stream_chat_turn_dispatch(
    base_url: &str,
    api_key: &str,
    model: &str,
    provider_kind: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
    mut on_event: impl FnMut(TurnStreamEvent) -> bool + Send,
    should_stop: impl Fn() -> bool + Send + Sync,
) -> Result<TurnStreamOutcome, TurnStreamError> {
    match parse_provider(provider_kind) {
        ProviderKind::OpenAI => {
            stream_chat_turn_openai(
                base_url, api_key, model, system, user, temperature, max_tokens,
                timeout_secs, &mut on_event, &should_stop,
            )
            .await
        }
        ProviderKind::Anthropic => {
            stream_chat_turn_anthropic(
                base_url, api_key, model, system, user, temperature, max_tokens,
                timeout_secs, &mut on_event, &should_stop,
            )
            .await
        }
        ProviderKind::Google => {
            stream_chat_turn_google(
                base_url, api_key, model, system, user, temperature, max_tokens,
                timeout_secs, &mut on_event, &should_stop,
            )
            .await
        }
    }
}

pub(crate) type TurnEventCb<'a> = &'a mut (dyn FnMut(TurnStreamEvent) -> bool + Send);
pub(crate) type StopCb<'a> = &'a (dyn Fn() -> bool + Send + Sync);

pub(crate) fn turn_http_client(timeout_secs: u64) -> Result<reqwest::Client, TurnStreamError> {
    reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .pool_idle_timeout(std::time::Duration::from_secs(4))
        .pool_max_idle_per_host(2)
        .timeout(StdDuration::from_secs(timeout_secs.max(30)))
        .build()
        .map_err(|e| TurnStreamError::Connect(format!("http client: {e}")))
}

/// SSE 行切分器：跨 chunk 安全（utf8 边界由 utf8_stream 处理，行边界在此处理）。
pub(crate) struct SseLineSplitter {
    buf: String,
    carry: Vec<u8>,
}

impl SseLineSplitter {
    pub(crate) fn new() -> Self {
        Self { buf: String::new(), carry: Vec::new() }
    }
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.buf.push_str(&crate::utf8_stream::push_utf8_chunk(&mut self.carry, bytes));
    }
    pub(crate) fn next_line(&mut self) -> Option<String> {
        let pos = self.buf.find('\n')?;
        let mut line = self.buf[..pos].to_string();
        self.buf = self.buf[pos + 1..].to_string();
        if line.ends_with('\r') {
            line.pop();
        }
        Some(line)
    }
}

/// OpenAI-compatible 分支（deepseek 等）：语义与旧内嵌循环逐条对齐 ——
/// data:[DONE] 终止、usage.total_tokens 捕获、reasoning_content|reasoning
/// 思维链、data: 后无空格也容忍（openai.rs 同款宽松解析）。
pub(crate) async fn stream_chat_turn_openai(
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
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "stream": true,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": temperature,
        "max_tokens": max_tokens,
    });
    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
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
            if line.is_empty() {
                continue;
            }
            let data = match line.strip_prefix("data:") {
                Some(rest) => rest.trim_start(),
                None => continue,
            };
            if data == "[DONE]" {
                return Ok(outcome);
            }
            if data.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(u) = v["usage"]["total_tokens"].as_u64() {
                outcome.total_tokens = Some(u);
            }
            if let Some(d) = v["choices"][0]["delta"]["content"].as_str() {
                if !d.is_empty() {
                    outcome.text.push_str(d);
                    if !on_event(TurnStreamEvent::Delta(d.to_string())) {
                        return Err(TurnStreamError::Stopped);
                    }
                }
            }
            let reasoning = v["choices"][0]["delta"]["reasoning_content"]
                .as_str()
                .or_else(|| v["choices"][0]["delta"]["reasoning"].as_str());
            if let Some(r) = reasoning {
                if !r.is_empty() {
                    outcome.thinking.push_str(r);
                    if !on_event(TurnStreamEvent::Thinking(r.to_string())) {
                        return Err(TurnStreamError::Stopped);
                    }
                }
            }
        }
    }
    Ok(outcome)
}
