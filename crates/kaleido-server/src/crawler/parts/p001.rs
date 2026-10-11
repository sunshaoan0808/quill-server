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

pub(crate) fn proxy_url_default() -> String {
    std::env::var("CRAWLER_PROXY_URL").unwrap_or_default()
}
/// Number of chapters to fetch before creating a fresh client (forces new IP).
pub(crate) const PROXY_ROTATION_INTERVAL: usize = 20;

/// Proxy configuration for the crawler.
pub struct ProxyConfig {
    pub proxy_url: String,
    pub rotation_interval: usize,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            proxy_url: proxy_url_default(),
            rotation_interval: PROXY_ROTATION_INTERVAL,
        }
    }
}

use kaleido_core::{is_terminal_job_status, normalize_job_status, JobEvent, JobListFilter};
use crate::{map_core_err, session_from, AppState};

pub(crate) const FIRST_CHAPTER_COUNT: usize = 999;

// ─── Background crawl progress tracker ───────────────────────────────────────

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Progress entry for a background crawl task.
#[derive(Debug, Clone, Serialize)]
pub struct CrawlProgress {
    pub crawl_id: String,
    pub title: String,
    pub total: usize,
    pub fetched: usize,
    pub skipped: usize,
    pub current_chapter: String,
    pub status: String, // "running" | "done" | "error"
    pub output_path: String,
}

/// Global progress store: crawl_id -> progress
pub(crate) static PROGRESS: std::sync::LazyLock<Arc<RwLock<HashMap<String, CrawlProgress>>>> = std::sync::LazyLock::new(|| Arc::new(RwLock::new(HashMap::new())));

pub(crate) fn progress_store() -> Arc<RwLock<HashMap<String, CrawlProgress>>> {
    PROGRESS.clone()
}

/// Generate a crawl ID from title + timestamp.
pub(crate) fn make_crawl_id(title: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{}_{}", sanitize_filename(title), ts)
}

/// Update progress in the global store.
async fn update_progress(prog: CrawlProgress) {
    let store = progress_store();
    store.write().await.insert(prog.crawl_id.clone(), prog);
}

/// Body for `POST /api/v1/crawler/fanqie`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanqieCrawlBody {
    /// Article / page URL (preferred when present).
    #[serde(default)]
    pub url: Option<String>,
    /// Fanqie article / chapter id (alternative to url).
    #[serde(default, alias = "articleId")]
    pub article_id: Option<String>,
    /// `番茄小说-短篇` | `番茄小说-长篇` (default short if reader/id only).
    #[serde(default)]
    pub novel_type: Option<String>,
    /// When true, write markdown under works jail `crawler/`.
    #[serde(default)]
    pub save: Option<bool>,
    /// When true and real fetch fails, return mock payload (gate convenience).
    #[serde(default)]
    pub mock_on_failure: Option<bool>,
}

