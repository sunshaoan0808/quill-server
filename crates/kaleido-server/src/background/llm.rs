//! background/llm —— LLM 流式调用 + prompts + 结果规范化。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn try_background_llm_stream(
    state: &AppState,
    jobs: &kaleido_core::JobStore,
    run_id: &str,
    mode: &str,
    title: &str,
    premise: &str,
    character_name: &str,
    world_book_context: &str,
    include_names: bool,
) -> Option<serde_json::Value> {
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

    let (system, user, max_tokens, timeout_secs) =
        background_prompts(mode, title, premise, character_name, world_book_context, include_names);

    let mut chars: usize = 0;
    let jobs_c = jobs.clone();
    let run_c = run_id.to_string();
    let full = match crate::llm_stream::stream_chat_completions_dispatch(
        &llm.base_url,
        &llm.api_key,
        &model,
        &prov_kind,
        &system,
        &user,
        0.5,
        max_tokens,
        timeout_secs,
        |chunk| {
            if cancelled(&jobs_c, &run_c) {
                return false;
            }
            chars = chars.saturating_add(chunk.chars().count());
            // map rough progress 0.2 → 0.8 while streaming
            let p = (0.2 + (chars as f64 / 4000.0).min(0.55)).min(0.8);
            let _ = jobs_c.push_event(
                &run_c,
                JobEvent {
                    event_type: "delta".into(),
                    ts: chrono::Utc::now(),
                    message: None,
                    progress: Some(p),
                    code: None,
                    data: Some(json!({"delta": chunk, "stage": mode})),
                },
                Some(p),
                Some(format!("{mode}:delta")),
            );
            true
        },
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            if e != "cancelled" {
                let _ = jobs.push_event(
                    run_id,
                    JobEvent::event(
                        format!("llm soft-fail: {e}"),
                        Some(json!({"stage": mode, "error": e})),
                    ),
                    None,
                    Some(format!("{mode}:llm_fail")),
                );
            }
            return None;
        }
    };

    if cancelled(jobs, run_id) {
        return None;
    }

    let parsed = crate::llm_stream::extract_json_value(&full)?;
    Some(normalize_background_llm_result(
        mode,
        title,
        character_name,
        parsed,
        &full,
    ))
}

pub(crate) fn background_prompts(
    mode: &str,
    title: &str,
    premise: &str,
    character_name: &str,
    world_book_context: &str,
    include_names: bool,
) -> (String, String, u32, u64) {
    let text = crate::llm_stream::limit_text(premise, 6000);
    match mode {
        "items" => {
            let system = "你是一个世界观与人物设定专家。根据参考文本提取结构化世界书与角色卡列表。务必返回严格纯 JSON，不要 Markdown 或额外说明。".to_string();
            let user = format!(
                "标题：{title}\n\
请返回 JSON，结构：\n\
{{\n  \"worldBooks\": [ {{\"name\": \"...\", \"fields\": {{ \"theme\": \"...\", \"era\": \"...\", \"techLevel\": \"...\", \"magicLevel\": \"...\", \"geography\": \"...\", \"keyScenes\": \"...\", \"culturalFeatures\": \"...\", \"history\": \"...\", \"conflict\": \"...\" }} }} ],\n  \"characterCards\": [ {{\"name\": \"角色名\", \"fields\": {{ \"age\": \"...\", \"gender\": \"...\", \"occupation\": \"...\", \"coreDesire\": \"...\", \"backgroundStory\": \"...\", \"speakingStyle\": \"...\" }} }} ]\n}}\n\
重要：仅返回纯 JSON。\n\n参考内容：\n===========================\n{text}\n==========================="
            );
            (system, user, 4096, 180)
        }
        "character_card" => {
            let system = "你是一个人物设定专家。为指定角色生成一张结构化角色卡。务必返回严格纯 JSON，不要 Markdown 或额外说明。".to_string();
            let ctx = if world_book_context.trim().is_empty() {
                String::new()
            } else {
                format!(
                    "\n\n已确认的世界书上下文：\n{}\n",
                    crate::llm_stream::limit_text(world_book_context, 2000)
                )
            };
            let user = format!(
                "请只为角色「{character_name}」生成一张角色卡，不要生成其他角色。{ctx}\n\
JSON 结构：\n\
{{\"name\":\"{character_name}\",\"fields\":{{\"age\":\"...\",\"gender\":\"...\",\"race\":\"...\",\"birthplace\":\"...\",\"occupation\":\"...\",\"socialClass\":\"...\",\"identityTags\":[\"...\"],\"heightBuild\":\"...\",\"iconicFeatures\":\"...\",\"clothingStyle\":\"...\",\"overallVibe\":\"...\",\"externalPersonality\":\"...\",\"internalPersonality\":\"...\",\"coreDesire\":\"...\",\"fearWeakness\":\"...\",\"moralValues\":\"...\",\"quirk\":\"...\",\"skills\":\"...\",\"backgroundStory\":\"...\",\"relationships\":\"...\",\"speakingStyle\":\"...\",\"typicalReactions\":\"...\",\"userRelationType\":\"...\",\"userInteractionModel\":\"...\",\"userRelationBottomLine\":\"...\",\"keyEvents\":\"...\"}}}}\n\
约束：文本未给出的字段请谨慎概括，不要明显冲突；仅返回纯 JSON。\n\n参考内容：\n===========================\n{text}\n==========================="
            );
            (system, user, 4096, 180)
        }
        _ => {
            // stage_one
            let system = "你是一个世界观与人物设定专家。根据用户提供的参考文本提取结构化世界书；如果任务要求，还要提取适合继续生成角色卡的角色姓名列表。务必返回严格纯 JSON，不要 Markdown 或额外说明。".to_string();
            let names_inst = if include_names {
                "\n同时提取需要生成角色卡的角色姓名列表，字段名为 characterNames。角色名只保留姓名或常用称呼，不要附带解释。"
            } else {
                "\n本次仅提取世界书，不要输出 characterNames，也不要输出角色卡。"
            };
            let schema = if include_names {
                r#"{
  "worldBooks": [ { "name": "世界书名", "fields": { "theme": "...", "era": "...", "techLevel": "...", "magicLevel": "...", "geography": "...", "keyScenes": "...", "culturalFeatures": "...", "history": "...", "conflict": "..." } } ],
  "characterNames": ["角色姓名1", "角色姓名2"]
}"#
            } else {
                r#"{
  "worldBooks": [ { "name": "世界书名", "fields": { "theme": "...", "era": "...", "techLevel": "...", "magicLevel": "...", "geography": "...", "keyScenes": "...", "culturalFeatures": "...", "history": "...", "conflict": "..." } } ]
}"#
            };
            let user = format!(
                "标题：{title}\n{names_inst}\nJSON 必须严格满足：\n{schema}\n\n重要：仅返回纯 JSON。\n\n参考内容：\n===========================\n{text}\n==========================="
            );
            (system, user, 3072, 150)
        }
    }
}

