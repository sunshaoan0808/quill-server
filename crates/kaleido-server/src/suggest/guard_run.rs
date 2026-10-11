//! suggest/guard_run —— run_guard 执行守卫 / accept / posthoc 后验 / reject。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) async fn run_guard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pid, sg)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (pid, sg) = match (safe_id(&pid), safe_id(&sg)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return bad_request("SUGGEST_BAD_ID", "invalid id"),
    };
    let doc = match read_sg(&state, &session.workspace_id, &pid, &sg) {
        Ok(d) => d,
        Err(r) => return r,
    };
    // 门 0：只有续写建议可跑守卫
    if doc.task_type != "continue" {
        return Json(json!({"ok": false, "code": "GUARD_NOT_APPLICABLE", "error": "只有续写建议可以运行一致性检查"})).into_response();
    }
    let ch = match read_ch(&state, &session.workspace_id, &pid, &doc.chapter_id) {
        Ok(d) => d,
        Err(r) => return r,
    };
    // 门 1：正文版本必须一致
    if ch.version_no != doc.chapter_version {
        return Json(json!({"ok": false, "code": "STALE_SUGGESTION", "error": "正文版本已变化，请重新生成建议"})).into_response();
    }
    // contextRefs：章节版本 + 内容哈希（Kaleido 化简版，原版是人物/设定/大纲/伏笔快照）
    let ctx = json!({
        "chapterId": ch.id, "chapterVersion": ch.version_no,
        "chapterHash": hash_content(&ch.content),
    });
    // ⑤ 伏笔账龄口径 + ⑦ 全书统计喂给守卫
    let stats = project_style_stats(&state, &session.workspace_id, &pid);
    let stats_line = format!(
        "\n\n全书统计：共{}章，总{}字，章均{}字，破折号{}处，三段式信号{}处，章末短句率{:.0}%（{}/{}），高频词{}。检查：破折号是否全书泛滥、同一高频词是否疲劳、章末是否全是短句（ainovel-cli style_stats 口径）。\n伏笔健康：已写{}章，超过 5 章未推进的伏笔必须出 foreshadow 维 issue；新伏笔必须有回收方向，否则出 issue。",
        stats.chapters,
        stats.total_chars,
        stats.avg_chars,
        stats.em_dashes,
        stats.triple_lists,
        stats.short_ending_rate * 100.0,
        stats.short_endings,
        stats.chapters,
        stats
            .top_phrases
            .iter()
            .map(|(w, n)| format!("{}×{}", w, n))
            .collect::<Vec<_>>()
            .join("/"),
        stats.chapters,
    );
    // ① 相关章节也喂给守卫（对照物更全）
    let focus_g = format!("{} {}", ch.title, doc.instruction);
    let related_g = related_chapters(&state, &session.workspace_id, &pid, &doc.chapter_id, &focus_g);
    let mut related_ctx = String::new();
    for (id, title, _reason) in related_g.iter().take(3) {
        let rel_path = format!("projects/{pid}/chapters/{id}.json");
        if let Ok(body) = state.works.read_text(&session.workspace_id, &rel_path) {
            if let Ok(rch) = serde_json::from_str::<ChapterDoc>(&body.content) {
                let excerpt: String = rch.content.chars().take(500).collect();
                related_ctx.push_str(&format!("\n\n历史章节《{}》摘要：\n{}", title, excerpt));
            }
        }
    }
    let user = format!(
        "检查下面的续写候选是否与提供的上下文冲突。输出 JSON 数组，没有冲突时输出 []。\n每项字段必须为：type（character/location/time/world/outline/foreshadow）、severity（low/medium/high）、title、description（必须引用候选正文原文 10 字以上举证）、candidateQuote、sourceRefs（数组）、suggestion。\n不得把文风偏好当成事实冲突，不得使用 Markdown 代码块。\n续写候选：\n{}\n\n章节正文：\n{}{}",
        doc.content, ch.content, format!("{}{}", related_ctx, stats_line)
    );
    let raw = match call_llm_tracked(&state, &session.workspace_id, &pid, "guard", GUARD_SYS, &user).await {
        Ok((r, _, _)) => r,
        Err(e) => {
            // failed 状态落盘（原版同款）
            let g = ContinuationGuard {
                suggestion_id: sg.clone(),
                chapter_version: ch.version_no,
                content_hash: hash_content(&doc.content),
                status: "failed".into(),
                issues: vec![],
                dimensions: vec![],
                context_refs: ctx,
                failure: e,
            };
            let s = serde_json::to_string_pretty(&g).unwrap_or_default();
            let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}/guards"));
            let _ = state.works.write_text(&session.workspace_id, &guard_path(&pid, &sg), &s);
            return Json(json!({"ok": true, "guard": g})).into_response();
        }
    };
    let (issues, mut dims) = match parse_guard(&raw) {
        Ok(v) => v,
        Err(first_err) => {
            // [waidrin ②] schema 拒收返工：短提示重试一次（DeterminFlow output_validation 同款）
            let retry_user = format!(
                "对下面的续写候选做一致性检查。只输出 JSON 数组，不要解释，不要 markdown 包裹。每项必须带 type（character/location/time/world/outline/foreshadow 之一）+ severity（low/medium/high 之一）+ title + description（引用候选原文 10 字以上）：\n{}",
                doc.content.chars().take(2000).collect::<String>()
            );
            let retry_out: Option<(Vec<GuardIssue>, Vec<DimensionScore>)> =
                if let Ok((retry_raw, _, _)) = call_llm_tracked(&state, &session.workspace_id, &pid, "guard", "你是小说评审，只输出 JSON。", &retry_user).await {
                    parse_guard(&retry_raw).ok()
                } else { None };
            let Some((rissues, rdims)) = retry_out else {
                let g = ContinuationGuard {
                    suggestion_id: sg.clone(),
                    chapter_version: ch.version_no,
                    content_hash: hash_content(&doc.content),
                    status: "failed".into(),
                    issues: vec![],
                    dimensions: vec![],
                    context_refs: ctx,
                    failure: format!("guard parse failed twice: {first_err}"),
                };
                let s = serde_json::to_string_pretty(&g).unwrap_or_default();
                let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}/guards"));
                let _ = state.works.write_text(&session.workspace_id, &guard_path(&pid, &sg), &s);
                return Json(json!({"ok": true, "guard": g})).into_response();
            };
            (rissues, rdims)
        }
    };
    // dimensions 为空时重试一次（只问七维评分，低成本补齐）
    if dims.is_empty() {
        let retry_user = format!(
            "对下面的续写候选按七维打分 0-100，只输出 JSON 对象，不要解释：\n{{\"dimensions\":{{\"consistency\":{{\"score\":N,\"comment\":\"…\"}},\"character\":{{…}},\"pacing\":{{…}},\"continuity\":{{…}},\"foreshadow\":{{…}},\"hook\":{{…}},\"aesthetic\":{{…}}}}}}\n\n续写候选：\n{}",
            doc.content.chars().take(2000).collect::<String>()
        );
        if let Ok((retry_raw, _, _)) = call_llm_tracked(&state, &session.workspace_id, &pid, "guard", "你是小说评审，只输出 JSON。", &retry_user).await {
            if let Ok((_, rdims)) = parse_guard(&format!("[]\n{}", retry_raw)) {
                if !rdims.is_empty() {
                    dims = rdims;
                }
            }
        }
    }
    let g = ContinuationGuard {
        suggestion_id: sg.clone(),
        chapter_version: ch.version_no,
        content_hash: hash_content(&doc.content),
        status: if issues.is_empty() { "clear".into() } else { "warning".into() },
        issues,
        dimensions: dims,
        context_refs: ctx,
        failure: String::new(),
    };
    let s = serde_json::to_string_pretty(&g).map_err(|e| internal("GUARD_SERIALIZE", e.to_string()));
    let s = match s {
        Ok(v) => v,
        Err(r) => return r,
    };
    let _ = state.works.mkdir(&session.workspace_id, &format!("projects/{pid}/guards"));
    if let Err(e) = state.works.write_text(&session.workspace_id, &guard_path(&pid, &sg), &s) {
        return map_core_err(e);
    }
    Json(json!({"ok": true, "guard": g})).into_response()
}

