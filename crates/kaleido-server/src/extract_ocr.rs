//! Document extraction endpoint + OCR fallback (PR6).
//!
//! Ports DEEIX `application/extraction/service.go` and its RapidOCR HTTP
//! client onto axum: `/api/v1/kaleido-tools/extract` probes the upload,
//! runs the builtin engine, and — when the PDF has thin or garbled pages —
//! asks the configured OCR service for just those page ranges and merges the
//! result. Deterministic decisions (which pages, which error code) live in
//! `kaleido_core::extraction::orchestrate`; this module owns the I/O.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use kaleido_core::extraction::ocr::{normalize_ocr_text, OcrError, OcrResponse};
use kaleido_core::extraction::orchestrate::{
    decide, encode_page_ranges, index_ocr_pages, join_pages, Decision, DecisionInput, PageRange,
};
use kaleido_core::extraction::FileCategory;
use kaleido_core::extraction::PageText;
use serde::Deserialize;
use serde_json::json;

/// Wire page entry — any of the historical field spellings.
#[derive(Debug, Default, Deserialize)]
struct LoosePage {
    page: Option<usize>,
    page_number: Option<usize>,
    index: Option<usize>,
    text: Option<String>,
    content: Option<String>,
    markdown: Option<String>,
}

impl LoosePage {
    fn number(&self) -> usize {
        self.page.or(self.page_number).or(self.index).unwrap_or(0)
    }

    fn text(&self) -> &str {
        [self.text.as_deref(), self.content.as_deref(), self.markdown.as_deref()]
            .into_iter()
            .flatten()
            .find(|t| !t.trim().is_empty())
            .unwrap_or("")
    }
}

/// `pages` arrives either as a bare count or as a list — same union DEEIX
/// unmarshals in `traditionalOCRPagesValue`.
#[derive(Debug, Default)]
struct LoosePages {
    count: usize,
    items: Vec<LoosePage>,
}

