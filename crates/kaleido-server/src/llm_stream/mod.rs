//! OpenAI-compatible chat/completions streaming helper.
//!
//! Used by Background / BookTravel (and reusable elsewhere) to:
//! - call `{base}/chat/completions` with `stream: true`
//! - parse SSE `data:` chunks
//! - invoke `on_delta` for each content piece
//! - return the full accumulated text
//!
//! Soft-fail style: returns `Err(String)` for connect/HTTP/empty; callers map to heuristic.

#![allow(unused_imports)]

use futures_util::StreamExt;
use serde_json::{json, Value};

use std::time::Duration as StdDuration;

/// G6: effective provider kind for an LlmRuntime — managed providers carry
/// their own protocol; env/settings fallback defers to the KALEIDO_LLM_PROVIDER

use crate::llm_provider::{
    ChatMessage, ChatRequest, LLMProvider, ProviderConfig, ProviderKind,
};

/// default held in AppState.
pub fn runtime_provider_kind(llm: &kaleido_core::LlmRuntime, state_kind: &str) -> String {
    let k = llm.provider_kind.trim();
    if k.is_empty() {
        state_kind.to_string()
    } else {
        k.to_string()
    }
}

/// Stream an OpenAI-compatible chat completion.
///
/// `on_delta(chunk) -> bool`: return `false` to abort early (cancel).
/// On abort, returns `Err("cancelled")` if no text yet, else `Ok(partial)`.

#[allow(unused_imports)]
pub(crate) mod basic;
#[allow(unused_imports)]
pub use basic::*;
#[allow(unused_imports)]
pub(crate) mod dispatch;
#[allow(unused_imports)]
pub use dispatch::*;
#[allow(unused_imports)]
pub(crate) mod turn;
#[allow(unused_imports)]
pub use turn::*;
#[allow(unused_imports)]
pub(crate) mod turn_proto;
#[allow(unused_imports)]
pub use turn_proto::*;
#[cfg(test)]
mod tests;

pub fn cap_for_prompt(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    t.chars().take(max).collect::<String>() + "\n…(truncated)"
}

/// Backward-compat alias for cap_for_prompt.
pub fn limit_text(text: &str, max_chars: usize) -> String {
    cap_for_prompt(text, max_chars)
}

/// Resolve `ProviderKind` from a string (case-insensitive).
fn parse_provider(kind: &str) -> ProviderKind {
    match kind.trim().to_lowercase().as_str() {
        "anthropic" => ProviderKind::Anthropic,
        "google" | "gemini" => ProviderKind::Google,
        _ => ProviderKind::OpenAI,
    }
}
