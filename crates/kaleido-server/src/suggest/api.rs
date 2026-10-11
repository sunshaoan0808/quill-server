//! suggest/api —— create/list_suggests + get_stats。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn create_suggest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pid, ch)): Path<(String, String)>,
    Json(body): Json<SuggestBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (pid, ch) = match (safe_id(&pid), safe_id(&ch)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return bad_request("CHAPTER_BAD_ID", "invalid id"),
    };
    let task_type = body.task_type;
    if task_type != "continue" && task_type != "polish" {
        return bad_request("SUGGEST_BAD_TASK", "taskType must be continue|polish");
    }
    let action = body.action;
    if action != "append" && action != "replace" {
        return bad_request("SUGGEST_BAD_ACTION", "action must be append|replace");
    }
    let doc = match read_ch(&state, &session.workspace_id, &pid, &ch) {
        Ok(d) => d,
        Err(r) => return r,
    };
    // ⑪ 逐章验收门禁（review 下无本章许可即 409）
    let adv = read_advance(&state, &session.workspace_id, &pid);
    if adv.mode == "review" && adv.permit_chapter != ch {
        return Json(json!({"ok": false, "code": "ADVANCE_HOLD", "error": "逐章验收模式：请先放行本章（POST /permit）"})).into_response();
    }
    let instruction = body.instruction;
    let source_text = body.source_text;
    // ① 相关章节推荐（ainovel-cli 四维思路：focus=指令+标题，与历史章节关键词交集 top5）
    let focus = format!("{} {}", doc.title, instruction);
    let related = related_chapters(&state, &session.workspace_id, &pid, &ch, &focus);
    let related_block = if related.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = related
            .iter()
            .map(|(id, title, reason)| format!("- 《{}》（{}）：{}", title, id, reason))
            .collect();
        format!("\n\n相关历史章节（写作时注意承接）：\n{}", lines.join("\n"))
    };
    // ⑨ 写作规则注入（ainovel-cli user_rules 同款：后续 suggest/guard 自动带上）
    let writing_rules = read_rules(&state, &session.workspace_id, &pid);
    let rules_block = if writing_rules.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n本书写作规则（必须遵守）：\n{}", writing_rules)
    };
    // LLM 生成候选（不写正文）
    let sys = "你是长篇小说续写助手。根据章节正文和指令生成续写/润色候选。只输出候选正文，不要解释。";
    let user = if task_type == "continue" {
        format!("章节《{}》正文：\n{}\n\n指令：{}{}{}\n\n请续写下一段（300-800字）：", doc.title, doc.content, instruction, related_block, rules_block)
    } else {
        format!("章节《{}》正文：\n{}\n\n待润色选区：\n{}\n\n指令：{}{}{}\n\n请输出润色后文本：", doc.title, doc.content, source_text, instruction, related_block, rules_block)
    };
    let (content, _, _) = match call_llm_tracked_with(&state, &session.workspace_id, &pid, "suggest", sys, &user, &body.model).await {
        Ok(v) => v,
        Err(e) => return internal("SUGGEST_LLM_FAILED", e),
    };
    let sg_id = format!("sg-{}", &Uuid::new_v4().to_string()[..8]);
    let sg = Suggestion {
        id: sg_id.clone(),
        project_id: pid.clone(),
        chapter_id: ch.clone(),
        chapter_version: doc.version_no,
        task_type,
        action,
        instruction,
        content,
        status: "pending".into(),
        source_text,
        created_at: now(),
    };
    let s = serde_json::to_string_pretty(&sg).map_err(|e| internal("SUGGEST_SERIALIZE", e.to_string()));
    let s = match s {
        Ok(v) => v,
        Err(r) => return r,
    };
    let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}/suggestions"));
    if let Err(e) = state.works.write_text(&session.workspace_id, &sg_path(&pid, &sg_id), &s) {
        return map_core_err(e);
    }
    let related_json: Vec<Value> = related
        .iter()
        .map(|(id, title, reason)| json!({"chapterId": id, "title": title, "reason": reason}))
        .collect();
    Json(json!({"ok": true, "suggestion": sg, "relatedChapters": related_json})).into_response()
}

pub(crate) async fn list_suggests(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pid = match safe_id(&pid) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let mut out = Vec::new();
    let dir = format!("projects/{pid}/suggestions");
    if let Ok(entry) = state.works.list(&session.workspace_id, &dir, 1) {
        for child in entry.children {
            if child.kind == "dir" {
                continue;
            }
            let rel = format!("{dir}/{}", child.name);
            if let Ok(body) = state.works.read_text(&session.workspace_id, &rel) {
                if let Ok(sg) = serde_json::from_str::<Suggestion>(&body.content) {
                    out.push(json!({"id": sg.id, "chapterId": sg.chapter_id, "taskType": sg.task_type, "action": sg.action, "status": sg.status, "createdAt": sg.created_at}));
                }
            }
        }
    }
    Json(json!({"ok": true, "count": out.len(), "suggestions": out})).into_response()
}

pub(crate) async fn get_stats(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pid = match safe_id(&pid) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let stats = project_style_stats(&state, &session.workspace_id, &pid);
    Json(json!({"ok": true, "stats": stats})).into_response()
}