impl<'de> Deserialize<'de> for LoosePages {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        Ok(match value {
            serde_json::Value::Number(n) => LoosePages {
                count: n.as_u64().unwrap_or(0) as usize,
                items: Vec::new(),
            },
            serde_json::Value::Array(arr) => {
                let items: Vec<LoosePage> = arr
                    .into_iter()
                    .filter_map(|i| serde_json::from_value(i).ok())
                    .collect();
                LoosePages { count: items.len(), items }
            }
            _ => LoosePages::default(),
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct LooseNode {
    text: Option<String>,
    full_text: Option<String>,
    content: Option<String>,
    markdown: Option<String>,
}

impl LooseNode {
    fn first_text(&self) -> Option<String> {
        [self.text.clone(), self.full_text.clone(), self.content.clone(), self.markdown.clone()]
            .into_iter()
            .flatten()
            .find(|t| !t.trim().is_empty())
    }
}

/// Loose OCR response payload (DEEIX `traditionalOCRPayload`): several
/// top-level and nested spellings compete; first non-blank text wins.
#[derive(Debug, Default, Deserialize)]
struct LooseOcrPayload {
    text: Option<String>,
    full_text: Option<String>,
    content: Option<String>,
    markdown: Option<String>,
    rendered_pages: Option<usize>,
    #[serde(default)]
    pages: LoosePages,
    #[serde(default)]
    page_items: Vec<LoosePage>,
    #[serde(default)]
    page_results: Vec<LoosePage>,
    data: Option<LooseNode>,
    result: Option<LooseNode>,
}

impl LooseOcrPayload {
    fn into_response(self) -> Result<OcrResponse, OcrError> {
        let mut text = [self.text, self.full_text, self.content, self.markdown]
            .into_iter()
            .flatten()
            .find(|t| !t.trim().is_empty())
            .or_else(|| self.data.as_ref().and_then(LooseNode::first_text))
            .or_else(|| self.result.as_ref().and_then(LooseNode::first_text))
            .unwrap_or_default();
        let raw_pages = if !self.page_items.is_empty() {
            self.page_items
        } else if !self.page_results.is_empty() {
            self.page_results
        } else {
            self.pages.items
        };
        let mut pages: Vec<PageText> = Vec::with_capacity(raw_pages.len());
        for p in raw_pages {
            let number = p.number();
            if number == 0 {
                continue;
            }
            let t = normalize_ocr_text(p.text());
            if t.is_empty() {
                continue;
            }
            pages.push(PageText { page_number: number, text: t, extract_failed: false });
        }
        if text.trim().is_empty() && !pages.is_empty() {
            text = pages
                .iter()
                .map(|p| p.text.clone())
                .collect::<Vec<_>>()
                .join("\n\n");
        }
        if text.trim().is_empty() {
            return Err(OcrError::Empty);
        }
        let rendered = self.rendered_pages.unwrap_or(0).max(self.pages.count);
        Ok(OcrResponse { text, rendered_pages: rendered, pages })
    }
}

/// Parse an OCR HTTP body: JSON payloads by content type, plain text as a
/// fallback (DEEIX `parseRemoteOCRResponse`).
fn parse_ocr_body(body: &[u8], content_type: &str) -> Result<OcrResponse, OcrError> {
    if content_type.to_ascii_lowercase().contains("application/json") {
        if let Ok(parsed) = serde_json::from_slice::<LooseOcrPayload>(body) {
            return parsed.into_response();
        }
    }
    let text = normalize_ocr_text(&String::from_utf8_lossy(body));
    if text.is_empty() {
        return Err(OcrError::Empty);
    }
    Ok(OcrResponse { text, rendered_pages: 0, pages: Vec::new() })
}


/// RapidOCR / Tesseract style HTTP backend (DEEIX `infra/extraction/ocr`).
///
/// Sends the source document as multipart `file` plus optional `page_ranges`
/// (`"1-3,5"`) and accepts either the loose JSON payload or a plain-text body.
pub(crate) struct RapidOcrHttp {
    base_url: String,
    auth_token: Option<String>,
    timeout: Duration,
    client: reqwest::Client,
}

impl RapidOcrHttp {
    /// `KALEIDO_OCR_URL` — unset means the OCR leg is unavailable (PDFs then
    /// return native text only, images fail with `ocr_unavailable`).
    pub(crate) fn from_env() -> Option<Self> {
        let base_url = std::env::var("KALEIDO_OCR_URL").ok()?;
        let base_url = base_url.trim().trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return None;
        }
        let timeout_secs: u64 = std::env::var("KALEIDO_OCR_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);
        Some(Self {
            auth_token: std::env::var("KALEIDO_OCR_TOKEN").ok().filter(|t| !t.trim().is_empty()),
            timeout: Duration::from_secs(timeout_secs),
            base_url,
            client: reqwest::Client::new(),
        })
    }

    async fn extract_inner(
        &self,
        file: &Path,
        file_name: &str,
        mime: &str,
        ranges: &[PageRange],
    ) -> Result<OcrResponse, OcrError> {
        let bytes = tokio::fs::read(file)
            .await
            .map_err(|e| OcrError::Other(format!("read_source_failed: {e}")))?;
        let mut form = reqwest::multipart::Form::new().part(
            "file",
            reqwest::multipart::Part::bytes(bytes).file_name(file_name.to_string()),
        );
        if !file_name.is_empty() {
            form = form.text("file_name", file_name.to_string());
        }
        if !mime.is_empty() {
            form = form.text("mime_type", mime.to_string());
        }
        let encoded = encode_page_ranges(ranges);
        if !encoded.is_empty() {
            form = form.text("page_ranges", encoded);
        }
        let mut req = self
            .client
            .post(&self.base_url)
            .multipart(form)
            .timeout(self.timeout)
            .header("Accept", "application/json, text/plain");
        if let Some(token) = &self.auth_token {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| OcrError::Other(format!("ocr_request_failed: {e}")))?;
        let status = resp.status();
        if status == StatusCode::NO_CONTENT {
            return Err(OcrError::Empty);
        }
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            let detail = detail.trim().chars().take(256).collect::<String>();
            return Err(match status {
                StatusCode::UNAUTHORIZED => OcrError::Unauthorized,
                StatusCode::FORBIDDEN => OcrError::Forbidden,
                StatusCode::UNPROCESSABLE_ENTITY => {
                    if detail.is_empty() {
                        OcrError::Unprocessable("empty detail".into())
                    } else {
                        OcrError::Unprocessable(detail)
                    }
                }
                _ => {
                    if detail.is_empty() {
                        OcrError::Http(status.as_u16())
                    } else {
                        OcrError::Failed(format!("http_{}: {}", status.as_u16(), detail))
                    }
                }
            });
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = resp
            .bytes()
            .await
            .map_err(|e| OcrError::Other(format!("ocr_body_read_failed: {e}")))?;
        parse_ocr_body(&body, &content_type)
    }
}


/// Options for one extraction request (form fields mirror DEEIX flags).
pub(crate) struct ExtractOptions {
    pub(crate) ocr_fallback: bool,
    pub(crate) image_ocr: bool,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self { ocr_fallback: true, image_ocr: true }
    }
}

/// Run one upload through the full DEEIX decision pipeline:
/// probe → builtin engine → (PDF) selective OCR over suspect pages.
pub(crate) async fn run_extract(
    file_name: &str,
    data: &[u8],
    opts: &ExtractOptions,
    ocr: Option<&RapidOcrHttp>,
) -> Result<serde_json::Value, (StatusCode, String)> {
    // Probe the category ourselves so orchestration can branch before the
    // builtin engine runs (DEEIX `upload/service.go` classification).
    let category = kaleido_core::extraction::probe_category(file_name, data);
    // Image has no builtin engine — the decision layer routes it straight to
    // OCR (DEEIX `extractLocalFile` branches before touching a text engine).
    let native = if category == FileCategory::Image {
        None
    } else {
        kaleido_core::extraction::extract_builtin(file_name, data, category).ok()
    };
    let (pages, text, native_failed) = match &native {
        Some(doc) => (doc.pages.clone(), doc.text.clone(), false),
        None if category == FileCategory::Image => (Vec::new(), String::new(), false),
        None => (Vec::new(), String::new(), true),
    };
    let decision = decide(&DecisionInput {
        category,
        ocr_fallback_enabled: opts.ocr_fallback && ocr.is_some(),
        image_ocr_enabled: opts.image_ocr && ocr.is_some(),
        pages: &pages,
        native_failed,
        native_text: &text,
    });

    match decision {
        Decision::Done => {
            let doc = native.ok_or_else(|| internal_err_now())?;
            Ok(json!({
                "text": doc.text,
                "pageCount": doc.pages.len(),
                "engine": doc.engine,
                "ocrUsed": false,
            }))
        }
        Decision::Fail(code) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "code": code, "message": code }).to_string(),
        )),
        Decision::OcrImage => {
            let Some(ocr) = ocr else {
                return Err(unavailable());
            };
            let mime = mime_from_name(file_name);
            let path = write_temp(data, file_name).await?;
            let result = ocr.extract_inner(&path, file_name, mime, &[]).await;
            cleanup(&path).await;
            let resp = result.map_err(ocr_err_to_status)?;
            Ok(json!({
                "text": resp.text,
                "pageCount": 1,
                "engine": "ocr_image",
                "ocrUsed": true,
            }))
        }
        Decision::OcrFile => {
            let Some(ocr) = ocr else {
                return Err(unavailable());
            };
            let mime = mime_from_name(file_name);
            let path = write_temp(data, file_name).await?;
            let result = ocr.extract_inner(&path, file_name, mime, &[]).await;
            cleanup(&path).await;
            let resp = result.map_err(ocr_err_to_status)?;
            Ok(json!({
                "text": resp.text,
                "pageCount": resp.rendered_pages.max(resp.pages.len()),
                "engine": "ocr_full",
                "ocrUsed": true,
            }))
        }
        Decision::OcrCandidatePages(ranges) => {
            let Some(ocr) = ocr else {
                return Err(unavailable());
            };
            let mime = mime_from_name(file_name);
            let path = write_temp(data, file_name).await?;
            let result = ocr.extract_inner(&path, file_name, mime, &ranges).await;
            cleanup(&path).await;
            let resp = result.map_err(ocr_err_to_status)?;
            let ocr_by_page = index_ocr_pages(&resp.pages);
            let text = join_pages(&pages, Some(&ocr_by_page));
            let text = if text.trim().is_empty() { resp.text } else { text };
            Ok(json!({
                "text": text,
                "pageCount": pages.len().max(resp.pages.len()),
                "engine": "builtin+ocr_selective",
                "ocrUsed": true,
                "ocrPages": encode_page_ranges(&ranges),
            }))
        }
    }
}


