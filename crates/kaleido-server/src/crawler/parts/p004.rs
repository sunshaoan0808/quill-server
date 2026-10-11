#![allow(unused_imports)]
use axum::{
    extract::State,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration as StdDuration;
use kaleido_core::JobStore;
use crate::encoding_sniff::decode_text;
use crate::error_codes::*;
use super::*;
use kaleido_core::{is_terminal_job_status, normalize_job_status, JobEvent, JobListFilter};
use crate::{map_core_err, session_from, AppState};

/// [L0] 蒸馏→graph.sqlite 桥：把解析出实体 id 的关系边写入运行时图谱。
///
/// 语义：
/// - 端点归一：用卡 id → 卡名 映射拿到 canonical 名，经 `resolve_or_create_character`
///   按名幂等解析（graph 里已有同名卡则复用，否则建 ai_suggestion 卡）；
/// - 幽灵端点（无 from_id/to_id）跳过：不建幽灵角色卡；
/// - rel→category：`rel_category::normalize_rel_category` 自由词 → 五类映射；
/// - 幂等 + 跨章演化：每章一个 suggestion_id（`work::from::to::rel::章`），重蒸同章
///   直接返回已有边；不同章同 (from,to,category) 的边由 graph_store 演化合并进
///   `chapters[]`（运行时"关系演化"注入的数据来源）。
pub(crate) fn bridge_relations_to_graph(
    graph: &kaleido_core::graph_store::GraphStore,
    work_id: &str,
    edges: &[serde_json::Value],
    id_to_name: &std::collections::HashMap<String, String>,
) -> Result<(), String> {
    for e in edges {
        let from_id = e.get("from_id").and_then(|x| x.as_str()).unwrap_or("");
        let to_id = e.get("to_id").and_then(|x| x.as_str()).unwrap_or("");
        if from_id.is_empty() || to_id.is_empty() {
            continue; // 幽灵端点：不建卡不落边
        }
        let from_name = id_to_name.get(from_id).cloned().unwrap_or_default();
        let to_name = id_to_name.get(to_id).cloned().unwrap_or_default();
        if from_name.is_empty() || to_name.is_empty() {
            continue;
        }
        let rel = e.get("rel").and_then(|x| x.as_str()).unwrap_or("").to_string();
        if rel.is_empty() {
            continue;
        }
        let note = e.get("note").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let category = kaleido_core::rel_category::normalize_rel_category(&rel);
        let chapters: Vec<String> = e
            .get("chapters")
            .and_then(|x| x.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| c.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if chapters.is_empty() {
            let sid = format!("{work_id}::{from_id}::{to_id}::{rel}");
            graph
                .create_relationship_from_suggestion(
                    work_id,
                    &from_name,
                    &to_name,
                    Some(from_id),
                    Some(to_id),
                    category,
                    &rel,
                    &note,
                    None,
                    &sid,
                )
                .map_err(|err| {
                    format!("create_relationship_from_suggestion({from_name}→{to_name} {rel}): {err}")
                })?;
        } else {
            for ch in &chapters {
                let sid = format!("{work_id}::{from_id}::{to_id}::{rel}::{ch}");
                graph
                    .create_relationship_from_suggestion(
                        work_id,
                        &from_name,
                        &to_name,
                        Some(from_id),
                        Some(to_id),
                        category,
                        &rel,
                        &note,
                        Some(ch.as_str()),
                        &sid,
                    )
                    .map_err(|err| {
                        format!(
                            "create_relationship_from_suggestion({from_name}→{to_name} {rel} @ {ch}): {err}"
                        )
                    })?;
            }
        }
    }
    Ok(())
}

/// POST /api/v1/crawler/novels/{slug}/distil-world — 世界树/节拍/多出口/世界线蒸馏。
///
/// 复用 `novel_distil` 的 shelf 解析逻辑，依次执行：
/// 1) 世界树 → pack.lore_entries；
/// 2) 每章节拍 → nodes[].locked_beats；
/// 3) 多出口 → nodes[].exit（原单链 continue 时替换为多出口，已有出口保留并补齐）；
/// 4) 世界线 → 写 `data/story-packs/<pack>/worldline.json`（旁挂，不改 pack.json 结构）。
pub async fn novel_distil_world(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    let session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };
    // 可选指定角色蒸馏：body {"characters": ["外婆","春儿"]} → 跳过角色谱，只蒸馏名单角色并合并进 pack
    let named_chars: Option<Vec<String>> = if body.is_empty() {
        None
    } else {
        serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("characters").cloned())
            .and_then(|v| match v {
                serde_json::Value::Array(a) => Some(a),
                _ => None,
            })
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
    };
    // [L0] forceRelations: true → 已有 relations.json 也强制重跑关系图谱（重蒸馏+重新落 graph 桥）
    let force_relations: bool = if body.is_empty() {
        false
    } else {
        serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("forceRelations").cloned())
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    // 读 shelf md（复用 novel_distil 的解析逻辑）
    let shelf = scan_shelf();
    let entry = match shelf.into_iter().find(|e| e.slug == slug) {
        Some(e) => e,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"ok": false, "error": "novel not found"})),
            )
                .into_response();
        }
    };
    let dir = shelf_dir();
    let slug_tag = shelf_slug(&entry.title);
    let file_path = find_shelf_md_file(&dir, &slug_tag, &entry.title);
    if !file_path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .is_some()
    {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"ok": false, "error": "file not found"})),
        )
            .into_response();
    }

    // 防重：同一 slug 已有 running/queued 的 shelf_distil_world 任务 → 409
    if let Ok(items) = state.jobs.list(JobListFilter {
        kind: Some("shelf_distil_world".into()),
        status: None,
        user_id: None,
        workspace_id: Some(session.workspace_id.clone()),
        limit: 50,
    }) {
        if let Some(j) = items.iter().find(|j| {
            !is_terminal_job_status(&normalize_job_status(&j.status))
                && j.payload
                    .as_ref()
                    .and_then(|p| p.get("slug"))
                    .and_then(|v| v.as_str())
                    == Some(slug.as_str())
        }) {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "ok": false,
                    "error": "该作品已有转换任务在运行，请稍候",
                    "jobId": j.run_id,
                    "jobsStream": format!("/api/v1/jobs/{}/stream", j.run_id),
                    "status": normalize_job_status(&j.status),
                })),
            )
                .into_response();
        }
    }

    let title = entry.title.clone();

    // 创建后台 job：立即返回 202，蒸馏在后台执行
    let job = match state.jobs.create(
        "shelf_distil_world",
        &session.user_id,
        &session.workspace_id,
        json!({ "slug": slug, "title": title, "characters": named_chars, "force_relations": force_relations }),
        None,
        None,
    ) {
        Ok(j) => j,
        Err(e) => return map_core_err(e),
    };
    let run_id = job.run_id.clone();

    // 后台执行体已抽出为 exec_shelf_distil_world（创建后调度与重启恢复续跑共用）
    let run_id_work = run_id.clone();
    let state_spawn = state.clone();
    let title_spawn = title.clone();
    let resume_meta = job.payload.clone();
    tokio::spawn(async move {
        exec_shelf_distil_world(state_spawn, run_id_work, slug, title_spawn, resume_meta).await;
    });

    (StatusCode::ACCEPTED, Json(json!({
        "ok": true,
        "jobId": run_id,
        "jobsStream": format!("/api/v1/jobs/{}/stream", run_id),
        "status": "queued",
    })))
        .into_response()
}