/// `POST /api/v1/crawler/fanqie`
///
/// - Missing/invalid bearer → 401 via session_from.
/// - `crawlerEnabled != true` → **403** `{ "error": "crawler_disabled" }`.
/// - Enabled → real fetch when URL/id looks like Fanqie; else structured error.
pub async fn fanqie(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let session = match session_from(&state, &headers) {
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
            Json(json!({
                "ok": false,
                "error": "crawler_disabled",
                "code": "CRAWLER_DISABLED",
                "stage": "gate",
                "retryable": false,
                "crawlerEnabled": false,
                "defaultOff": true,
                "hint": crawler_hint("CRAWLER_DISABLED"),
            })),
        )
            .into_response();
    }

    let req: FanqieCrawlBody = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            return bad_request("CRAWL_INVALID", format!("Invalid JSON: {e}"));
        }
    };

    let mut url = req
        .url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let article_id = req
        .article_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    if url.is_none() && article_id.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "url or article_id required",
                "code": "CRAWLER_BAD_REQUEST",
                "stage": "request",
                "retryable": false,
                "hint": "Provide url or articleId",
            })),
        )
            .into_response();
    }

    // Normalize bare id → book page URL (user provides book_id, not chapter_id).
    if url.is_none() {
        if let Some(id) = article_id.as_ref() {
            url = Some(format!("https://fanqienovel.com/page/{id}"));
        }
    }

    // 不变量：162 行双缺早退 + 178 行 article_id 兜底填充后 url 必为 Some
    let url = url.expect("url filled from articleId or early-returned above");
    let novel_type = req
        .novel_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if url.contains("/page/") {
                "番茄小说-长篇"
            } else {
                "番茄小说-短篇"
            }
        })
        .to_string();
    let do_save = req.save.unwrap_or(false);
    let mock_on_failure = req.mock_on_failure.unwrap_or(false);

    match crawl_real(&url, &novel_type, &Some(ProxyConfig::default()), do_save).await {
        Ok(result) => {
            let mut saved_path: Option<String> = None;
            if do_save {
                let rel = format!(
                    "crawler/{}.md",
                    sanitize_filename(&result.title)
                );
                let _ = state.works.mkdir(&session.workspace_id, "crawler");
                let md = format!(
                    "# {}\n\n**原文链接**: {}\n\n---\n\n{}",
                    result.title, result.source_url, result.content
                );
                match state
                    .works
                    .write_text(&session.workspace_id, &rel, &md)
                {
                    Ok(f) => saved_path = Some(f.path),
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({
                                "ok": false,
                                "error": format!("save failed: {e}"),
                                "code": "CRAWLER_SAVE",
                                "stage": "save",
                                "retryable": true,
                                "title": result.title,
                                "content": result.content,
                                "source": result.source,
                                "hint": crawler_hint("CRAWLER_SAVE"),
                            })),
                        )
                            .into_response();
                    }
                }
            }
            Json(json!({
                "ok": true,
                "title": result.title,
                "content": result.content,
                "source": result.source,
                "url": result.source_url,
                "articleId": result.article_id,
                "chaptersFetched": result.chapters_fetched,
                "savedPath": saved_path,
                "note": result.note,
                "crawlerEnabled": true,
                "meta": result.meta,
            }))
            .into_response()
        }
        Err(err) => {
            let fail = classify_crawl_error(&err, &url);
            if mock_on_failure {
                let title = article_id
                    .as_ref()
                    .map(|id| format!("Mock Fanqie Article {id}"))
                    .unwrap_or_else(|| format!("Mock crawl of {url}"));
                let content = format!(
                    "Mock fallback after real fetch error.\nurl={url}\narticle_id={}\nerror={err}\ncode={}",
                    article_id.as_deref().unwrap_or(""),
                    fail.code
                );
                return Json(json!({
                    "ok": true,
                    "title": title,
                    "content": content,
                    "source": "mock",
                    "url": url,
                    "articleId": article_id,
                    "warning": err,
                    "code": fail.code,
                    "stage": fail.stage,
                    "retryable": fail.retryable,
                    "mockOnFailure": true,
                }))
                .into_response();
            }
            let status = match fail.code {
                "CRAWLER_UNSUPPORTED_HOST" | "CRAWLER_BAD_URL" => StatusCode::BAD_REQUEST,
                "CRAWLER_ANTIBOT" => StatusCode::FORBIDDEN,
                _ => StatusCode::BAD_GATEWAY,
            };
            (status, Json(fail.to_json(&url, &article_id))).into_response()
        }
    }
}

/// Novel metadata extracted from __INITIAL_STATE__.page
#[derive(Debug, Clone, Serialize, Default)]
pub struct NovelMeta {
    pub author: String,
    pub author_id: String,
    pub book_id: String,
    pub abstract_text: String,
    pub word_number: u64,
    pub category: String,
    pub creation_status: i32,
    pub read_count: u64,
    pub cover_url: String,
    pub last_chapter_title: String,
    pub chapter_total: u32,
}

/// Extract novel metadata from page HTML's __INITIAL_STATE__.page
pub(crate) fn extract_novel_meta(html: &str) -> NovelMeta {
    let mut meta = NovelMeta::default();
    // Reuse extract_from_initial_state infrastructure to get the JSON
    let start_marker = "window.__INITIAL_STATE__ =";
    let start_marker2 = "window.__INITIAL_STATE__=";
    let idx = match html.find(start_marker).or_else(|| html.find(start_marker2)) {
        Some(i) => i,
        None => return meta,
    };
    let brace_rel = match html[idx..].find('{') {
        Some(b) => b,
        None => return meta,
    };
    let json_str = &html[idx + brace_rel..];
    let cleaned = replace_undefined(json_str);
    let mut stream = serde_json::Deserializer::from_str(&cleaned).into_iter::<Value>();
    if let Some(Ok(data)) = stream.next() {
        let page = &data["page"];
        meta.author = page["author"].as_str().unwrap_or("").to_string();
        meta.author_id = page["authorId"].as_str().unwrap_or("").to_string();
        meta.book_id = page["bookId"].as_str().unwrap_or("").to_string();
        meta.abstract_text = page["abstract"].as_str().unwrap_or("").to_string();
        meta.word_number = page["wordNumber"].as_u64().unwrap_or(0);
        meta.creation_status = page["creationStatus"].as_i64().unwrap_or(-1) as i32;
        meta.read_count = page["readCount"].as_u64().unwrap_or(0);
        meta.cover_url = page["thumbUri"].as_str().unwrap_or("").to_string();
        meta.last_chapter_title = page["lastChapterTitle"].as_str().unwrap_or("").to_string();
        meta.chapter_total = page["chapterTotal"].as_u64().unwrap_or(0) as u32;
        // category: try categoryV2 array first, fallback to category string
        if let Some(cats) = page["categoryV2"].as_array() {
            let names: Vec<&str> = cats.iter()
                .filter_map(|c| c["Name"].as_str())
                .collect();
            if !names.is_empty() {
                meta.category = names.join(", ");
            }
        }
        if meta.category.is_empty() {
            meta.category = page["category"].as_str().unwrap_or("").to_string();
        }
    }
    meta
}