fn internal_err_now() -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, json!({ "code": "extract_failed", "message": "no engine result" }).to_string())
}

fn unavailable() -> (StatusCode, String) {
    (StatusCode::SERVICE_UNAVAILABLE, json!({ "code": "ocr_unavailable", "message": "OCR service not configured" }).to_string())
}

fn ocr_err_to_status(e: OcrError) -> (StatusCode, String) {
    let status = match e {
        OcrError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        OcrError::Unauthorized | OcrError::Forbidden => StatusCode::BAD_GATEWAY,
        OcrError::Http(_) | OcrError::Failed(_) | OcrError::Other(_) => StatusCode::BAD_GATEWAY,
        OcrError::Empty | OcrError::Unprocessable(_) => StatusCode::UNPROCESSABLE_ENTITY,
    };
    (status, json!({ "code": "ocr_failed", "message": e.to_string() }).to_string())
}

fn mime_from_name(name: &str) -> &'static str {
    match std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("tif") | Some("tiff") => "image/tiff",
        _ => "application/octet-stream",
    }
}

async fn write_temp(data: &[u8], file_name: &str) -> Result<PathBuf, (StatusCode, String)> {
    let ext = std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e))
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "kaleido_extract_{}{}",
        uuid::Uuid::new_v4().simple(),
        ext
    ));
    tokio::fs::write(&path, data)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("tmp_write_failed: {e}")))?;
    Ok(path)
}