/// GET /api/v1/crawler/novels/{slug}/export — download shelf markdown as attachment.
pub async fn novel_export(
    _state: State<AppState>,
    _headers: HeaderMap,
    Path(slug): Path<String>,
) -> Response {
    let shelf = scan_shelf();
    let entry = match shelf.into_iter().find(|e| e.slug == slug) {
        Some(e) => e,
        None => {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
    };
    let dir = shelf_dir();
    let slug_tag = shelf_slug(&entry.title);
    let file_path = find_shelf_md_file(&dir, &slug_tag, &entry.title);
    match file_path.and_then(|p| std::fs::read(p).ok()) {
        Some(bytes) => {
            let fname = format!("{}.md", entry.title.replace(['/', '\\', '"'], "_"));
            let mut res = (StatusCode::OK, bytes).into_response();
            res.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/markdown; charset=utf-8"),
            );
            if let Ok(v) = axum::http::HeaderValue::from_str(&format!(
                "attachment; filename=\"{}\"",
                fname.replace('"', "")
            )) {
                res.headers_mut()
                    .insert(axum::http::header::CONTENT_DISPOSITION, v);
            }
            res
        }
        None => (StatusCode::NOT_FOUND, "file not found").into_response(),
    }
}

/// `GET /api/v1/crawler/fanqie/meta?url=...` or `?bookId=...`
/// Returns novel metadata only (no chapter content fetch).
pub async fn fanqie_meta(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let _session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let enabled = state
        .app_state
        .load_settings_public()
        .map(|s| s.crawler_enabled)
        .unwrap_or(false);
    if !enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "ok": false, "error": "crawler_disabled", "code": "CRAWLER_DISABLED" })),
        )
            .into_response();
    }

    let url = params.get("url").cloned().or_else(|| {
        params.get("bookId").map(|id| format!("https://fanqienovel.com/page/{id}"))
    });

    let url = match url {
        Some(u) => u,
        None => return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "url or bookId required" })),
        ).into_response(),
    };

    if !is_fanqie_host(&url) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "unsupported host" })),
        ).into_response();
    }

    let book_id = match capture_after(&url, "/page/") {
        Some(id) => id,
        None => return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "need /page/{id} URL or bookId param" })),
        ).into_response(),
    };

    let proxy_config = ProxyConfig::default();
    let client = match make_crawler_client(Some(&proxy_config.proxy_url)) {
        Ok(c) => c,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("client: {e}") })),
        ).into_response(),
    };

    let resp = match client
        .get(&url)
        .headers(browser_headers(random_ua()))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("fetch: {e}") })),
        ).into_response(),
    };

    if !resp.status().is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("HTTP {}", resp.status()) })),
        ).into_response();
    }

    let html = match resp.text().await {
        Ok(t) => t,
        Err(e) => return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("read: {e}") })),
        ).into_response(),
    };

    if let Err(e) = check_anti_bot(&html) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "ok": false, "error": e, "code": "CRAWLER_ANTIBOT" })),
        ).into_response();
    }

    let meta = extract_novel_meta(&html);
    let title = extract_h1_title(&html).unwrap_or_else(|| format!("未命名_{book_id}"));

    Json(json!({
        "ok": true,
        "bookId": book_id,
        "title": title,
        "meta": meta,
        "url": url,
    }))
    .into_response()
}

