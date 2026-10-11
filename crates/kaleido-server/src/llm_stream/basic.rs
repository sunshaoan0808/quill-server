//! llm_stream/basic —— 非 turn 流式基础（chat/completions SSE 逐块 + 非流式补全
//! + JSON 抽取 + embedding）。软失败风格：Err(String) 由调用方映射启发式。
//! [R6-3② 2026-10-07] 机械迁入。
#![allow(unused_imports)]

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
use crate::llm_provider::{
    ChatMessage, ChatRequest, LLMProvider, ProviderConfig, ProviderKind,
};
use super::*;

pub async fn stream_chat_completions<F>(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
    mut on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> bool + Send,
{
    let messages = vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": user}),
    ];
    stream_chat_completions_msgs(base_url, api_key, model, messages, temperature, max_tokens, timeout_secs, &mut on_delta).await
}

/// 多轮版：`messages` 为完整对话数组（system 需调用方自行放入首位）。
/// 剧情助手等需要携带对话历史的场景使用。
pub async fn stream_chat_completions_msgs<F>(
    base_url: &str,
    api_key: &str,
    model: &str,
    messages: Vec<serde_json::Value>,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
    mut on_delta: F,
) -> Result<String, String>
where
    F: FnMut(&str) -> bool + Send,
{
    if base_url.trim().is_empty() {
        return Err("llm base_url empty".into());
    }
    // [SSRF 加固 2026-08-15, 吸收 6fef9d12] 连接前防御纵深复检（设置层之外的第二道防线）。
    if let Err(e) = kaleido_core::validate_llm_base_url(base_url) {
        return Err(format!("llm base_url rejected: {e}").into());
    }
    if api_key.trim().is_empty() {
        return Err("llm api_key empty".into());
    }
    let model = if model.trim().is_empty() {
        "gpt-4o-mini"
    } else {
        model.trim()
    };

    let client = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .pool_idle_timeout(std::time::Duration::from_secs(4))
        .pool_max_idle_per_host(2)
        .timeout(StdDuration::from_secs(timeout_secs.max(30)))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let url = format!(
        "{}/chat/completions",
        base_url.trim_end_matches('/')
    );
    let body = json!({
        "model": model,
        "stream": true,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "messages": messages,
    });

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("upstream connect: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "upstream {status}: {}",
            text.chars().take(300).collect::<String>()
        ));
    }

    let mut full = String::new();
    let mut buf = String::new();
    let mut byte_carry: Vec<u8> = Vec::new();
    let mut byte_stream = resp.bytes_stream();
    let mut aborted = false;

    // Cancel-poll interval. `on_delta` is the cooperative cancel hook used by
    // callers (background / book_travel) — they return `false` once the job is
    // terminal. But the upstream can sit idle on a long tokenization for seconds
    // (no chunks → no callback → cancel never observed). Wrap the next-chunk
    // wait in `select!` with a 250ms heartbeat: on timeout, invoke the callback
    // with an empty delta so it can re-check cancel and abort promptly. Empty
    // pieces are filtered out of `full`, so this never corrupts the result.
    const CANCEL_POLL_MS: u64 = 250;

    loop {
        let next_chunk = byte_stream.next();
        tokio::pin!(next_chunk);
        let poll = tokio::time::sleep(StdDuration::from_millis(CANCEL_POLL_MS));
        tokio::pin!(poll);
        let item = tokio::select! {
            i = &mut next_chunk => match i {
                Some(it) => it,
                None => break, // stream ended
            },
            _ = &mut poll => {
                // Heartbeat: give the callback a chance to signal cancel.
                // Return `false` to abort; empty string never lands in `full`.
                if !on_delta("") {
                    aborted = true;
                    break;
                }
                continue;
            }
        };

        match item {
            Ok(bytes) => {
                // Carry incomplete UTF-8 across TCP chunks (CJK is 3 bytes).
                buf.push_str(&crate::utf8_stream::push_utf8_chunk(&mut byte_carry, &bytes));
                while let Some(pos) = buf.find('\n') {
                    let mut line = buf[..pos].to_string();
                    buf = buf[pos + 1..].to_string();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                    if line.is_empty() {
                        continue;
                    }
                    // tolerate "data:" and "data: "
                    let data = if let Some(rest) = line.strip_prefix("data:") {
                        rest.trim_start()
                    } else {
                        continue;
                    };
                    if data == "[DONE]" {
                        return Ok(full);
                    }
                    if data.is_empty() {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(data) {
                        // [fix 2026-08-15 截断可观测] 捕获 finish_reason=length（上游 max_tokens
                        // 硬截断）——之前零检测，截断被静默接受为完整正文（宿醉 6360 字半截实踩）。
                        // 最小侵入：warn 日志 + 尾部截断标记，不改返回签名（调用方无需迁移）。
                        if let Some(fr) = v.pointer("/choices/0/finish_reason").and_then(|c| c.as_str()) {
                            if fr == "length" {
                                let total = full.chars().count();
                                tracing::warn!(
                                    model,
                                    %base_url,
                                    chars = total,
                                    "llm stream truncated by max_tokens (finish_reason=length)"
                                );
                                // 截断只记日志，不污染返回文本：调用方（回合正文/Heavy 管道
                                // draft）会直接落盘，尾部标记会泄漏进用户可见正文（实踩：
                                // 宿醉消息尾部出现「📤【输出已达长度上限，已截断】」）。
                            }
                        }
                        // OpenAI delta.content
                        let mut pieces: Vec<String> = Vec::new();
                        if let Some(s) = v
                            .pointer("/choices/0/delta/content")
                            .and_then(|c| c.as_str())
                        {
                            if !s.is_empty() {
                                pieces.push(s.to_string());
                            }
                        } else if let Some(arr) = v
                            .pointer("/choices/0/delta/content")
                            .and_then(|c| c.as_array())
                        {
                            // some providers send content as array of parts
                            for part in arr {
                                if let Some(t) = part.get("text").and_then(|x| x.as_str()) {
                                    if !t.is_empty() {
                                        pieces.push(t.to_string());
                                    }
                                } else if let Some(t) = part.as_str() {
                                    if !t.is_empty() {
                                        pieces.push(t.to_string());
                                    }
                                }
                            }
                        }
                        // non-stream style in a stream chunk
                        if pieces.is_empty() {
                            if let Some(s) = v
                                .pointer("/choices/0/message/content")
                                .and_then(|c| c.as_str())
                            {
                                if !s.is_empty() {
                                    pieces.push(s.to_string());
                                }
                            }
                        }
                        for piece in pieces {
                            full.push_str(&piece);
                            if !on_delta(&piece) {
                                aborted = true;
                                break;
                            }
                        }
                        if aborted {
                            break;
                        }
                    }
                }
                if aborted {
                    break;
                }
            }
            Err(e) => {
                if full.is_empty() {
                    return Err(format!("upstream stream error: {e}"));
                }
                // partial is better than nothing for soft-fail parse
                break;
            }
        }
    }

    // End of stream: drop incomplete trailing bytes into buf (rare).
    let tail = crate::utf8_stream::flush_utf8_carry(&mut byte_carry);
    if !tail.is_empty() {
        buf.push_str(&tail);
    }

    if aborted {
        if full.is_empty() {
            return Err("cancelled".into());
        }
        return Ok(full);
    }

    if full.trim().is_empty() {
        return Err("empty stream content".into());
    }
    Ok(full)
}