async fn cleanup(path: &Path) {
    let _ = tokio::fs::remove_file(path).await;
}

/// POST /api/v1/kaleido-tools/extract — multipart `file` (+ optional
/// `ocrFallback` / `imageOcr` boolean fields) → extracted text JSON.
pub(crate) async fn extract_handler(
    State(_state): State<crate::AppState>,
    mut multipart: Multipart,
) -> Response {
    let mut data: Option<Vec<u8>> = None;
    let mut file_name = String::new();
    let mut opts = ExtractOptions::default();
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "file" => {
                if data.is_none() {
                    file_name = field.file_name().unwrap_or("").to_string();
                    if let Ok(bytes) = field.bytes().await {
                        if !bytes.is_empty() {
                            data = Some(bytes.to_vec());
                        }
                    }
                }
            }
            "ocrFallback" => {
                if let Ok(v) = field.text().await {
                    opts.ocr_fallback = !matches!(v.trim(), "false" | "0" | "");
                }
            }
            "imageOcr" => {
                if let Ok(v) = field.text().await {
                    opts.image_ocr = !matches!(v.trim(), "false" | "0" | "");
                }
            }
            _ => {}
        }
    }
    let Some(data) = data else {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({ "code": "TOOLS_FILE_FIELD_MISSING", "message": "missing multipart field 'file'" })),
        )
            .into_response();
    };
    let ocr = RapidOcrHttp::from_env();
    match run_extract(&file_name, &data, &opts, ocr.as_ref()).await {
        Ok(value) => axum::Json(value).into_response(),
        Err((status, message)) => (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            message,
        )
            .into_response(),
    }
}

pub(crate) fn router() -> Router<crate::AppState> {
    Router::new()
        .route("/api/v1/kaleido-tools/extract", post(extract_handler).get(extract_info))
}

async fn extract_info() -> axum::Json<serde_json::Value> {
    axum::Json(json!({
        "engine": "builtin",
        "formats": ["docx", "pdf", "xlsx", "xls", "csv", "txt", "png", "jpg"],
        "ocr": RapidOcrHttp::from_env().is_some(),
    }))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_traditional_text_field() {
        let body = br#"{"text": "hello page", "rendered_pages": 1}"#;
        let r = parse_ocr_body(body, "application/json").unwrap();
        assert_eq!(r.text, "hello page");
        assert_eq!(r.rendered_pages, 1);
    }

    #[test]
    fn falls_through_field_variants() {
        let body = br#"{"full_text": "via full_text"}"#;
        assert_eq!(parse_ocr_body(body, "application/json").unwrap().text, "via full_text");
        let body = br#"{"data": {"markdown": "nested md"}}"#;
        assert_eq!(parse_ocr_body(body, "application/json").unwrap().text, "nested md");
    }

    #[test]
    fn pages_list_and_joined_fallback() {
        let body = br#"{"pages": [
            {"page_number": 1, "text": "one"},
            {"page": 2, "content": "two"},
            {"index": 3, "markdown": "three"}
        ]}"#;
        let r = parse_ocr_body(body, "application/json").unwrap();
        assert_eq!(r.text, "one\n\ntwo\n\nthree");
        assert_eq!(r.pages.len(), 3);
        assert_eq!(r.pages[1].page_number, 2);
    }

    #[test]
    fn pages_as_bare_count() {
        let body = br#"{"text": "t", "pages": 5}"#;
        let r = parse_ocr_body(body, "application/json").unwrap();
        assert_eq!(r.rendered_pages, 5);
    }

    #[test]
    fn plain_text_body_passthrough() {
        let r = parse_ocr_body(b"  line one \n\n line two ", "text/plain").unwrap();
        assert_eq!(r.text, "line one\nline two");
        assert!(parse_ocr_body(b"\n\n ", "text/plain").is_err());
    }

    #[test]
    fn empty_json_is_empty_error() {
        let body = br#"{"text": "   "}"#;
        assert_eq!(parse_ocr_body(body, "application/json"), Err(OcrError::Empty));
    }

    #[test]
    fn mime_from_extension() {
        assert_eq!(mime_from_name("a.pdf"), "application/pdf");
        assert_eq!(mime_from_name("scan.PNG"), "image/png");
        assert_eq!(mime_from_name("weird.bin"), "application/octet-stream");
    }
}