/// `GET /api/v1/crawler/fanqie/progress` - list all background crawl progress.
/// `GET /api/v1/crawler/fanqie/progress?crawlId=xxx` - single crawl progress.
pub async fn fanqie_progress(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let _session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let store = progress_store();
    let guard = store.read().await;

    if let Some(crawl_id) = params.get("crawlId") {
        match guard.get(crawl_id) {
            Some(prog) => Json(json!({ "ok": true, "progress": prog })).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({ "ok": false, "error": "crawl_id not found" })),
            )
                .into_response(),
        }
    } else {
        let all: Vec<&CrawlProgress> = guard.values().collect();
        Json(json!({ "ok": true, "progress": all }))
            .into_response()
    }
}

/// `GET /api/v1/crawler/fanqie/search?q=书名`
/// Searches fanqie novels via Sogou site search, returns book_id list with meta.
pub async fn fanqie_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    let _session = match session_from(&state, &headers) {
        Ok(s) => s,
        Err(r) => return r,
    };

    let q = match params.get("q").map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(q) => q.to_string(),
        None => return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "query param 'q' required" })),
        ).into_response(),
    };

    // Use local FQ unidbg signing service for search
    let fq_base = "http://127.0.0.1:9999";
    let search_url = format!("{}/search?key={}&page=1&size=10&tabType=3", fq_base, urlencode(&q));
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(e) => return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("client: {e}") })),
        ).into_response(),
    };

    let resp = match client.get(&search_url).send().await {
        Ok(r) => r,
        Err(e) => return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("fq search: {e}") })),
        ).into_response(),
    };

    let text = match resp.text().await {
        Ok(t) => t,
        Err(e) => return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("fq read: {e}") })),
        ).into_response(),
    };

    let body: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => return (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "ok": false, "error": format!("fq parse: {e}") })),
        ).into_response(),
    };

    let books = body["data"]["books"].as_array().cloned().unwrap_or_default();
    let results: Vec<Value> = books.iter().filter_map(|b| {
        let book_id = b["bookId"].as_str()?;
        let title = b["bookName"].as_str().unwrap_or("");
        let author = b["author"].as_str().unwrap_or("");
        Some(json!({
            "book_id": book_id,
            "title": title,
            "author": author,
            "category": b["category"].as_str().unwrap_or(""),
            "abstract": b["description"].as_str().unwrap_or(""),
            "word_number": b["wordCount"].as_i64().unwrap_or(0),
            "cover_url": b["coverUrl"].as_str().unwrap_or(""),
            "url": format!("https://fanqienovel.com/page/{}", book_id),
        }))
    }).collect();

    Json(json!({
        "ok": true,
        "query": q,
        "results": results,
    })).into_response()
}