pub(crate) fn normalize_background_llm_result(
    mode: &str,
    title: &str,
    character_name: &str,
    parsed: Value,
    raw: &str,
) -> Value {
    // Accept either top-level structured fields or nested under data.
    let root = if parsed.get("worldBooks").is_some()
        || parsed.get("characterCards").is_some()
        || parsed.get("characterCard").is_some()
        || parsed.get("characterNames").is_some()
        || parsed.get("name").is_some()
    {
        parsed
    } else if let Some(d) = parsed.get("data").cloned() {
        d
    } else {
        parsed
    };

    match mode {
        "items" => {
            let world_books = root
                .get("worldBooks")
                .cloned()
                .unwrap_or_else(|| json!([]));
            let cards = root
                .get("characterCards")
                .cloned()
                .or_else(|| root.get("characters").cloned())
                .unwrap_or_else(|| json!([]));
            json!({
                "kind": "background",
                "stage": "items",
                "mode": "items",
                "title": title,
                "worldBooks": world_books,
                "characterCards": cards,
                "ok": true,
                "fallback": false,
                "generationMode": "llm",
                "raw": raw.chars().take(4000).collect::<String>(),
            })
        }
        "character_card" => {
            let card = if root.get("fields").is_some() || root.get("name").is_some() {
                // single card object
                if root.get("name").is_none() {
                    let mut o = root.as_object().cloned().unwrap_or_default();
                    o.insert("name".into(), json!(character_name));
                    Value::Object(o)
                } else {
                    root.clone()
                }
            } else {
                root.get("characterCard")
                    .cloned()
                    .unwrap_or(root.clone())
            };
            json!({
                "kind": "background",
                "stage": "character_card",
                "mode": "character_card",
                "title": title,
                "characterName": character_name,
                "characterCard": card,
                "ok": true,
                "fallback": false,
                "generationMode": "llm",
                "raw": raw.chars().take(4000).collect::<String>(),
            })
        }
        _ => {
            let world_books = root
                .get("worldBooks")
                .cloned()
                .unwrap_or_else(|| json!([]));
            let names = root
                .get("characterNames")
                .cloned()
                .unwrap_or_else(|| json!([]));
            json!({
                "kind": "background",
                "stage": "stage_one",
                "mode": "stage_one",
                "title": title,
                "worldBooks": world_books,
                "characterNames": names,
                "ok": true,
                "fallback": false,
                "generationMode": "llm",
                "raw": raw.chars().take(4000).collect::<String>(),
            })
        }
    }
}
