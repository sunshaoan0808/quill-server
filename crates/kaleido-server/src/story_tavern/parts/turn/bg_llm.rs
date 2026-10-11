//! parts/turn/bg_llm — [2026-10-05 P2-P4] 按设计文档 §3.1 从 p006/p009 逐字节迁出。
//! bg_llm_with_retry（后台 LLM 调用，原 p006）+ call_llm_nonstream（原 p009 的隐藏反向依赖，就地正位）。
//! 以 `use super::super::*;` 直连 parts 作用域；本文件不自带 use 头，
//! cargo fix 无桥可拆（工程纪律见 docs/REFACTOR_20260927.md）。
#![allow(unused_imports)]

use super::super::*;

/// P3.1（审D 后处理韧性）：非阻塞后处理 LLM 单发改两发 —— 首发失败立即重试一次。
/// 适用 roster 回退 / LLM 抽取 / 记忆压缩等 fire-and-forget 生成调用；
/// 两次皆败仍 fail-open（调用方记日志跳过），不影响回合本体。空响应视为失败触发重试。
pub(crate) async fn bg_llm_with_retry(
    base_url: &str,
    api_key: &str,
    model: &str,
    provider_kind: &str,
    system: &str,
    user: &str,
    temperature: f64,
    max_tokens: u32,
    timeout_secs: u64,
) -> Result<String, String> {
    let first = stream_chat_completions_dispatch(
        base_url, api_key, model, provider_kind, system, user, temperature, max_tokens,
        timeout_secs, |_| true,
    )
    .await;
    match first {
        Ok(t) if !t.trim().is_empty() => Ok(t),
        other => {
            let reason = other.err().unwrap_or_else(|| "empty stream content".into());
            tracing::warn!(reason = %reason, "P3.1 bg llm first attempt failed; retrying once");
            stream_chat_completions_dispatch(
                base_url, api_key, model, provider_kind, system, user, temperature, max_tokens,
                timeout_secs, |_| true,
            )
            .await
        }
    }
}


pub(crate) async fn call_llm_nonstream(state: &AppState, system: &str, user: &str) -> Result<String, String> {
    let llm = state.app_state.resolve_llm(
        state.llm_base.as_deref(),
        state.llm_key.as_deref(),
        &state.llm_model,
    );
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return Err("LLM not configured: base_url or api_key is empty".into());
    }
    let model = if llm.model.is_empty() {
        state.llm_model.clone()
    } else {
        llm.model
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .pool_idle_timeout(std::time::Duration::from_secs(4))
        .pool_max_idle_per_host(2)
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/chat/completions", llm.base_url.trim_end_matches('/'));
    let body = json!({
        "model": model,
        "stream": false,
        "temperature": 0.3,
        // [fix 2026-08-15] 8192→16384：模型思考+压缩内容挤占预算曾致截断（5980/6119 实踩）。
        "max_tokens": 16384,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    });
    let resp = client
        .post(&url)
        .bearer_auth(&llm.api_key)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("LLM read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("LLM HTTP {status}: {}", text.chars().take(300).collect::<String>()));
    }
    let v: Value = serde_json::from_str(&text)
        .map_err(|e| format!("LLM JSON parse failed: {e}"))?;
    v.get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "LLM response missing choices[0].message.content".into())
}

// ─── SoulLink 吸收：角色档案维护端点 ──────────────────────────────────────────
// 数据存 pack.characters[].archive（CharacterArchive：标量字段 + 5 分节）。
// analyze = LLM 增量分析（diff 应用）；refine = LLM 精编（整体替换）；purge = 删楼溯源清理。

