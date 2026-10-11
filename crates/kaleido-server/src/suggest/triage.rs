//! suggest/triage —— 改编指令分诊 + 写作规则读写。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) const TRIAGE_SYS: &str = "你是改编干预分诊器。把用户指令分成一类，只输出 JSON，不要解释。类别：style（怎么写：笔法/风格/质量/禁用语/句式，任何章节都成立）、plot（写什么：剧情/结构/人物/篇幅/新走向）、rework（改已写的：重写/修订已有章节）、query（查询：问状态/设定/进度）。输出格式：{\"kind\":\"style|plot|rework|query\",\"answer\":\"给用户的回话\",\"rules\":\"style 类时要落盘的规则原文，否则空\",\"targets\":\"rework 类时的章节 id 列表（逗号分隔），否则空\",\"reason\":\"一句话理由\" }。判别口径：「怎么写」→style；「写什么」→plot；「改已写的」→rework；相对式指令（增加10章/重写第3章）绝不进 style。";
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TriageBody {
    #[serde(default)]
    instruction: String,
    #[serde(default)]
    model: String,
}

pub(crate) fn rules_path(pid: &str) -> String {
    format!("projects/{pid}/writing_rules.md")
}

pub(crate) fn read_rules(state: &AppState, ws: &str, pid: &str) -> String {
    state
        .works
        .read_text(ws, &rules_path(pid))
        .map(|b| b.content)
        .unwrap_or_default()
}

pub(crate) async fn triage_instruction(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
    Json(body): Json<TriageBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pid = match safe_id(&pid) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let instruction = body.instruction.trim().to_string();
    if instruction.is_empty() {
        return bad_request("TRIAGE_EMPTY", "instruction required");
    }
    // 章节计数（facts 快照）
    let stats = project_style_stats(&state, &session.workspace_id, &pid);
    let user = format!(
        "用户指令：{}\n当前事实：共{}章，总{}字。只输出 JSON。",
        instruction, stats.chapters, stats.total_chars
    );
    let (raw, _, _) = match call_llm_tracked_with(&state, &session.workspace_id, &pid, "triage", TRIAGE_SYS, &user, &body.model).await {
        Ok(r) => r,
        Err(e) => return internal("TRIAGE_LLM_FAILED", e),
    };
    let s = raw.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
        .trim();
    let v: Value = match serde_json::from_str(s)
        .or_else(|_| serde_json::from_str(&s[s.find('{').unwrap_or(0)..s.rfind('}').map(|i| i + 1).unwrap_or(s.len())]))
    {
        Ok(v) => v,
        Err(e) => return internal("TRIAGE_BAD_JSON", e.to_string()),
    };
    let kind = v.get("kind").and_then(|x| x.as_str()).unwrap_or("query").to_string();
    let answer = v.get("answer").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let rules = v.get("rules").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let targets = v.get("targets").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("").to_string();
    // ⑨ style → rules 落盘（追加）
    let mut ruled = false;
    if kind == "style" && !rules.trim().is_empty() {
        let mut cur = read_rules(&state, &session.workspace_id, &pid);
        if !cur.is_empty() && !cur.ends_with('\n') {
            cur.push('\n');
        }
        cur.push_str(&format!("- {}\n", rules.trim()));
        let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}"));
        if state.works.write_text(&session.workspace_id, &rules_path(&pid), &cur).is_ok() {
            ruled = true;
        }
    }
    // ⑩ rework → 批量 suggest 入队（每章一个 pending continue 建议）
    let mut queued: Vec<Value> = Vec::new();
    if kind == "rework" {
        for tid in targets.split([',', '、', ' ']).map(|x| x.trim()).filter(|x| !x.is_empty()) {
            let ch_id = match safe_id(tid) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if read_ch(&state, &session.workspace_id, &pid, &ch_id).is_err() {
                continue;
            }
            let sg_id = format!("sg-{}", &Uuid::new_v4().to_string()[..8]);
            let sg = Suggestion {
                id: sg_id.clone(),
                project_id: pid.clone(),
                chapter_id: ch_id.clone(),
                chapter_version: read_ch(&state, &session.workspace_id, &pid, &ch_id)
                    .map(|c| c.version_no)
                    .unwrap_or(1),
                task_type: "continue".into(),
                action: "append".into(),
                instruction: format!("按用户要求返工：{}", instruction),
                content: String::new(),
                status: "pending".into(),
                source_text: String::new(),
                created_at: now(),
            };
            if let Ok(s) = serde_json::to_string_pretty(&sg) {
                let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}/suggestions"));
                if state.works.write_text(&session.workspace_id, &sg_path(&pid, &sg_id), &s).is_ok() {
                    queued.push(json!({"suggestionId": sg_id, "chapterId": ch_id}));
                }
            }
        }
    }
    Json(json!({
        "ok": true,
        "kind": kind,
        "answer": answer,
        "rulesSaved": ruled,
        "queued": queued,
        "reason": reason,
    }))
    .into_response()
}

pub(crate) async fn get_writing_rules(
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
    Json(json!({"ok": true, "rules": read_rules(&state, &session.workspace_id, &pid)})).into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PutRulesBody {
    #[serde(default)]
    rules: String,
}

pub(crate) async fn put_writing_rules(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pid): Path<String>,
    Json(body): Json<PutRulesBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let pid = match safe_id(&pid) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}"));
    if let Err(e) = state.works.write_text(&session.workspace_id, &rules_path(&pid), &body.rules) {
        return map_core_err(e);
    }
    Json(json!({"ok": true})).into_response()
}
