//! background/apply —— apply 落地：hash/slugify/字段映射/partner 条目构造。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundApplyBody {
    /// Load job.result when `result` is omitted.
    #[serde(default, alias = "run_id", alias = "jobId", alias = "id")]
    pub run_id: Option<String>,
    /// Explicit BG payload: worldBooks / characterCards / characterCard.
    #[serde(default)]
    pub result: Option<Value>,
    /// Select first applied card (or world book) after upsert. Default true.
    #[serde(default)]
    pub select: Option<bool>,
    /// Optional name prefix for applied items.
    #[serde(default)]
    pub prefix: Option<String>,
}

pub(crate) fn short_hash(s: &str) -> String {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    let full = format!("{:x}", h.finish());
    full.chars().take(8).collect()
}

pub(crate) fn slugify(name: &str) -> String {
    // Keep Unicode letters/numbers (incl. CJK). ASCII alnum → lower; runs of other → '-'.
    // Old is_ascii_alphanumeric path turned 中文 into "___" (looks like mojibake in IDs).
    let mut out = String::new();
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if c.is_alphanumeric() {
            // CJK / other scripts
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let s: String = out.trim_matches('-').chars().take(32).collect();
    if s.is_empty() {
        "item".into()
    } else {
        s
    }
}

pub(crate) fn fields_to_content(name: &str, item_type: &str, fields: &Value) -> String {
    if fields.is_null() {
        return name.to_string();
    }
    if let Some(obj) = fields.as_object() {
        if obj.is_empty() {
            return name.to_string();
        }
        // Prefer markdown compile when fields look structured.
        let md = kaleido_core::compile_partner_markdown(name, item_type, fields);
        if md.trim().len() > name.len() + 2 {
            return md;
        }
        let mut parts = Vec::new();
        for (k, v) in obj {
            let vs = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Array(a) => a
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                other => other.to_string(),
            };
            if !vs.trim().is_empty() {
                parts.push(format!("{k}: {vs}"));
            }
        }
        if parts.is_empty() {
            name.to_string()
        } else {
            format!("{name}\n{}", parts.join("\n"))
        }
    } else {
        fields.to_string()
    }
}

pub(crate) fn entry_name(entry: &Value, fallback: &str) -> String {
    entry
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

pub(crate) fn entry_fields(entry: &Value) -> Option<Value> {
    entry.get("fields").cloned().or_else(|| {
        // Treat whole object minus name/id/type as fields if it has other keys.
        if let Some(obj) = entry.as_object() {
            let mut m = Map::new();
            for (k, v) in obj {
                if matches!(
                    k.as_str(),
                    "name" | "id" | "type" | "itemType" | "content" | "fields" | "worldBookId" | "world_book_id"
                ) {
                    continue;
                }
                m.insert(k.clone(), v.clone());
            }
            if m.is_empty() {
                None
            } else {
                Some(Value::Object(m))
            }
        } else {
            None
        }
    })
}

pub(crate) fn stable_partner_id(prefix_tag: &str, name: &str, salt: &str) -> String {
    let slug = slugify(name);
    let h = short_hash(&format!("{prefix_tag}|{name}|{salt}"));
    format!("{prefix_tag}-{slug}-{h}")
}

pub(crate) fn map_world_book(entry: &Value, name_prefix: &str, salt: &str) -> PartnerItem {
    let raw_name = entry_name(entry, "世界书");
    let name = if name_prefix.is_empty() {
        raw_name.clone()
    } else {
        format!("{name_prefix}{raw_name}")
    };
    let fields = entry_fields(entry);
    let content = if let Some(ref f) = fields {
        fields_to_content(&name, "world_book", f)
    } else if let Some(c) = entry.get("content").and_then(|v| v.as_str()) {
        c.to_string()
    } else {
        name.clone()
    };
    let id = entry
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| stable_partner_id("wb", &raw_name, salt));
    PartnerItem {
        id,
        name,
        item_type: "world_book".into(),
        content,
        fields,
        world_book_id: None,
    }
}