/// Non-streaming chat completion. Returns the full response text.
/// Uses the same endpoint but without `stream:true`, for small structured calls (extractions, etc.).
/// Accepts a pre-built `reqwest::Client` (cheap to clone) to reuse existing client config & connection pool.
/// `timeout_secs` is unused (client manages its own timeout).
pub async fn chat_completion(
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: u32,
    _timeout_secs: u64,
    client: &reqwest::Client,
) -> Result<String, String> {
    let request_timeout = std::time::Duration::from_secs(_timeout_secs.max(30));
    if base_url.trim().is_empty() {
        return Err("llm base_url empty".into());
    }
    // [SSRF 加固 2026-08-15, 吸收 6fef9d12] 非流式入口同样复检。
    if let Err(e) = kaleido_core::validate_llm_base_url(base_url) {
        return Err(format!("llm base_url rejected: {e}").into());
    }
    if api_key.trim().is_empty() {
        return Err("llm api_key empty".into());
    }
    let model = if model.trim().is_empty() {
        "gpt-4o-mini"
    } else {
        model.trim()
    };

    let url = format!(
        "{}/chat/completions",
        base_url.trim_end_matches('/')
    );
    let body = json!({
        "model": model,
        "stream": false,
        "temperature": temperature,
        // [F4 调参化] 原 16384 硬编码 → 调用方显式传参（convert 等传 16384 保持现状）。
        "max_tokens": max_tokens,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    });

    let resp = client
        .post(&url)
        .bearer_auth(api_key)
        .header("content-type", "application/json")
        .json(&body)
        .timeout(request_timeout)
        .send()
        .await
        .map_err(|e| format!("extraction upstream: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!(
            "extraction upstream {status}: {}",
            text.chars().take(200).collect::<String>()
        ));
    }

    let data: Value = resp
        .json()
        .await
        .map_err(|e| format!("extraction parse: {e}"))?;

    let content = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    if content.is_empty() {
        return Err("extraction empty response".into());
    }

    Ok(content)
}