pub(crate) async fn accept_suggest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pid, sg)): Path<(String, String)>,
    Json(body): Json<AcceptBody>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (pid, sg) = match (safe_id(&pid), safe_id(&sg)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return bad_request("SUGGEST_BAD_ID", "invalid id"),
    };
    let mut doc = match read_sg(&state, &session.workspace_id, &pid, &sg) {
        Ok(d) => d,
        Err(r) => return r,
    };
    // 门 1：pending
    if doc.status != "pending" {
        return Json(json!({"ok": false, "code": "SUGGESTION_DECIDED", "error": "该建议已经处理"})).into_response();
    }
    // 门 2：问答/note 类不可写入正文（本书架只有 continue/polish，polish 走 replace 需 sourceText）
    let ch = match read_ch(&state, &session.workspace_id, &pid, &doc.chapter_id) {
        Ok(d) => d,
        Err(r) => return r,
    };
    // 门 3：正文版本必须一致
    if ch.version_no != doc.chapter_version {
        return Json(json!({"ok": false, "code": "STALE_SUGGESTION", "error": "正文版本已变化，请重新生成建议"})).into_response();
    }
    let content = body.content.unwrap_or_else(|| doc.content.clone());
    // 门 4（continue）：守卫必须 clear
    if doc.task_type == "continue" {
        let guard = read_guard(&state, &session.workspace_id, &pid, &sg);
        let guard = match guard {
            Some(g) => g,
            None => return Json(json!({"ok": false, "code": "GUARD_REQUIRED", "error": "续写建议尚未完成一致性检查"})).into_response(),
        };
        if guard.status == "failed" {
            return Json(json!({"ok": false, "code": "GUARD_FAILED", "error": "续写一致性检查失败，请重新运行检查后再采纳"})).into_response();
        }
        if guard.chapter_version != ch.version_no || guard.content_hash != hash_content(&content) {
            return Json(json!({"ok": false, "code": "GUARD_STALE", "error": "续写内容或正文版本已变化，请重新运行一致性检查"})).into_response();
        }
        // 门 4b：contextRefs 修订比对（章节哈希）
        let cur_hash = hash_content(&ch.content);
        if guard.context_refs.get("chapterHash").and_then(|v| v.as_str()) != Some(&cur_hash) {
            return Json(json!({"ok": false, "code": "GUARD_STALE", "error": "章节内容已变化，请重新运行一致性检查"})).into_response();
        }
    }
    // 写正文
    let next = if doc.action == "append" {
        format!("{}\n\n{}", ch.content.trim_end(), content.trim()).trim().to_string()
    } else {
        if doc.source_text.is_empty() || !ch.content.contains(&doc.source_text) {
            return Json(json!({"ok": false, "code": "SOURCE_TEXT_CHANGED", "error": "原选中文本已不存在，请重新生成建议"})).into_response();
        }
        ch.content.replacen(&doc.source_text, content.trim(), 1)
    };
    let mut ch2 = ch;
    ch2.content = next;
    ch2.version_no += 1;
    ch2.updated_at = now();
    let s = serde_json::to_string_pretty(&ch2).map_err(|e| internal("CHAPTER_SERIALIZE", e.to_string()));
    let s = match s {
        Ok(v) => v,
        Err(r) => return r,
    };
    if let Err(e) = state.works.write_text(&session.workspace_id, &ch_path(&pid, &doc.chapter_id), &s) {
        return map_core_err(e);
    }
    doc.status = "accepted".into();
    doc.content = content.clone();
    let s = serde_json::to_string_pretty(&doc).unwrap_or_default();
    let _ = state.works.write_text(&session.workspace_id, &sg_path(&pid, &sg), &s);
    // P1 后验（吞噬 bishu-novel post-hoc OBS→ARB 压缩版，fail-open 不挡采纳）
    let posthoc = run_posthoc(&state, &session.workspace_id, &pid, &doc.chapter_id, ch2.version_no, "", &content).await;
    Json(json!({"ok": true, "suggestion": doc, "chapterVersion": ch2.version_no, "posthoc": posthoc})).into_response()
}