pub(crate) fn map_character_card(
    entry: &Value,
    name_prefix: &str,
    salt: &str,
    default_wb: Option<&str>,
) -> PartnerItem {
    let raw_name = entry_name(entry, "角色卡");
    let name = if name_prefix.is_empty() {
        raw_name.clone()
    } else {
        format!("{name_prefix}{raw_name}")
    };
    let fields = entry_fields(entry);
    let content = if let Some(ref f) = fields {
        fields_to_content(&name, "character_card", f)
    } else if let Some(c) = entry.get("content").and_then(|v| v.as_str()) {
        c.to_string()
    } else {
        name.clone()
    };
    let id = entry
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| stable_partner_id("cc", &raw_name, salt));
    let world_book_id = entry
        .get("worldBookId")
        .or_else(|| entry.get("world_book_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| default_wb.map(|s| s.to_string()));
    PartnerItem {
        id,
        name,
        item_type: "character_card".into(),
        content,
        fields,
        world_book_id,
    }
}

/// `POST /api/v1/background/apply` — upsert BG world books / character cards into Partner.
pub async fn apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BackgroundApplyBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // C2 审计修复：per-user 隔离，防跨租户写其他用户 partner store。
    let partner = state.partner.clone().scoped(&session.user_id);

    let result = if let Some(r) = body.result.clone() {
        r
    } else if let Some(run_id) = body
        .run_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match state.jobs.get(run_id) {
            None => {
                return not_found("BG_NOT_FOUND", format!("job not found: {run_id}"));
            }
            Some(j)
                if j.workspace_id != session.workspace_id && j.user_id != session.user_id =>
            {
                return forbidden("BG_FORBIDDEN_SCOPE", "job belongs to another workspace");
            }
            Some(j) => match j.result {
                Some(r) => r,
                None => {
                    return err_with_code(
            StatusCode::BAD_REQUEST,
            "BG_BAD_REQUEST", "job has no result yet",
            serde_json::json!({"runId": run_id,
                            "status": normalize_job_status(&j.status)}),
                    );
                }
            },
        }
    } else {
        return bad_request("BG_MISSING_FIELD", "result or runId is required");
    };

    let prefix = body.prefix.as_deref().unwrap_or("").to_string();
    let salt = body
        .run_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut world_books_in: Vec<Value> = Vec::new();
    if let Some(arr) = result.get("worldBooks").and_then(|v| v.as_array()) {
        world_books_in.extend(arr.iter().cloned());
    } else if let Some(one) = result.get("worldBook") {
        world_books_in.push(one.clone());
    }

    let mut cards_in: Vec<Value> = Vec::new();
    if let Some(arr) = result.get("characterCards").and_then(|v| v.as_array()) {
        cards_in.extend(arr.iter().cloned());
    }
    if let Some(one) = result.get("characterCard") {
        cards_in.push(one.clone());
    }

    if world_books_in.is_empty() && cards_in.is_empty() {
        return err_with_code(
            StatusCode::BAD_REQUEST,
            "BG_BAD_REQUEST", "result has no worldBooks / characterCards / characterCard",
            serde_json::json!({"ok": false}),
        );
    }

    let mut applied_wb: Vec<PartnerItem> = Vec::new();
    for entry in &world_books_in {
        let item = map_world_book(entry, &prefix, &salt);
        match partner.upsert_world_book(item) {
            Ok(saved) => applied_wb.push(saved),
            Err(e) => return map_core_err(e),
        }
    }

    let default_wb_id = if applied_wb.len() == 1 {
        Some(applied_wb[0].id.clone())
    } else {
        None
    };

    let mut applied_cc: Vec<PartnerItem> = Vec::new();
    for entry in &cards_in {
        let item = map_character_card(entry, &prefix, &salt, default_wb_id.as_deref());
        match partner.upsert_character_card(item) {
            Ok(saved) => applied_cc.push(saved),
            Err(e) => return map_core_err(e),
        }
    }

    let do_select = body.select.unwrap_or(true);
    let mut selected_wb: Option<String> = None;
    let mut selected_cc: Option<String> = None;
    if do_select {
        if let Some(first_cc) = applied_cc.first() {
            selected_cc = Some(first_cc.id.clone());
            selected_wb = first_cc
                .world_book_id
                .clone()
                .or_else(|| applied_wb.first().map(|w| w.id.clone()));
        } else if let Some(first_wb) = applied_wb.first() {
            selected_wb = Some(first_wb.id.clone());
        }
        if selected_wb.is_some() || selected_cc.is_some() {
            if let Err(e) = partner
                .select(selected_wb.clone(), selected_cc.clone())
            {
                return map_core_err(e);
            }
        }
    }

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "schemaVersion": 1,
            "kind": "background_apply",
            "worldBooks": applied_wb,
            "characterCards": applied_cc,
            "counts": {
                "worldBooks": applied_wb.len(),
                "characterCards": applied_cc.len(),
            },
            "selected": {
                "worldBookId": selected_wb,
                "characterCardId": selected_cc,
            },
            "runId": body.run_id,
            "workspaceId": session.workspace_id,
        })),
    )
        .into_response()
}