pub(crate) struct CrawlResult {
    title: String,
    content: String,
    source: String,
    source_url: String,
    article_id: Option<String>,
    chapters_fetched: usize,
    note: String,
    meta: Option<NovelMeta>,
}

/// W19: machine-readable crawl failure (HTTP body extras).
#[derive(Debug, Clone)]
pub(crate) struct CrawlFailure {
    code: &'static str,
    message: String,
    stage: &'static str,
    retryable: bool,
    http_status: Option<u16>,
    host: Option<String>,
}

impl CrawlFailure {
    fn new(code: &'static str, stage: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            stage,
            retryable,
            http_status: None,
            host: None,
        }
    }

    fn with_host(mut self, host: impl Into<String>) -> Self {
        self.host = Some(host.into());
        self
    }

    fn with_http(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    fn to_json(&self, url: &str, article_id: &Option<String>) -> Value {
        let mut v = json!({
            "ok": false,
            "error": self.message,
            "code": self.code,
            "stage": self.stage,
            "retryable": self.retryable,
            "url": url,
            "articleId": article_id,
            "source": "live",
            "crawlerEnabled": true,
            "hint": crawler_hint(self.code),
        });
        if let Some(h) = &self.host {
            v.as_object_mut().unwrap().insert("host".into(), json!(h));
        }
        if let Some(s) = self.http_status {
            v.as_object_mut().unwrap().insert("httpStatus".into(), json!(s));
        }
        v
    }
}

pub(crate) fn crawler_hint(code: &str) -> &'static str {
    match code {
        "CRAWLER_DISABLED" => "PATCH /api/v1/settings {crawlerEnabled:true} then retry; default remains off",
        "CRAWLER_UNSUPPORTED_HOST" => "Only fanqienovel.com / fqnovel.com / fanqie.* reader|page URLs",
        "CRAWLER_BAD_URL" => "Need /reader/{id} or /page/{id} on supported host",
        "CRAWLER_ANTIBOT" => "Anti-bot / captcha page; rotate proxy, cool down, or try later",
        "CRAWLER_HTTP" => "Upstream non-2xx; check proxy and target availability",
        "CRAWLER_NETWORK" => "Transport/proxy failure; verify resin/proxy and network",
        "CRAWLER_EMPTY" => "Parsed zero chapters or all chapter fetches failed",
        "CRAWLER_PARSE" => "HTML parse / decrypt failed",
        "CRAWLER_SAVE" => "Works jail write failed — mkdir crawler/ and check WORKS_* limits",
        _ => "See docs/W19_CRAWLER.md",
    }
}

/// Map legacy free-text crawl errors → stable codes (W19).
pub(crate) fn classify_crawl_error(err: &str, url: &str) -> CrawlFailure {
    let e = err.to_ascii_lowercase();
    let host = host_of(url);
    if e.contains("unsupported host") || e.contains("expected fanqienovel") {
        return CrawlFailure::new(
            "CRAWLER_UNSUPPORTED_HOST",
            "ssrf_guard",
            err,
            false,
        )
        .with_host(host);
    }
    if e.contains("url格式") || e.contains("无法提取小说") || e.contains("reader/") && e.contains("page/") {
        return CrawlFailure::new("CRAWLER_BAD_URL", "url_parse", err, false).with_host(host);
    }
    if e.contains("反爬") || e.contains("验证") || e.contains("滑块") || e.contains("captcha") {
        return CrawlFailure::new("CRAWLER_ANTIBOT", "antibot", err, true).with_host(host);
    }
    if e.contains("状态码") || e.contains("status") {
        let status = err
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<u16>()
            .ok();
        let mut f = CrawlFailure::new("CRAWLER_HTTP", "http", err, true).with_host(host);
        if let Some(s) = status {
            f = f.with_http(s);
        }
        return f;
    }
    if e.contains("request failed")
        || e.contains("http client")
        || e.contains("bad proxy")
        || e.contains("error sending")
        || e.contains("timed out")
        || e.contains("timeout")
        || e.contains("connection")
    {
        return CrawlFailure::new("CRAWLER_NETWORK", "network", err, true).with_host(host);
    }
    if e.contains("未解析到任何章节")
        || e.contains("全部抓取失败")
        || e.contains("未找到短篇")
    {
        return CrawlFailure::new("CRAWLER_EMPTY", "extract", err, true).with_host(host);
    }
    if e.contains("无法提取章节") || e.contains("decrypt") || e.contains("parse") {
        return CrawlFailure::new("CRAWLER_PARSE", "parse", err, true).with_host(host);
    }
    CrawlFailure::new("CRAWLER_FETCH_FAILED", "fetch", err, true).with_host(host)
}

/// Fanqie allowed hosts (exact match after lowercasing, port-stripped).
pub(crate) const ALLOWED_HOSTS: &[&str] = &[
    "fanqienovel.com",
    "www.fanqienovel.com",
    "fqnovel.com",
    "www.fqnovel.com",
    "fanqie.com",
    "www.fanqie.com",
];

/// User-Agent pool for rotation.
pub(crate) const UA_POOL: &[&str] = &[
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2 Safari/605.1.15",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:122.0) Gecko/20100101 Firefox/122.0",
];