/// 后验审计路径
pub(crate) fn posthoc_path(pid: &str, ch: &str, v: u32) -> String {
    format!("projects/{pid}/posthoc/{ch}-v{v}.json")
}

pub(crate) const POSTHOC_SYS: &str = "你是小说后验裁决器。对比章节旧正文和刚采纳的新增内容，输出 JSON（只输出 JSON）：{\"newForeshadows\":[{\"title\":\"伏笔名\",\"description\":\"描述\"}],\"foreshadowRecall\":[{\"title\":\"疑似回收的旧伏笔\"}],\"debts\":[\"未决叙事债务\"],\"charNotes\":[\"人物状态变化\"]}。没有则给空数组。";

/// 跑后验（fail-open，返回审计摘要；LLM 失败返回 {skipped}）。
pub(crate) async fn run_posthoc(
    state: &AppState,
    ws: &str,
    pid: &str,
    ch: &str,
    ver: u32,
    old_text: &str,
    new_text: &str,
) -> Value {
    let user = format!(
        "旧正文（截断3000字）：\n{}\n\n新增内容：\n{}\n\n只输出 JSON。",
        old_text.chars().take(3000).collect::<String>(),
        new_text.chars().take(2000).collect::<String>()
    );
    let (raw, _, _) = match call_llm_tracked(state, ws, pid, "posthoc", POSTHOC_SYS, &user).await {
        Ok(v) => v,
        Err(e) => return json!({"skipped": true, "reason": e}),
    };
    let s = raw.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s)
        .trim()
        .trim_end_matches("```")
        .trim();
    let start = s.find('{').unwrap_or(0);
    let end = s.rfind('}').map(|i| i + 1).unwrap_or(s.len());
    let v: Value = serde_json::from_str(&s[start..end]).unwrap_or(json!({}));
    // 新伏笔写回（work_id=pid 约定：改编 project 即 foreshadow work）
    let mut planted = 0;
    if let Some(arr) = v.get("newForeshadows").and_then(|x| x.as_array()) {
        for f in arr {
            let title = f.get("title").and_then(|x| x.as_str()).unwrap_or("").trim();
            if title.is_empty() {
                continue;
            }
            let desc = f.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if state.foreshadow.create_foreshadow(pid, title.to_string(), desc, "planted".into(), None).is_ok() {
                planted += 1;
            }
        }
    }
    let audit = json!({
        "chapterId": ch, "version": ver,
        "newForeshadows": v.get("newForeshadows").cloned().unwrap_or(json!([])),
        "foreshadowRecall": v.get("foreshadowRecall").cloned().unwrap_or(json!([])),
        "debts": v.get("debts").cloned().unwrap_or(json!([])),
        "charNotes": v.get("charNotes").cloned().unwrap_or(json!([])),
        "planted": planted, "at": now(),
    });
    let s = serde_json::to_string_pretty(&audit).unwrap_or_default();
    let _ = state.works.mkdir(ws, &format!("projects/{pid}/posthoc"));
    let _ = state.works.write_text(ws, &posthoc_path(pid, ch, ver), &s);
    audit
}

pub(crate) async fn reject_suggest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((pid, sg)): Path<(String, String)>,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (pid, sg) = match (safe_id(&pid), safe_id(&sg)) {
        (Ok(a), Ok(b)) => (a, b),
        _ => return bad_request("SUGGEST_BAD_ID", "invalid id"),
    };
    let mut doc = match read_sg(&state, &session.workspace_id, &pid, &sg) {
        Ok(d) => d,
        Err(r) => return r,
    };
    if doc.status != "pending" {
        return Json(json!({"ok": false, "code": "SUGGESTION_DECIDED", "error": "该建议已经处理"})).into_response();
    }
    doc.status = "rejected".into();
    let s = serde_json::to_string_pretty(&doc).unwrap_or_default();
    let _ = state.works.write_text(&session.workspace_id, &sg_path(&pid, &sg), &s);
    Json(json!({"ok": true, "suggestion": doc})).into_response()
}
