//! suggest/guard —— 续写一致性守卫系统提示 + 七维解析 + suggest_router。（R6④b 机械迁入）
#![allow(unused_imports)]

use super::*;

pub(crate) const GUARD_SYS: &str = "你是续写一致性守卫。按七维逐项检查：①设定一致性 consistency（事件顺序/世界规则/角色属性/状态记录）；②人设一致性 character（行为/对话/动机）；③节奏平衡 pacing（连续同类型/主线停滞/单章情感质变/情节越界）；④叙事连贯 continuity（场景过渡/因果/信息一致）；⑤伏笔健康 foreshadow（长期未推进/新伏笔回收方向）；⑥钩子质量 hook（章末吸引力/类型重复/主线一致）；⑦审美品质 aesthetic（AI 味/叙事手法/情感打动力，每子项必须引用原文举证）。输出分两块：第一块是 JSON 数组（issues），每项必须带 dimension（七维之一）+ type（character/location/time/world/outline/foreshadow）双打标；description 必须引用候选正文原文 10 字以上举证；aesthetic 维 candidateQuote 不得为空。第二块是 JSON 对象（七维评分），格式 {\"dimensions\":{\"consistency\":{\"score\":0-100,\"comment\":\"…\"},…七维全列}}。两块都必须输出，不得省略第二块。不得把文风偏好当成事实冲突。";

/// 七维映射：type → dimension 缺省（LLM 没打 dimension 时回退）。
pub(crate) fn dim_for_kind(kind: &str) -> &'static str {
    match kind {
        "character" => "character",
        "location" | "time" | "world" => "consistency",
        "outline" => "continuity",
        "foreshadow" => "foreshadow",
        _ => "continuity",
    }
}

pub(crate) fn parse_guard(content: &str) -> Result<(Vec<GuardIssue>, Vec<DimensionScore>), String> {
    // 提取 JSON（兼容 markdown 代码块包裹；数组 issues + 可选尾部 dimensions 对象）
    let s = content.trim();
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s)
        .strip_suffix("```")
        .unwrap_or(s)
        .trim();
    let start = s.find('[').ok_or("guard result must be array")?;
    let end = s.rfind(']').ok_or("guard result must be array")?;
    let arr: Vec<Value> =
        serde_json::from_str(&s[start..=end]).map_err(|e| format!("guard invalid json: {e}"))?;
    // 尾部 dimensions 对象（数组之后第一个 {...} 且含 dimension/score）
    let mut dims: Vec<DimensionScore> = Vec::new();
    let tail = &s[end + 1..];
    if let (Some(ds), Some(de)) = (tail.find('{'), tail.rfind('}')) {
        if let Ok(v) = serde_json::from_str::<Value>(&tail[ds..=de]) {
            let obj = if v.get("dimensions").is_some() { v.get("dimensions").unwrap() } else { &v };
            if let Some(map) = obj.as_object() {
                for (k, val) in map {
                    let score = val.get("score").and_then(|x| x.as_i64()).unwrap_or_else(|| val.as_i64().unwrap_or(0)) as i32;
                    dims.push(DimensionScore {
                        dimension: k.clone(),
                        score: score.clamp(0, 100),
                        verdict: if score >= 80 { "pass".into() } else if score >= 60 { "warning".into() } else { "fail".into() },
                        comment: val.get("comment").and_then(|x| x.as_str()).unwrap_or("").into(),
                    });
                }
            }
        }
    }
    let kinds = ["character", "location", "time", "world", "outline", "foreshadow"];
    let sevs = ["low", "medium", "high"];
    let dims7 = ["consistency", "character", "pacing", "continuity", "foreshadow", "hook", "aesthetic"];
    let mut out = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        let kind = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if !kinds.contains(&kind) {
            return Err(format!("guard item {} invalid type", i + 1));
        }
        let sev = item.get("severity").and_then(|v| v.as_str()).unwrap_or("");
        if !sevs.contains(&sev) {
            return Err(format!("guard item {} invalid severity", i + 1));
        }
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
        if title.is_empty() {
            return Err(format!("guard item {} missing title", i + 1));
        }
        // ④ aesthetic 维 candidateQuote 不得为空
        let dim_raw = item.get("dimension").and_then(|v| v.as_str()).unwrap_or("");
        let dim = if dims7.contains(&dim_raw) { dim_raw.to_string() } else { dim_for_kind(kind).to_string() };
        let quote = item.get("candidateQuote").and_then(|v| v.as_str()).unwrap_or("");
        if dim == "aesthetic" && quote.trim().is_empty() {
            return Err(format!("guard item {} aesthetic missing candidateQuote", i + 1));
        }
        out.push(GuardIssue {
            kind: kind.into(),
            dimension: dim,
            severity: sev.into(),
            title: title.into(),
            description: item.get("description").and_then(|v| v.as_str()).unwrap_or("").into(),
            candidate_quote: quote.into(),
            source_refs: item.get("sourceRefs").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
            suggestion: item.get("suggestion").and_then(|v| v.as_str()).unwrap_or("").into(),
        });
    }
    Ok((out, dims))
}

pub fn suggest_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/author/projects/{id}/chapters/{ch}/suggest",
            post(create_suggest),
        )
        .route(
            "/api/v1/author/projects/{id}/suggestions",
            get(list_suggests),
        )
        .route(
            "/api/v1/author/projects/{id}/stats",
            get(get_stats),
        )
        .route(
            "/api/v1/author/projects/{id}/triage",
            post(triage_instruction),
        )
        .route(
            "/api/v1/author/projects/{id}/writing-rules",
            get(get_writing_rules).put(put_writing_rules),
        )
        .route(
            "/api/v1/author/projects/{id}/advance",
            get(get_advance).post(set_advance),
        )
        .route(
            "/api/v1/author/projects/{id}/permit",
            post(grant_permit),
        )
        .route(
            "/api/v1/author/projects/{id}/arcs",
            get(get_arcs),
        )
        .route(
            "/api/v1/author/projects/{id}/ledger",
            get(get_ledger),
        )
        .route(
            "/api/v1/author/projects/{id}/suggestions/{sg}",
            get(get_suggest),
        )
        .route(
            "/api/v1/author/projects/{id}/suggestions/{sg}/guard",
            post(run_guard),
        )
        .route(
            "/api/v1/author/projects/{id}/suggestions/{sg}/accept",
            post(accept_suggest),
        )
        .route(
            "/api/v1/author/projects/{id}/suggestions/{sg}/reject",
            post(reject_suggest),
        )
}