/// Parse URL and return the lowercase host (no port). Returns None on malformed URLs.
pub(crate) fn url_host(url: &str) -> Option<String> {
    let no_proto = url.split("://").nth(1).unwrap_or(url);
    let host = no_proto.split('/').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or(host);
    let host = host.to_ascii_lowercase();
    if host.is_empty() { None } else { Some(host) }
}

/// Strict SSRF check: parsed host must be in the allowlist.
pub(crate) fn is_fanqie_host(url: &str) -> bool {
    match url_host(url) {
        Some(h) => ALLOWED_HOSTS.contains(&h.as_str()),
        None => false,
    }
}

/// Pick a random UA from the pool.
pub(crate) fn random_ua() -> &'static str {
    let idx = (nanos_rand() as usize) % UA_POOL.len();
    UA_POOL[idx]
}

/// Random sleep between min and max millis.
async fn random_sleep(min_ms: u64, max_ms: u64) {
    let range = max_ms.saturating_sub(min_ms) + 1;
    let ms = min_ms + (nanos_rand() % range);
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// Quick pseudo-random from system nanos (no external rand dep needed).
pub(crate) fn nanos_rand() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Build a reqwest Client, optionally routed through the proxy pool.
/// When `proxy_url` is None/empty, a direct client is returned.
/// Each client gets a random UA from the pool.
pub(crate) fn make_crawler_client(proxy_url: Option<&str>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(random_ua())
        .redirect(reqwest::redirect::Policy::limited(5));
    if let Some(url) = proxy_url {
        if !url.is_empty() {
            builder = builder.proxy(
                reqwest::Proxy::all(url)
                    .map_err(|e| format!("bad proxy url: {e}"))?,
            );
        }
    } else {
        // Fallback to system proxy env vars (HTTP_PROXY / http_proxy)。
        // [2026-10-06] 未配置任何代理时显式 no_proxy——reqwest 0.12 的 system-proxy
        // 会读 Windows 注册表代理，桌面代理软件的生死会劫持服务端出站（见 today 事故）。
        let mut env_proxy = false;
        for var in &["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            if let Ok(val) = std::env::var(var) {
                if !val.is_empty() {
                    if let Ok(proxy) = reqwest::Proxy::all(&val) {
                        builder = builder.proxy(proxy);
                        env_proxy = true;
                        break;
                    }
                }
            }
        }
        if !env_proxy {
            builder = builder.no_proxy();
        }
    }
    builder.build().map_err(|e| format!("http client: {e}"))
}