/// Simple URL-encode for query parameters.
pub(crate) fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' | b' ' => {
                if *b == b' ' { out.push('+'); } else { out.push(*b as char); }
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn capture_reader_id() {
        assert_eq!(
            capture_after("https://fanqienovel.com/reader/12345", "/reader/").as_deref(),
            Some("12345")
        );
    }

    #[test]
    fn strip_basic_tags() {
        assert_eq!(strip_tags("<p>你好</p>"), "你好");
    }

    // 书架 slug→md 文件解析（find_shelf_md_file）：
    // ① stem 精确相等优先于 contains——「老茶馆.md」stem contains「茶馆」，
    //    旧实现 read_dir 先遇到它就会把《茶馆》错配到《老茶馆》的文件；
    // ② 精确候选标题校验不过时，退到 contains 候选并回读首行 `# 标题` 校验命中。
    #[test]
    fn find_shelf_md_file_prefers_exact_and_verifies_title() {
        let dir = std::env::temp_dir().join(format!("kaleido-shelf-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 前缀重叠：两个 stem 归一化后都 contains「茶馆」
        std::fs::write(dir.join("老茶馆.md"), "# 老茶馆\n\n正文").unwrap();
        std::fs::write(dir.join("茶馆.md"), "# 茶馆\n\n正文").unwrap();
        let slug_tag = shelf_slug("茶馆");
        // read_dir 顺序不定：多次解析都应精确命中《茶馆》自己的文件
        for _ in 0..8 {
            let hit = find_shelf_md_file(&dir, &slug_tag, "茶馆").unwrap();
            assert_eq!(hit.file_stem().and_then(|s| s.to_str()), Some("茶馆"));
        }
        // 标题校验：精确候选首行标题不一致 → 退回 contains 候选并按标题命中
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("茶馆.md"), "# 完全不同的书\n\n正文").unwrap();
        std::fs::write(dir.join("茶馆_2.md"), "# 茶馆\n\n正文").unwrap();
        let hit = find_shelf_md_file(&dir, &slug_tag, "茶馆").unwrap();
        assert_eq!(hit.file_stem().and_then(|s| s.to_str()), Some("茶馆_2"));
        // 标题都校验失败 → None（宁缺勿错，不再返回 contains 首个命中）
        std::fs::write(dir.join("茶馆_2.md"), "# 另一本不相干的书\n\n正文").unwrap();
        assert!(find_shelf_md_file(&dir, &slug_tag, "茶馆").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // A0: 章号连续性检测——正常序列零告警；缺章（1,2,3,6）报 gap；乱序报不递增。
    #[test]
    fn chapter_gaps_detects_missing_and_out_of_order() {
        let ok = vec![
            (0usize, "第一章 相遇".to_string()),
            (10, "第二章 误会".to_string()),
            (20, "第三章 接触".to_string()),
        ];
        assert!(chapter_gaps(&ok).is_empty(), "连续序列不应报 gap: {:?}", chapter_gaps(&ok));

        // 源缺第五章：1,2,3,6 → 缺 4,5
        let missing = vec![
            (0usize, "第一章 相遇".to_string()),
            (10, "第二章 误会".to_string()),
            (20, "第三章 接触".to_string()),
            (30, "第六章 初恋".to_string()),
        ];
        let gaps = chapter_gaps(&missing);
        assert_eq!(gaps.len(), 1, "应只报一个 gap: {gaps:?}");
        assert!(gaps[0].contains("缺"), "gap 应含缺章信息: {}", gaps[0]);

        // 中文数字章号解析：第六 ←> 3？ 反例验证能识别数值（「第六章」=6 > 3 +1）
        let cn = vec![
            (0usize, "第三章 接触".to_string()),
            (10, "第六章 初恋".to_string()),
        ];
        let g2 = chapter_gaps(&cn);
        assert_eq!(g2.len(), 1, "中文数字 gap 应检出: {g2:?}");

        // 非「第N章」标题（序章/番外）跳过不误报
        let mixed = vec![
            (0usize, "序章".to_string()),
            (10, "第一章 相遇".to_string()),
            (20, "第二章 误会".to_string()),
        ];
        assert!(chapter_gaps(&mixed).is_empty(), "序章应被跳过: {:?}", chapter_gaps(&mixed));
    }

    // A0 完整版 (2026-08-19): chXX = max(原著章号, 递增游标)。
    // 覆盖三场景：①连续序列恒不变 ②源缺章→自动跳号对齐原著标题（度蜜月 ch05→ch06）③楔子/续章回退→游标顺延零回归（宿醉）。
    #[test]
    fn chapter_id_seq_aligns_to_canonical_numbers() {
        let t = |s: &str| (s.to_string(), "正文".to_string());

        // ① 连续序列：章号=序号，编号不变
        let seq = vec![t("第一章 相遇"), t("第二章 误会"), t("第三章 接触"), t("第四章 亲近")];
        let ids = chapter_id_seq(&seq);
        assert_eq!(ids, vec!["ch01", "ch02", "ch03", "ch04"]);
        assert!(!ids.iter().any(|x| x.is_empty()));

        // ② 度蜜月实证：源缺第五章，「第六章 初恋」排在第 5 位 → 自动跳号 ch06（ch05 空缺），
        // 后续全部对齐原著标题（ch07/ch08…ch17），下游 worldline/relations/事件卡章节引用随之对齐。
        let honeymoon = vec![
            t("第一章 蜜月的起始"), t("第二章 误会"), t("第三章 接触"), t("第四章 亲近"),
            t("第六章 初恋"), t("第七章 过火"), t("第八章 沉沦"), t("第九章 播种"),
            t("第十章 孕"), t("第十一章 相亲"), t("第十二章 约会"), t("第十三章 恋"),
            t("第十四章 生女"), t("第十五章 狂乱"), t("第十六章 日常"), t("第十七章 又一次蜜月"),
        ];
        let ids = chapter_id_seq(&honeymoon);
        let expect: Vec<String> = (1..=17)
            .map(|n| if n == 5 { String::new() } else { format!("ch{n:02}") })
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(ids, expect, "度蜜月应 ch01..ch04, ch06..ch17（缺 ch05）: {ids:?}");
        // 长度不变（16 切片 → 16 个 id），只是跳过 5
        assert_eq!(ids.len(), 16);

        // ③ 宿醉实证：楔子（解析失败）在第 1 位 → ch01；「第一章 续一」章号=1 但游标已到 2 → 顺延 ch02。
        // 结果与旧切分序号完全一致（零回归），且绝不产出重复 chXX。
        let hangover = vec![
            t("楔子"), t("第一章 续一"), t("第二章 续二"), t("第三章 续三"),
            t("第四章 续四"), t("第5章 续5"),
        ];
        let ids = chapter_id_seq(&hangover);
        assert_eq!(ids, vec!["ch01", "ch02", "ch03", "ch04", "ch05", "ch06"], "宿醉应保持原编号: {ids:?}");

        // ④ 章号重复去重：连续两个「第三章」→ ch03, ch04（游标顺延）
        let dup = vec![t("第三章 接触"), t("第三章 再见")];
        let ids = chapter_id_seq(&dup);
        assert_eq!(ids, vec!["ch03", "ch04"], "重复章号应顺延去重: {ids:?}");

        // ⑤ 大跳号：1 → 10 → 编号 ch01, ch10（不补中间空号，ch02..ch09 空缺可见）
        let jump = vec![t("第一章 相遇"), t("第十章 重逢")];
        let ids = chapter_id_seq(&jump);
        assert_eq!(ids, vec!["ch01", "ch10"], "大跳号应直接落到原著章号: {ids:?}");
    }

    // 2026-08-19: chapter_value 支持括号前导序号（度蜜月 `（5）释放`）
    #[test]
    fn chapter_value_parses_bracket_leading() {
        assert_eq!(chapter_value("　　（5）释放（手交口交）"), Some(5), "full: {:?}", chapter_value("　　（5）释放（手交口交）"));
        assert_eq!(chapter_value("（5）释放"), Some(5), "short: {:?}", chapter_value("（5）释放"));
        assert_eq!(chapter_value("　　（5）"), Some(5), "bare: {:?}", chapter_value("　　（5）"));
        assert_eq!(chapter_value("（5）章 内容"), Some(5), "zh: {:?}", chapter_value("（5）章 内容"));
        assert_eq!(chapter_value("第一章 相遇"), Some(1));
    }

    // 2026-08-19: TOC 规则补缺——Markdown 列表破折号前缀 `- 第X章`（安全屋/老板你也没说等 txt 导出格式）
    #[test]
    fn split_novel_chapters_handles_dash_prefix() {
        // 正文填充 >1000 字符（触发 pick_toc_rule 的间隔过滤，防止宽松规则把正文行当标题）
        let long_body = "正文内容……".repeat(120);
        let text = format!(
            "- 第1章 猎人与猎物\n{}\n- 第2章 与世界再见\n{}\n- 第3章 新世界\n{}\n",
            long_body, long_body, long_body
        );
        let chs = split_novel_chapters(&text);
        assert_eq!(chs.len(), 3, "破折号前缀应切出 3 章，实际: {chs:?}");
        assert!(chs[0].0.contains("第1章"), "第1章标题应保留: {}", chs[0].0);
        assert!(chs[2].0.contains("第3章"), "第3章标题应保留: {}", chs[2].0);
    }

    // 2026-08-19: 混合格式——主流「第X章」+ 内嵌「（N）标题」（度蜜月 第五章 `　　（5）释放`）
    // 期望：6 个标题全被识别（4 个第X章 + （5） + 第六章），不留并章。
    #[test]
    fn split_novel_chapters_mixed_bracket_keeps_all_titles() {
        let long_body = "正文内容……".repeat(120);
        let text = format!(
            "第一章 蜜月的起始\n{}\n第二章 误会\n{}\n第三章 接触\n{}\n第四章 亲近\n{}\n　　（5）释放（手交口交）\n{}\n第六章 初恋\n{}\n",
            long_body, long_body, long_body, long_body, long_body, long_body
        );
        let chs = split_novel_chapters(&text);
        assert_eq!(chs.len(), 6, "混合格式应切出 6 章（4 第X章 + （5）+ 第六章），实际: {chs:?}");
        let titles: Vec<String> = chs.iter().map(|(t, _)| t.clone()).collect();
        assert!(titles.iter().any(|t| t.contains("5）释放") || t.contains("（5）释放")),
            "（5）释放 应作为独立标题保留: {titles:?}");
    }

    #[test]
    fn remap_node_present_to_distil_maps_by_name() {
        use kaleido_core::{NodeExit, PackCharacterRef, StoryNode};
        let characters = vec![
            PackCharacterRef {
                id: "c-distil-0".into(),
                name: "林逸".into(),
                role: "protagonist".into(),
                gender: String::new(),
                appearance: String::new(),
                opening_scene: String::new(),
                opening_lines: "我叫林逸，是个天才。".into(),
                nsfw_profile: String::new(),
                importance: "high".into(),
                content_tier: None,
                example_dialogs: vec![],
                boundaries: vec![],
                personality: String::new(),
                speech_style: String::new(),
                voice_profile: String::new(),
                motivation: String::new(),
                relationships: vec![],
                evidence_refs: vec![],
                mental_models: vec![],
                decision_heuristics: vec![],
                beliefs: vec![],
                expressions: std::collections::HashMap::new(),
                avatar: None,
                starting_wardrobe: Default::default(),
                voice: None,
                archive: None,
            },
            PackCharacterRef {
                id: "c-distil-1".into(),
                name: "陆清韵".into(),
                role: "supporting".into(),
                gender: String::new(),
                appearance: String::new(),
                opening_scene: String::new(),
                opening_lines: String::new(),
                nsfw_profile: String::new(),
                importance: "high".into(),
                content_tier: None,
                example_dialogs: vec![],
                boundaries: vec![],
                personality: String::new(),
                speech_style: String::new(),
                voice_profile: String::new(),
                motivation: String::new(),
                relationships: vec![],
                evidence_refs: vec![],
                mental_models: vec![],
                decision_heuristics: vec![],
                beliefs: vec![],
                expressions: std::collections::HashMap::new(),
                avatar: None,
                starting_wardrobe: Default::default(),
                voice: None,
                archive: None,
            },
        ];
        let mut nodes = vec![StoryNode {
            id: "n1".into(),
            chapter_id: "ch01".into(),
            title: "第一章".into(),
            entry: "本章开始".into(),
            exit: vec![NodeExit {
                id: String::new(),
                when: String::new(),
                next: String::new(),
            }],
            locked_beats: vec![],
            allowed_divergence: "branch".into(),
            present_characters: vec!["c-cast-0".into(), "c-cast-5".into()],
            location_id: None,
            summary: String::new(),
        }];
        // bodies 与 nodes 顺序对应:i0 章正文含林逸与陆清韵 → present 应重建 c-distil-0/1
        let bodies = vec![(
            "chapters/ch01.md".to_string(),
            "林逸推开门，说：今天天气不错。陆清韵在屋里淡淡回应。".to_string(),
        )];
        remap_node_present_to_distil(&mut nodes, &characters, &bodies);
        assert_eq!(
            nodes[0].present_characters,
            vec!["c-distil-0", "c-distil-1"]
        );
    }

    #[test]
    fn remap_node_present_clears_when_no_distil_name_in_body() {
        use kaleido_core::{NodeExit, PackCharacterRef, StoryNode};
        let characters = vec![
            PackCharacterRef {
                id: "c-distil-0".into(),
                name: "林逸".into(),
                role: "protagonist".into(),
                gender: String::new(),
                appearance: String::new(),
                opening_scene: String::new(),
                opening_lines: String::new(),
                nsfw_profile: String::new(),
                importance: "high".into(),
                content_tier: None,
                example_dialogs: vec![],
                boundaries: vec![],
                personality: String::new(),
                speech_style: String::new(),
                voice_profile: String::new(),
                motivation: String::new(),
                relationships: vec![],
                evidence_refs: vec![],
                mental_models: vec![],
                decision_heuristics: vec![],
                beliefs: vec![],
                expressions: std::collections::HashMap::new(),
                avatar: None,
                starting_wardrobe: Default::default(),
                voice: None,
                archive: None,
            },
        ];
        // 旁白/抒情章：正文只用「她/妈妈」代词，无任何蒸馏角色直名
        let mut nodes = vec![StoryNode {
            id: "n2".into(),
            chapter_id: "ch02".into(),
            title: "第二章".into(),
            entry: "本章开始".into(),
            exit: vec![NodeExit {
                id: String::new(),
                when: String::new(),
                next: String::new(),
            }],
            locked_beats: vec![],
            allowed_divergence: "branch".into(),
            present_characters: vec!["c-cast-0".into(), "c-cast-3".into()],
            location_id: None,
            summary: String::new(),
        }];
        let bodies = vec![(
            "chapters/ch02.md".to_string(),
            "她望着窗外，妈妈没有说话。夜色渐渐深了，她轻轻叹了口气。".to_string(),
        )];
        remap_node_present_to_distil(&mut nodes, &characters, &bodies);
        // 无直名 → present 清空，不保留 c-cast 悬空引用
        assert!(nodes[0].present_characters.is_empty());
    }
}