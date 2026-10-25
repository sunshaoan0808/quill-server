//! Thin-client sync/backup manifest helpers.
//!
//! These helpers deliberately expose only the portable, server-owned records;
//! provider credentials stay on the server and are redacted from exports.

use crate::AppState;
use kaleido_core::CoreResult;
use serde_json::{json, Value};

pub(crate) const SYNC_PROTOCOL: u64 = 1;
pub(crate) const SYNC_STATE_NAMES: [&str; 3] = [
    "settings-store",
    "partner-store",
    "chat-shelf-schedule",
];

const REDACTED: &str = "[server]";

fn is_secret_key(key: &str) -> bool {
    let normalized: String = key
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();
    normalized == "apikey"
        || normalized == "accesstoken"
        || normalized == "refreshtoken"
        || normalized == "authorization"
        || normalized == "cookie"
        || normalized == "cookies"
        || normalized == "password"
        || normalized == "secret"
        || normalized == "credentials"
        || normalized.ends_with("apikey")
        || normalized.ends_with("token")
        || normalized.ends_with("password")
        || normalized.ends_with("secret")
}

pub(crate) fn redact_secrets(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if is_secret_key(key) {
                    *child = Value::String(REDACTED.into());
                } else {
                    redact_secrets(child);
                }
            }
        }
        Value::Array(items) => {
            for child in items {
                redact_secrets(child);
            }
        }
        _ => {}
    }
}

pub(crate) fn build_manifest(state: &AppState, user_id: &str) -> CoreResult<Value> {
    // Keep the store API prefix-strict; merge both supported session families
    // without ever falling back to the legacy global directory.
    let mut sessions = Vec::new();
    for prefix in ["partner-session-", "story-session-"] {
        sessions.extend(state.sessions.list_for_user(user_id, prefix, None)?);
    }
    sessions.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    let mut states = serde_json::Map::new();
    for name in SYNC_STATE_NAMES {
        let raw = match name {
            "partner-store" => state.app_state.load_for_user(user_id, name),
            _ => state.app_state.load_for_user(user_id, name),
        }?;
        let mut value: Value = serde_json::from_str(&raw).unwrap_or_else(|_| json!({"raw": raw}));
        redact_secrets(&mut value);
        states.insert((*name).to_string(), value);
    }
    Ok(json!({
        "protocol": SYNC_PROTOCOL,
        "userId": user_id,
        "sessions": sessions,
        "states": states,
    }))
}

pub(crate) fn build_export(state: &AppState, user_id: &str) -> CoreResult<Value> {
    let mut export = build_manifest(state, user_id)?;
    redact_secrets(&mut export);
    if let Some(obj) = export.as_object_mut() {
        obj.insert("exportedAt".into(), json!(chrono::Utc::now().to_rfc3339()));
    }
    Ok(export)
}

#[cfg(test)]
mod tests {
    use super::redact_secrets;
    use serde_json::json;

    #[test]
    fn redacts_nested_provider_credentials_without_touching_other_values() {
        let mut value = json!({
            "state": {
                "llmApiKey": "do-not-export",
                "providerApiKey": "also-do-not-export",
                "accessToken": "another-secret",
                "refresh_token": "refresh-secret",
                "authorization": "Bearer secret",
                "credentials": {"password": "nested-secret"},
                "model": "model-a",
                "nested": [{"token": "also-secret", "label": "keep"}]
            }
        });
        redact_secrets(&mut value);
        assert_eq!(value["state"]["llmApiKey"], "[server]");
        assert_eq!(value["state"]["providerApiKey"], "[server]");
        assert_eq!(value["state"]["accessToken"], "[server]");
        assert_eq!(value["state"]["refresh_token"], "[server]");
        assert_eq!(value["state"]["authorization"], "[server]");
        assert_eq!(value["state"]["credentials"], "[server]");
        assert_eq!(value["state"]["model"], "model-a");
        assert_eq!(value["state"]["nested"][0]["token"], "[server]");
        assert_eq!(value["state"]["nested"][0]["label"], "keep");
    }
}