/// Build browser-like headers for a request to fanqienovel.com.
pub(crate) fn browser_headers(ua: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
    let mut h = HeaderMap::new();
    let _ = h.insert(HeaderName::from_static("accept"), HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"));
    let _ = h.insert(HeaderName::from_static("accept-language"), HeaderValue::from_static("zh-CN,zh;q=0.9,en;q=0.8"));
    let _ = h.insert(HeaderName::from_static("accept-encoding"), HeaderValue::from_static("gzip, deflate, br"));
    let _ = h.insert(HeaderName::from_static("cache-control"), HeaderValue::from_static("no-cache"));
    let _ = h.insert(HeaderName::from_static("pragma"), HeaderValue::from_static("no-cache"));
    let _ = h.insert(HeaderName::from_static("sec-ch-ua"), HeaderValue::from_static(r#""Chromium";v="121", "Not A(Brand";v="24""#));
    let _ = h.insert(HeaderName::from_static("sec-ch-ua-mobile"), HeaderValue::from_static("?0"));
    let _ = h.insert(HeaderName::from_static("sec-ch-ua-platform"), HeaderValue::from_static(r#""Windows""#));
    let _ = h.insert(HeaderName::from_static("sec-fetch-dest"), HeaderValue::from_static("document"));
    let _ = h.insert(HeaderName::from_static("sec-fetch-mode"), HeaderValue::from_static("navigate"));
    let _ = h.insert(HeaderName::from_static("sec-fetch-site"), HeaderValue::from_static("none"));
    let _ = h.insert(HeaderName::from_static("sec-fetch-user"), HeaderValue::from_static("?1"));
    let _ = h.insert(HeaderName::from_static("upgrade-insecure-requests"), HeaderValue::from_static("1"));
    if let Ok(v) = HeaderValue::from_str(ua) {
        let _ = h.insert(HeaderName::from_static("user-agent"), v);
    }
    h
}
async fn crawl_real(url: &str, novel_type: &str, proxy_config: &Option<ProxyConfig>, do_save: bool) -> Result<CrawlResult, String> {
    // Strict SSRF guard: parsed host must be in allowlist (not contains()).
    if !is_fanqie_host(url) {
        return Err(format!(
            "unsupported host (expected fanqienovel.com): {}",
            url_host(url).unwrap_or_else(|| url.to_string())
        ));
    }

    let proxy_url = proxy_config.as_ref().map(|c| c.proxy_url.as_str());
    let rotation_interval = proxy_config.as_ref().map_or(0, |c| c.rotation_interval);
    let mut client = make_crawler_client(proxy_url)?;

    // Reader single chapter
    if let Some(chapter_id) = capture_after(url, "/reader/") {
        let (title, content) = fetch_chapter_content(&client, &chapter_id, None).await?;
        let decrypted = decrypt_text(&content);
        let cleaned = clean_html_content(&decrypted);
        let novel_name = if title.is_empty() {
            format!("短篇小说_{chapter_id}")
        } else {
            title
        };
        return Ok(CrawlResult {
            title: novel_name,
            content: cleaned,
            source: "live".into(),
            source_url: url.to_string(),
            article_id: Some(chapter_id),
            chapters_fetched: 1,
            note: "single reader chapter".into(),
            meta: None,
        });
    }

    // Book page
    let book_id = capture_after(url, "/page/").ok_or_else(|| {
        "URL格式不正确，无法提取小说ID或章节ID（需要 /reader/{{id}} 或 /page/{{id}}）".to_string()
    })?;

    let response = client
        .get(url)
        .headers(browser_headers(random_ua()))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("请求失败，状态码: {}", response.status()));
    }
    let html = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {e}"))?;

    check_anti_bot(&html)?;

    // Extract novel metadata from __INITIAL_STATE__.page
    let novel_meta = extract_novel_meta(&html);

    let novel_name = extract_h1_title(&html)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("未命名小说_{book_id}"));

    if novel_type.contains("短") {
        // Fetch ALL reader chapters for short novels, not just the first.
        let all_ids = extract_reader_ids(&html);
        if all_ids.is_empty() {
            return Err("未找到短篇小说阅读链接。".to_string());
        }
        let mut parts: Vec<String> = Vec::new();
        let mut fetched = 0usize;
        for (ci, chapter_id) in all_ids.iter().enumerate() {
            match fetch_chapter_content(&client, chapter_id, Some(&book_id)).await {
                Ok((title, content)) => {
                    let decrypted = decrypt_text(&content);
                    let cleaned = clean_html_content(&decrypted);
                    if cleaned.is_empty() || cleaned.contains("暂无内容") {
                        continue;
                    }
                    let ch_title = if title.is_empty() {
                        format!("第{}章", ci + 1)
                    } else {
                        title
                    };
                    parts.push(format!(
                        "# {}\n\n**章节链接**: https://fanqienovel.com/reader/{}\n\n---\n\n{}",
                        ch_title, chapter_id, cleaned
                    ));
                    fetched += 1;
                }
                Err(_) => continue,
            }
            if ci + 1 < all_ids.len() {
                random_sleep(800, 2000).await;
            }
        }
        if parts.is_empty() {
            return Err("短篇小说所有章节抓取失败。".into());
        }
        let name = novel_name;
        return Ok(CrawlResult {
            title: name,
            content: parts.join("\n\n"),
            source: "live".into(),
            source_url: url.to_string(),
            article_id: Some(book_id),
            chapters_fetched: fetched,
            note: format!("short novel {} chapters", fetched),
            meta: Some(novel_meta),
        });
    }

    // Long novel: catalog + first N chapters
    let mut chapters = extract_chapter_items(&html);
    if chapters.is_empty() {
        for id in extract_reader_ids(&html) {
            if !chapters.iter().any(|(_, cid)| cid == &id) {
                chapters.push((format!("章节 {id}"), id));
            }
        }
    }
    if chapters.is_empty() {
        return Err("未解析到任何章节目录。".into());
    }

    let mut catalog = format!(
        "# 《{}》章节目录\n\n**全部章节**: {} 章\n\n---\n\n",
        novel_name,
        chapters.len()
    );
    for (i, (title, _)) in chapters.iter().enumerate() {
        catalog.push_str(&format!("- 第{}章：{}\n", i + 1, title));
    }

    let target: Vec<_> = chapters.into_iter().take(FIRST_CHAPTER_COUNT).collect();
    // Background crawl for long novels with save:true
    if do_save && target.len() > 50 {
        let bg_title = novel_name.to_string();
        let bg_url = url.to_string();
        let _bg_book_id = book_id.clone();
        let bg_proxy_string = proxy_config.as_ref().map(|c| c.proxy_url.clone());
        let bg_rotation = proxy_config.as_ref().map_or(0, |c| c.rotation_interval);
        let bg_target = target.clone();
        let _bg_total = bg_target.len();
        // Output path: novel_workspace/<title>_<bookId>_<timestamp>.md
        let ws_dir = std::path::PathBuf::from("novel_workspace");
        let _ = std::fs::create_dir_all(&ws_dir);
        let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let bid_short = book_id.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect::<String>();
        let fname = format!("{}_{}_{}.md", sanitize_filename(&bg_title), bid_short, ts);
        let out_path = ws_dir.join(&fname);
        let out_str = out_path.to_string_lossy().to_string();
        let out_str2 = out_str.clone();

        // Register progress tracking
        let crawl_id = make_crawl_id(&bg_title);
        let bg_total = bg_target.len();
        update_progress(CrawlProgress {
            crawl_id: crawl_id.clone(),
            title: bg_title.clone(),
            total: bg_total,
            fetched: 0,
            skipped: 0,
            current_chapter: String::new(),
            status: "running".into(),
            output_path: out_str.clone(),
        }).await;

        // Write initial catalog
        let initial = format!("# {}\n\n**URL**: {}\n\n# 章节目录\n\n{}\n\n---\n\n*正在后台爬取...*\n", bg_title, bg_url, catalog);
        let _ = std::fs::write(&out_path, &initial);

        let bg_book_id = book_id.clone();
        // Spawn background task
        tokio::spawn(async move {
            let bg_proxy: Option<&str> = bg_proxy_string.as_deref();
            let mut bg_client = match make_crawler_client(bg_proxy) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[crawler] bg client: {e}");
                    return;
                }
            };
            // Extract cover from first chapter page (each reader page has og:image)
            let bg_cover_url = if let Some((_, first_cid)) = bg_target.first() {
                let reader_url = format!("https://fanqienovel.com/reader/{}", first_cid);
                match bg_client.get(&reader_url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        match resp.text().await {
                            Ok(ch_html) => extract_cover_url(&ch_html),
                            Err(_) => None,
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

            // Resume / incremental: extract already-fetched chapter IDs from existing file
            let existing_content = std::fs::read_to_string(&out_path).unwrap_or_default();
            let mut already_fetched: std::collections::HashSet<String> = std::collections::HashSet::new();
            for line in existing_content.lines() {
                if line.contains("**链接**: https://fanqienovel.com/reader/") {
                    if let Some(id) = line.split("reader/").nth(1) {
                        let cid: String = id.chars().take_while(|c| c.is_ascii_digit()).collect();
                        if !cid.is_empty() {
                            already_fetched.insert(cid);
                        }
                    }
                }
            }
            if !already_fetched.is_empty() {
                tracing::info!("[crawler] resume: {} chapters already fetched, skipping", already_fetched.len());
            }

            // Filter target to only un-fetched chapters (incremental update)
            let bg_to_fetch: Vec<&(String, String)> = bg_target.iter()
                .filter(|(_, cid)| !already_fetched.contains(cid))
                .collect();
            let bg_fetch_total = bg_to_fetch.len();

            let mut bg_ok = 0usize;
            let mut bg_skip = 0usize;

            for (bi, (btitle, bcid)) in bg_to_fetch.iter().enumerate() {
                // Update progress: current chapter
                update_progress(CrawlProgress {
                    crawl_id: crawl_id.clone(),
                    title: bg_title.clone(),
                    total: bg_fetch_total,
                    fetched: bg_ok,
                    skipped: bg_skip,
                    current_chapter: btitle.clone(),
                    status: "running".into(),
                    output_path: out_str2.clone(),
                }).await;

                // Rotate proxy at interval
                if bg_rotation > 0 && bi > 0 && bi % bg_rotation == 0 {
                    if let Ok(c) = make_crawler_client(bg_proxy) {
                        bg_client = c;
                    }
                    random_sleep(1500, 3000).await;
                }

                match fetch_chapter_content(&bg_client, bcid, Some(&bg_book_id)).await {
                    Ok((parsed_title, html)) => {
                        let ftitle = if parsed_title.is_empty() {
                            btitle.clone()
                        } else {
                            parsed_title
                        };
                        let dec = decrypt_text(&html);
                        let cl = clean_html_content(&dec);
                        if cl.is_empty() || cl.contains("暂无内容") {
                            bg_skip += 1;
                            continue;
                        }
                        let ch = format!(
                            "\n\n# {}\n\n**链接**: https://fanqienovel.com/reader/{}\n\n---\n\n{}\n",
                            ftitle, bcid, cl
                        );
                        // Append to file
                        if let Ok(existing) = std::fs::read_to_string(&out_path) {
                            let _ = std::fs::write(&out_path, format!("{}{}", existing, ch));
                        }
                        bg_ok += 1;
                    }
                    Err(_) => {
                        bg_skip += 1;
                    }
                }

                if bi + 1 < bg_fetch_total {
                    // Randomized delay: 800-2000ms, with 10% chance of a longer 3-6s pause
                    let n = nanos_rand();
                    if n % 10 == 0 {
                        random_sleep(3000, 6000).await;
                    } else {
                        random_sleep(800, 2000).await;
                    }
                }
            }

            // Completion footer
            if let Ok(existing) = std::fs::read_to_string(&out_path) {
                let footer = format!("

---

*后台爬取完成: {} 章成功, {} 章跳过*", bg_ok, bg_skip);
                let _ = std::fs::write(&out_path, format!("{}{}", existing, footer));
            }

            // Mark progress as done
            update_progress(CrawlProgress {
                crawl_id: crawl_id.clone(),
                title: bg_title.clone(),
                total: bg_fetch_total,
                fetched: bg_ok,
                skipped: bg_skip,
                current_chapter: String::new(),
                status: "done".into(),
                output_path: out_str2.clone(),
            }).await;

            // Download and save cover image
            if let Some(ref cover_url) = bg_cover_url {
                match bg_client.get(cover_url).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        let ct = resp.headers().get("content-type")
                            .and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                        let bytes = resp.bytes().await.unwrap_or_default();
                        if bytes.len() > 1000 {
                            let _ = std::fs::create_dir_all(&shelf_covers_dir());
                            let slug = shelf_slug(&bg_title);
                            let ext = if ct.contains("png") { "png" }
                                else if ct.contains("webp") { "webp" }
                                else { "jpg" };
                            let cp = shelf_covers_dir().join(format!("{}.{}", slug, ext));
                            let _ = std::fs::write(&cp, &bytes);
                            tracing::info!("[crawler] cover saved: {:?} ({} bytes, ct={})", cp, bytes.len(), ct);
                        }
                    }
                    Ok(resp) => eprintln!("[crawler] cover fetch failed: HTTP {}", resp.status()),
                    Err(e) => eprintln!("[crawler] cover fetch error: {}", e),
                }
            }

            tracing::info!("[crawler] bg crawl done: {} ok, {} skip -> {}", bg_ok, bg_skip, out_str2);
        });
    }

    let mut bodies: Vec<String> = Vec::new();
    let mut success = 0usize;
    for (i, (title, chapter_id)) in target.into_iter().enumerate() {
        // Rotate proxy every N chapters to avoid IP-based rate limiting.
        if rotation_interval > 0 && i > 0 && i % rotation_interval == 0 {
            client = make_crawler_client(proxy_url)?;
            random_sleep(1500, 3000).await;
        }
        match fetch_chapter_content(&client, &chapter_id, Some(&book_id)).await {
            Ok((parsed_title, content)) => {
                let final_title = if parsed_title.is_empty() {
                    title
                } else {
                    parsed_title
                };
                let decrypted = decrypt_text(&content);
                let cleaned = clean_html_content(&decrypted);
                if cleaned.is_empty() || cleaned.contains("暂无内容") {
                    continue;
                }
                bodies.push(format!(
                    "# {}\n\n**章节链接**: https://fanqienovel.com/reader/{}\n\n---\n\n{}\n",
                    final_title, chapter_id, cleaned
                ));
                success += 1;
            }
            Err(_) => {
                // skip individual chapter
            }
        }
        if i + 1 < FIRST_CHAPTER_COUNT {
            // Randomized delay: 800-2000ms, with 10% chance of a longer 3-6s pause
            if nanos_rand() % 10 == 0 {
                random_sleep(3000, 6000).await;
            } else {
                random_sleep(800, 2000).await;
            }
        }
    }

    if success == 0 {
        return Err("章节内容全部抓取失败（可能触发反爬）。".into());
    }

    let content = format!("{catalog}\n\n{}", bodies.join("\n\n"));
    Ok(CrawlResult {
        title: novel_name,
        content,
        source: "live".into(),
        source_url: url.to_string(),
        article_id: Some(book_id.clone()),
        chapters_fetched: success,
        note: format!("long novel first {success} chapters + catalog"),
        meta: Some(novel_meta),
    })
}

/// Max retries for transient failures (429/503/network).
pub(crate) const FETCH_MAX_RETRIES: usize = 2;

async fn fetch_chapter_content(
    client: &reqwest::Client,
    chapter_id: &str,
    book_id: Option<&str>,
) -> Result<(String, String), String> {
    // Try local FQ signing service first (decrypted content, no PUA charset needed)
    let fq_base = "http://127.0.0.1:9999";
    let fq_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .no_proxy()
        .build()
        .map_err(|e| format!("fq client: {e}"))?;

    if let Some(bid) = book_id {
        let ch_url = format!("{}/chapter/{}/{}", fq_base, bid, chapter_id);
        if let Ok(resp) = fq_client.get(&ch_url).send().await {
            if resp.status().is_success() {
                if let Ok(text) = resp.text().await {
                    if let Ok(body) = serde_json::from_str::<Value>(&text) {
                        let content = body["data"]["txtContent"].as_str()
                            .or_else(|| body["data"]["content"].as_str())
                            .unwrap_or("");
                        let title = body["data"]["title"].as_str().unwrap_or("");
                        if !content.is_empty() {
                            return Ok((title.to_string(), content.to_string()));
                        }
                    }
                }
            }
        }
    }

    // Fallback: original page parsing
    let url = format!("https://fanqienovel.com/reader/{chapter_id}");
    let ua = random_ua();
    let headers = browser_headers(ua);

    let mut last_err = String::new();
    for attempt in 0..=FETCH_MAX_RETRIES {
        if attempt > 0 {
            // Exponential backoff: 2s, 4s
            let backoff = 2000u64 * (1u64 << (attempt - 1));
            tokio::time::sleep(std::time::Duration::from_millis(backoff)).await;
        }

        let response = match client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("Req err: {e}");
                continue; // retry on network error
            }
        };

        // Retry on 429 / 503
        let status = response.status();
        if status.as_u16() == 429 || status.as_u16() == 503 {
            last_err = format!("HTTP {} (rate limited)", status);
            continue;
        }
        if !status.is_success() {
            return Err(format!("HTTP {}", status));
        }

        let html = match response.text().await {
            Ok(t) => t,
            Err(e) => {
                last_err = format!("Read err: {e}");
                continue;
            }
        };

        // Anti-bot check - retry once, then fail
        if let Err(e) = check_anti_bot(&html) {
            last_err = e;
            if attempt < FETCH_MAX_RETRIES {
                continue;
            }
            return Err(last_err);
        }

        // Primary: parse __INITIAL_STATE__ via serde
        if let Some((title, content)) = extract_from_initial_state(&html) {
            if !content.is_empty() {
                return Ok((title, content));
            }
        }

        // Fallback 1: manual brace-matching extraction
        if let Some((title, content)) = extract_from_state_fallback(&html) {
            if !content.is_empty() {
                return Ok((title, content));
            }
        }

        // Fallback 2: try the reader/full API endpoint
        let api_url = format!("https://fanqienovel.com/api/reader/full?itemId={chapter_id}");
        if let Ok(api_resp) = client.get(&api_url).headers(headers.clone()).send().await {
            if api_resp.status().is_success() {
                if let Ok(data) = api_resp.json::<Value>().await {
                    let content = data["data"]["chapterData"]["content"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let title = data["data"]["chapterData"]["title"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    if !content.is_empty() {
                        return Ok((title, content));
                    }
                }
            }
        }

        last_err = "无法提取章节内容".to_string();
    }

    Err(last_err)
}
pub(crate) fn extract_from_state_fallback(html: &str) -> Option<(String, String)> {
    let marker = "window.__INITIAL_STATE__=";
    let idx = html.find(marker)?;
    let rest = &html[idx + marker.len()..];
    let brace_start = rest.find('{')?;
    let mut depth = 0u32;
    let mut brace_end = 0;
    for (i, ch) in rest[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    brace_end = brace_start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if brace_end == 0 {
        return None;
    }
    let json_section = &rest[..brace_end];

    let content_key = r#""content":""#;
    let ck_pos = json_section.find(content_key)?;
    let vstart = ck_pos + content_key.len();
    let bytes = json_section.as_bytes();
    let mut i = vstart;
    let mut raw: Vec<u8> = Vec::new();
    while i < bytes.len() && bytes[i] != b'"' {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'\\' => raw.push(b'\\'),
                b'"' => raw.push(b'"'),
                b'n' => raw.push(b'\n'),
                b'r' => raw.push(b'\r'),
                b't' => raw.push(b'\t'),
                b'u' => {
                    let hex_start = i + 2;
                    if hex_start + 4 <= bytes.len() {
                        let hex = &bytes[hex_start..hex_start + 4];
                        if let Ok(hex_str) = std::str::from_utf8(hex) {
                            if let Ok(code) = u32::from_str_radix(hex_str, 16) {
                                if let Some(c) = char::from_u32(code) {
                                    let mut buf = [0u8; 4];
                                    let encoded = c.encode_utf8(&mut buf);
                                    raw.extend_from_slice(encoded.as_bytes());
                                }
                            }
                        }
                        i += 5;
                    } else {
                        i += 1;
                    }
                    continue;
                }
                _ => {
                    raw.push(bytes[i + 1]);
                }
            }
            i += 2;
        } else {
            raw.push(bytes[i]);
            i += 1;
        }
    }
    if raw.is_empty() {
        return None;
    }
    let val = String::from_utf8(raw).unwrap_or_default();
    if val.is_empty() {
        return None;
    }

    let title_key = r#""title":""#;
    let title = if let Some(ts) = json_section.find(title_key) {
        let tv_start = ts + title_key.len();
        let mut ti = tv_start;
        let mut t_raw: Vec<u8> = Vec::new();
        while ti < bytes.len() && bytes[ti] != b'"' {
            if bytes[ti] == b'\\' && ti + 1 < bytes.len() {
                match bytes[ti + 1] {
                    b'\\' => t_raw.push(b'\\'),
                    b'"' => t_raw.push(b'"'),
                    b'n' => t_raw.push(b'\n'),
                    b'r' => t_raw.push(b'\r'),
                    b't' => t_raw.push(b'\t'),
                    b'u' => {
                        let hex_start = ti + 2;
                        if hex_start + 4 <= bytes.len() {
                            let hex = &bytes[hex_start..hex_start + 4];
                            if let Ok(hex_str) = std::str::from_utf8(hex) {
                                if let Ok(code) = u32::from_str_radix(hex_str, 16) {
                                    if let Some(c) = char::from_u32(code) {
                                        let mut buf = [0u8; 4];
                                        let encoded = c.encode_utf8(&mut buf);
                                        t_raw.extend_from_slice(encoded.as_bytes());
                                    }
                                }
                            }
                            ti += 5;
                        } else {
                            ti += 1;
                        }
                        continue;
                    }
                    _ => {
                        t_raw.push(bytes[ti + 1]);
                    }
                }
                ti += 2;
            } else {
                t_raw.push(bytes[ti]);
                ti += 1;
            }
        }
        String::from_utf8(t_raw).unwrap_or_default()
    } else {
        String::new()
    };

    Some((title, val))
}

