//! p009/archive —— 存档分析/精炼/清空 + 角色抽取触发（call_llm_extract_with 共享助手）。（R6② 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn call_llm_extract_with(appst: &kaleido_core::AppStateStore, base: Option<&str>, key: Option<&str>, model: &str, system: &str, user: &str) -> Result<String, String> {
    let llm = appst.resolve_llm(base, key, model);
    if llm.base_url.trim().is_empty() || llm.api_key.trim().is_empty() {
        return Err("LLM not configured".into());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .pool_idle_timeout(std::time::Duration::from_secs(4))
        .pool_max_idle_per_host(2)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/chat/completions", llm.base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": llm.model,
        "stream": false,
        "temperature": 0.1,
        "max_tokens": 512,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    });
    let resp = client.post(&url).bearer_auth(&llm.api_key)
        .header("content-type", "application/json").json(&body)
        .send().await.map_err(|e| format!("extract request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("extract status {}", resp.status()));
    }
    let v: serde_json::Value = resp.json().await.map_err(|e| format!("extract read failed: {e}"))?;
    v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("message")).and_then(|m| m.get("content")).and_then(|c| c.as_str())
        .map(|s| s.to_string()).ok_or_else(|| "extract empty".to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveAnalyzeReq {
    #[serde(default)]
    character_id: Option<String>,
    #[serde(default)]
    character_name: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    recent_messages: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ArchiveRefineReq {
    #[serde(default)]
    character_id: Option<String>,
    #[serde(default)]
    character_name: Option<String>,
}

/// 剥 JSON 围栏（```json ... ``` / 前后缀文字 / 尾注思考文本），提取第一个完整 JSON 对象。
pub(crate) fn strip_json_fence(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    // 直接尝试整体解析
    if serde_json::from_str::<serde_json::Value>(s).is_ok() {
        return s.to_string();
    }
    // 括号深度扫描提取第一个完整 JSON 对象(吸收 extract_json_value 健壮版思想,
    // 避免 LLM 在 JSON 后追加思考文本时 rfind('}') 截到尾注里的花括号)
    let bytes = s.as_bytes();
    if let Some(a) = s.find('{') {
        let mut depth = 0i32;
        let mut in_str = false;
        let mut esc = false;
        for i in a..bytes.len() {
            let c = bytes[i] as char;
            if in_str {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            match c {
                '"' => in_str = true,
                '{' | '[' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return s[a..=i].to_string();
                    }
                }
                ']' => depth -= 1,
                _ => {}
            }
        }
    }
    s.to_string()
}

pub(crate) fn find_char_idx(pack: &StoryPack, id: Option<&str>, name: Option<&str>) -> Option<usize> {
    pack.characters.iter().position(|c| {
        (id.is_some() && c.id == id.unwrap_or_default())
            || (name.is_some() && c.name == name.unwrap_or_default())
    })
}

/// 会话消息 → 近期对话文本（角色: 内容 换行）。
pub(crate) fn session_messages_to_text(sess: &TavernSession, max_msgs: usize) -> String {
    let start = sess.messages.len().saturating_sub(max_msgs);
    let mut parts = Vec::new();
    for m in sess.messages.iter().skip(start) {
        let name = if m.role.is_empty() { "?" } else { m.role.as_str() };
        parts.push(format!("{name}: {}", m.content));
    }
    parts.join("\n")
}

/// POST /api/v1/story-tavern/packs/{id}/archive/analyze
/// body: { characterId? | characterName?, sessionId? | recentMessages? }
/// LLM 增量分析 → apply_diff → 落盘 → 返回变更列表 + 新档案。
pub(crate) async fn archive_analyze(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ArchiveAnalyzeReq>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let mut pack = match state.packs.get(&id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let Some(idx) = find_char_idx(&pack, body.character_id.as_deref(), body.character_name.as_deref()) else {
        return not_found("ST_CHAR_NOT_IN_PACK", "character not found in pack");
    };

    // 近期对话：优先 session_id（从会话取），否则用 recentMessages 原文
    let recent = if let Some(sid) = body.session_id.as_deref() {
        match state.sessions_tavern.get_for_owner(sid, &session.user_id) {
            Ok(s) => session_messages_to_text(&s, 30),
            Err(_) => body.recent_messages.clone().unwrap_or_default(),
        }
    } else {
        body.recent_messages.clone().unwrap_or_default()
    };
    if recent.trim().is_empty() {
        return bad_request("ST_CONTEXT_REQUIRED", "recentMessages or sessionId required");
    }

    let ch = &pack.characters[idx];
    let archive = ch.archive.clone().unwrap_or_default();
    let registered: Vec<String> = pack.characters.iter().map(|c| c.name.clone()).collect();
    let user = serde_json::json!({
        "character": ch.name,
        "turn_index": 0,
        "current_profile": archive.serialize_for_prompt(),
        "registered_characters": registered,
        "recent_messages": recent,
    })
    .to_string();

    let raw = match call_llm_nonstream(&state, crate::archive_prompts::ARCHIVESYSTEM, &user).await {
        Ok(s) => s,
        Err(e) => return bad_gateway("ST_UPSTREAM_LLM", e),
    };
    let cleaned = strip_json_fence(&raw);
    let diff: kaleido_core::character_archive::ArchiveDiff = match serde_json::from_str(&cleaned) {
        Ok(d) => d,
        Err(e) => {
            return err_with_code(
                StatusCode::UNPROCESSABLE_ENTITY,
                "ST_DIFF_PARSE",
                format!("diff parse: {e}"),
                serde_json::json!({ "raw": raw.chars().take(600).collect::<String>() }),
            );
        }
    };

    let mut archive = pack.characters[idx].archive.clone().unwrap_or_default();
    let source = body
        .session_id
        .clone()
        .unwrap_or_else(|| format!("manual-{}", ch.id));
    let changes = kaleido_core::character_archive::apply_diff(&mut archive, &diff, Some(&source));
    pack.characters[idx].archive = Some(archive);
    if let Err(e) = state.packs.save(pack) {
        return map_core_err(e);
    }
    Json(json!({
        "ok": true,
        "changes": changes,
        "profile": state.packs.get(&id).map(|p| p.characters[idx].archive.clone().unwrap_or_default().serialize_for_prompt()).unwrap_or(Value::Null),
    }))
    .into_response()
}

/// POST /api/v1/story-tavern/packs/{id}/archive/refine
/// body: { characterId? | characterName? }
/// LLM 精编（规范格式 + 提炼浓缩，不新增信息）→ apply_refine → 落盘。
pub(crate) async fn archive_refine(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ArchiveRefineReq>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let mut pack = match state.packs.get(&id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let Some(idx) = find_char_idx(&pack, body.character_id.as_deref(), body.character_name.as_deref()) else {
        return not_found("ST_CHAR_NOT_IN_PACK", "character not found in pack");
    };
    let ch = &pack.characters[idx];
    let archive = ch.archive.clone().unwrap_or_default();
    let user = serde_json::json!({
        "character": ch.name,
        "current_profile": archive.serialize_for_prompt(),
    })
    .to_string();

    let raw = match call_llm_nonstream(&state, crate::archive_prompts::ARCHIVEREFINE, &user).await {
        Ok(s) => s,
        Err(e) => return bad_gateway("ST_UPSTREAM_LLM", e),
    };
    let cleaned = strip_json_fence(&raw);
    let refined: Value = match serde_json::from_str(&cleaned) {
        Ok(v) => v,
        Err(e) => {
            return err_with_code(
                StatusCode::UNPROCESSABLE_ENTITY,
                "ST_REFINE_PARSE",
                format!("refine parse: {e}"),
                serde_json::json!({ "raw": raw.chars().take(600).collect::<String>() }),
            );
        }
    };

    let mut archive = pack.characters[idx].archive.clone().unwrap_or_default();
    let changes = kaleido_core::character_archive::apply_refine(&mut archive, &refined);
    pack.characters[idx].archive = Some(archive);
    if let Err(e) = state.packs.save(pack) {
        return map_core_err(e);
    }
    Json(json!({
        "ok": true,
        "changes": changes,
        "profile": state.packs.get(&id).map(|p| p.characters[idx].archive.clone().unwrap_or_default().serialize_for_prompt()).unwrap_or(Value::Null),
    }))
    .into_response()
}

/// DELETE /api/v1/story-tavern/packs/{id}/archive/purge/{source}
/// 删除来源（消息楼层/会话 id）关联的档案条目（删楼联动清理）。
pub(crate) async fn archive_purge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((id, source)): Path<(String, String)>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let mut pack = match state.packs.get(&id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let mut removed = 0usize;
    for ch in pack.characters.iter_mut() {
        if let Some(archive) = ch.archive.as_mut() {
            removed += kaleido_core::character_archive::purge_by_source(archive, &source);
        }
    }
    if removed > 0 {
        if let Err(e) = state.packs.save(pack) {
            return map_core_err(e);
        }
    }
    Json(json!({ "ok": true, "removed": removed })).into_response()
}

/// POST /api/v1/story-tavern/packs/{id}/cast-extract
/// 存量 pack 角色补抽：手动触发 LLM 角色抽取（异步 spawn，立即返回 started）。
/// 阈值相对判断 / body_path 断链容错 / 会话另存 pack 检测全部在 spawn_auto_cast_extraction 内。
pub(crate) async fn trigger_cast_extract(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(r) = session_from(&state, &headers) {
        return r;
    }
    let pack = match state.packs.get(&id) {
        Ok(p) => p,
        Err(e) => return map_core_err(e),
    };
    let real: Vec<String> = pack
        .characters
        .iter()
        .filter(|c| c.role != "narrator" && c.role != "player")
        .map(|c| c.name.clone())
        .collect();
    crate::crawler::spawn_auto_cast_extraction(state.clone(), id.clone());
    Json(json!({
        "ok": true,
        "packId": id,
        "status": "started",
        "existingCastCount": real.len(),
    }))
    .into_response()
}