/// Strip markdown fences and extract the outermost JSON object/array if needed.
pub fn extract_json_value(text: &str) -> Option<Value> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        return Some(v);
    }
    // strip ```json ... ```
    let mut s = t.to_string();
    if let Some(start) = s.find("```") {
        let after = &s[start + 3..];
        let after = after
            .strip_prefix("json")
            .or_else(|| after.strip_prefix("JSON"))
            .unwrap_or(after);
        if let Some(end) = after.find("```") {
            s = after[..end].trim().to_string();
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                return Some(v);
            }
        }
    }
    // outermost object (健壮版: 用括号深度找第一个完整闭合的 }，
    // 避免模型在 JSON 后追加文字/注释时 rfind('}') 截到文字里的右括号)
    // 同时跳过数组内部的 {（避免数组文本时截到元素对象）
    {
        let bytes = t.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            let c = bytes[i] as char;
            if c == '{' {
                // 检查此 { 是否在某个未闭合 [ 内部：从文本头到当前位置做括号扫描
                let mut arr_depth = 0i32;
                let mut in_str = false;
                let mut esc = false;
                for j in 0..i {
                    let cj = bytes[j] as char;
                    if in_str {
                        if esc { esc = false; }
                        else if cj == '\\' { esc = true; }
                        else if cj == '"' { in_str = false; }
                        continue;
                    }
                    match cj {
                        '"' => in_str = true,
                        '[' => arr_depth += 1,
                        ']' => arr_depth -= 1,
                        _ => {}
                    }
                }
                if arr_depth > 0 {
                    // 在数组内部，跳过这个 {（数组分支会处理）
                    i += 1;
                    continue;
                }
                // 顶层对象：深度匹配闭合 }
                let mut depth = 0i32;
                let mut in_str2 = false;
                let mut esc2 = false;
                for k in i..bytes.len() {
                    let ck = bytes[k] as char;
                    if in_str2 {
                        if esc2 { esc2 = false; }
                        else if ck == '\\' { esc2 = true; }
                        else if ck == '"' { in_str2 = false; }
                        continue;
                    }
                    match ck {
                        '"' => in_str2 = true,
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                if let Ok(v) = serde_json::from_str::<Value>(&t[i..=k]) {
                                    return Some(v);
                                }
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            i += 1;
        }
    }
    // outermost array (健壮版: 用括号深度找第一个闭合的 ]，
    // 避免模型在 JSON 后追加注释时 rfind(']') 截到注释里的方括号)
    if let Some(a) = t.find('[') {
        let bytes = t.as_bytes();
        let mut depth = 0i32;
        let mut in_str = false;
        let mut esc = false;
        for i in a..bytes.len() {
            let c = bytes[i] as char;
            if in_str {
                if esc { esc = false; }
                else if c == '\\' { esc = true; }
                else if c == '"' { in_str = false; }
                continue;
            }
            match c {
                '"' => in_str = true,
                '[' | '{' => depth += 1,
                ']' => {
                    depth -= 1;
                    if depth == 0 {
                        if let Ok(v) = serde_json::from_str::<Value>(&t[a..=i]) {
                            return Some(v);
                        }
                        break;
                    }
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
    }
    None
}

/// Get a single text embedding vector (512-d BGE-small-zh).
///
/// Order:
/// 1. In-process `fastembed` when `KALEIDO_EMBED_INLINE` enabled (default)
/// 2. HTTP OpenAI-compatible `/v1/embeddings` at `base_url` (legacy Python proxy)
/// 3. 零依赖本地哈希 embed 兜底（吸收自 Liyuan embedTextLocal）——模型/代理都不可用时
///    仍返回确定性 512-d 向量，保证检索功能不硬失败。
pub async fn get_embedding(
    base_url: &str,
    input: &str,
    client: &reqwest::Client,
) -> Result<Vec<f32>, String> {
    // Prefer in-process — no Python sidecar required.
    if crate::embed_local::inline_enabled() {
        match tokio::task::spawn_blocking({
            let s = input.to_string();
            move || crate::embed_local::embed_one(&s)
        })
        .await
        {
            Ok(Ok(v)) => return Ok(v),
            Ok(Err(e)) => {
                tracing::warn!(error=%e, "embed_local failed; trying remote");
            }
            Err(e) => {
                tracing::warn!(error=%e, "embed_local join failed; trying remote");
            }
        }
    }

    if !base_url.trim().is_empty() {
        let url = format!("{}/v1/embeddings", base_url.trim_end_matches('/'));
        let body = serde_json::json!({
            "input": input,
            "model": "BAAI/bge-small-zh-v1.5",
        });

        let resp = client
            .post(&url)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("embedding upstream: {e}"));

        if let Ok(resp) = resp {
            if resp.status().is_success() {
                let data: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("embedding parse: {e}"))?;

                let arr = data["data"][0]["embedding"]
                    .as_array()
                    .ok_or_else(|| "embedding missing data[0].embedding".to_string())?;

                let vec: Vec<f32> = arr
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();

                if !vec.is_empty() {
                    return Ok(vec);
                }
            }
        }
    }

    // 兜底：本地哈希 embed（无模型、无网络、确定性）
    tracing::warn!(
        "embedding chain exhausted (inline + remote); falling back to local hash embed (512-d)"
    );
    Ok(kaleido_core::embed_hash::embed_text_hash(input, 512))
}
