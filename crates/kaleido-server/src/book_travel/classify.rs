//! book_travel/classify —— 类型判定：LLM 优先 + 启发式回退、标签解析、上一轮结果装载。（R6④a 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) fn normalize_step(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_lowercase().replace('-', "_");
    match s.as_str() {
        "assemble" | "plan" | "start" => Some("assemble".into()),
        "plan_scene" | "planscene" | "scene" => Some("plan_scene".into()),
        "write_change_scene" | "change_scene" | "change" => Some("write_change_scene".into()),
        "write_insert_beat" | "insert_beat" | "beat" | "insert" => Some("write_insert_beat".into()),
        "judge_ending" | "ending" | "judge" => Some("judge_ending".into()),
        "summarize_memory" | "memory" | "summarize" => Some("summarize_memory".into()),
        "pipeline" | "full" | "all" | "journey" | "full_pipeline" => Some("pipeline".into()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub async fn classify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BookTravelClassifyBody>,
) -> axum::response::Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let text = body.text.as_deref().unwrap_or("").trim();
    if text.is_empty() && body.title.as_deref().unwrap_or("").trim().is_empty() {
        return bad_request("BT_INPUT", "需要 text 或 title");
    }

    let title = body.title.as_deref().unwrap_or("").trim();
    let genre_hint = body.genre_hint.as_deref();

    let (labels, generation_mode, fallback) =
        match try_classify_llm(&state, text, title, genre_hint).await {
            Some(labels) => (labels, "llm", false),
            None => (
                heuristic_classify(text, body.title.as_deref(), genre_hint),
                "heuristic",
                true,
            ),
        };

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "labels": labels,
            "workspaceId": session.workspace_id,
            "userId": session.user_id,
            "generationMode": generation_mode,
            "mvp": true,
            "fallback": fallback,
        })),
    )
        .into_response()
}

/// Soft-fail LLM classify: returns None on missing LLM config, transport, or parse errors.
pub(crate) async fn try_classify_llm(
    state: &AppState,
    text: &str,
    title: &str,
    genre_hint: Option<&str>,
) -> Option<BookTravelLabels> {
    let llm = state.app_state.resolve_llm(
        state.llm_base.as_deref(),
        state.llm_key.as_deref(),
        &state.llm_model,
    );
    let prov_kind = crate::llm_stream::runtime_provider_kind(&llm, &state.provider_kind);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return None;
    }
    let model = if llm.model.is_empty() {
        state.llm_model.clone()
    } else {
        llm.model.clone()
    };

    let system = r#"你是小说题材分类助手。根据用户提供的标题与正文片段，输出严格纯 JSON（不要 Markdown 代码块），字段必须齐全：
{
  "genre": string,
  "tone": string,
  "pov": string,
  "era": string,
  "themes": string[],
  "pacing": string,
  "audience": string,
  "mode": "llm",
  "confidence": number,
  "notes": string[]
}
genre 可用：science_fiction, fantasy, mystery, romance, horror, historical, xianxia, general_fiction 等。
tone：dark/light/epic/neutral；pov：first_person/second_person/third_person；
era：near_future/pre_modern/historical_modern/contemporary；
pacing：slow/moderate/fast；audience：children/young_adult/adult。
confidence 在 0~1 之间。只返回 JSON。"#.to_string();

    let text_l = crate::llm_stream::limit_text(text, 4000);
    let mut user = format!(
        "标题：{}
正文片段：
{}",
        if title.is_empty() { "(无)" } else { title },
        if text_l.is_empty() { "(无)" } else { &text_l }
    );
    if let Some(h) = genre_hint.map(str::trim).filter(|s| !s.is_empty()) {
        user.push_str(&format!("
用户 genre 提示：{h}"));
    }

    let full = match crate::llm_stream::stream_chat_completions_dispatch(
        &llm.base_url,
        &llm.api_key,
        &model,
        &prov_kind,
        &system,
        &user,
        0.3,
        800,
        60,
        |_chunk| true,
    )
    .await
    {
        Ok(s) => s,
        Err(_) => return None,
    };

    let v = crate::llm_stream::extract_json_value(&full)?;
    labels_from_json(&v, genre_hint)
}

pub(crate) fn labels_from_json(v: &Value, genre_hint: Option<&str>) -> Option<BookTravelLabels> {
    let str_field = |key: &str| -> Option<String> {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let mut genre = str_field("genre")?;
    if let Some(h) = genre_hint.map(str::trim).filter(|s| !s.is_empty()) {
        // honor explicit hint when present
        genre = h.to_string();
    }
    let tone = str_field("tone").unwrap_or_else(|| "neutral".into());
    let pov = str_field("pov").unwrap_or_else(|| "third_person".into());
    let era = str_field("era").unwrap_or_else(|| "contemporary".into());
    let pacing = str_field("pacing").unwrap_or_else(|| "moderate".into());
    let audience = str_field("audience").unwrap_or_else(|| "adult".into());
    let themes: Vec<String> = v
        .get("themes")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let themes = if themes.is_empty() {
        vec!["discovery".into()]
    } else {
        themes
    };
    let confidence = v
        .get("confidence")
        .and_then(|c| c.as_f64())
        .unwrap_or(0.7)
        .clamp(0.0, 1.0);
    let mut notes: Vec<String> = v
        .get("notes")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if notes.is_empty() {
        notes.push("classified by llm".into());
    }

    Some(BookTravelLabels {
        genre,
        tone,
        pov,
        era,
        themes,
        pacing,
        audience,
        mode: "llm".into(),
        confidence,
        notes,
    })
}

pub(crate) fn context_is_empty(ctx: &Option<Value>) -> bool {
    match ctx {
        None => true,
        Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Object(m)) => m.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        Some(_) => false,
    }
}

pub(crate) fn load_previous_result(
    state: &AppState,
    session: &kaleido_core::SessionRecord,
    previous_run_id: &str,
) -> Result<Value, axum::response::Response> {
    let run_id = previous_run_id.trim();
    if run_id.is_empty() {
        return Err(bad_request("BT_EMPTY", "previousRunId is empty"));
    }
    match state.jobs.get(run_id) {
        None => Err(not_found("BT_NOT_FOUND", format!("previous job not found: {run_id}"))),
        Some(j) if j.workspace_id != session.workspace_id && j.user_id != session.user_id => Err(forbidden("BT_FORBIDDEN_SCOPE", "previous job belongs to another workspace")),
        Some(j) => match j.result {
            Some(r) => Ok(r),
            None => Err(err_with_code(
            StatusCode::BAD_REQUEST,
            "BT_BAD_REQUEST", "previous job has no result yet",
            serde_json::json!({"previousRunId": run_id,
                    "status": normalize_job_status(&j.status)}))),
        },
    }
}
