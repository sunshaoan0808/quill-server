//! External document parsers — Apache Tika and Docling (PR7).
//!
//! Ports DEEIX `infra/extraction/{tika,docling}/client.go`. Both engines are
//! optional: without `KALEIDO_EXTRACT_ENGINE` (or a URL) the endpoint keeps
//! using the builtin extractors. When the primary external engine cannot
//! handle a category or fails, the pipeline falls back to builtin (the DEEIX
//! `documentParserEngine -> builtinEngine` demotion).

use std::path::Path;
use std::time::Duration;

use kaleido_core::extraction::ocr::{normalize_ocr_text, OcrError};
use kaleido_core::extraction::FileCategory;

/// HTTP client for Apache Tika (`PUT /tika` plain-text extraction).
pub(crate) struct TikaClient {
    base_url: String,
    auth_token: String,
    timeout: Duration,
    client: reqwest::Client,
}

/// HTTP client for a Docling REST service (multipart `POST`).
pub(crate) struct DoclingClient {
    base_url: String,
    auth_token: String,
    timeout: Duration,
    client: reqwest::Client,
}

/// The configured primary external engine, if any.
pub(crate) enum ExternalEngine {
    Tika(TikaClient),
    Docling(DoclingClient),
}

impl ExternalEngine {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            ExternalEngine::Tika(_) => "tika",
            ExternalEngine::Docling(_) => "docling",
        }
    }

    /// DEEIX supports matrix: Tika covers every document category;
    /// Docling is a PDF-only document parser.
    pub(crate) fn supports(&self, category: FileCategory) -> bool {
        match self {
            ExternalEngine::Tika(_) => matches!(
                category,
                FileCategory::Text
                    | FileCategory::Word
                    | FileCategory::Presentation
                    | FileCategory::Spreadsheet
                    | FileCategory::Pdf
            ),
            ExternalEngine::Docling(_) => category == FileCategory::Pdf,
        }
    }

    pub(crate) async fn extract(
        &self,
        path: &Path,
        mime: &str,
        file_name: &str,
    ) -> Result<String, String> {
        match self {
            ExternalEngine::Tika(c) => c.extract(path, mime, file_name).await,
            ExternalEngine::Docling(c) => c.extract(path, mime, file_name).await,
        }
    }

    /// Health probe (DEEIX `ProbeEndpoint`): GET `/tika` or `/healthz`.
    pub(crate) async fn probe(&self) -> bool {
        let url = match self {
            ExternalEngine::Tika(c) => format!("{}/tika", c.base_url),
            ExternalEngine::Docling(c) => DoclingClient::health_url(&c.base_url),
        };
        let client = match self {
            ExternalEngine::Tika(c) => c,
            ExternalEngine::Docling(c) => c,
        };
        let mut req = client.client.get(&url).timeout(Duration::from_millis(800));
        if let Some(auth) = auth_header(&client.auth_token) {
            req = req.header(reqwest::header::AUTHORIZATION, auth);
        }
        matches!(req.send().await, Ok(resp) if resp.status().is_success())
    }
}

fn auth_header(token: &str) -> Option<String> {
    let t = token.trim();
    if t.is_empty() {
        None
    } else {
        Some(format!("Bearer {t}"))
    }
}

/// DEEIX `ApplyAuthHeaders` writes the same token under three spellings;
/// we keep the standard Bearer header (the others exist for exotic proxies).
fn apply_auth(req: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    match auth_header(token) {
        Some(v) => req.header(reqwest::header::AUTHORIZATION, v),
        None => req,
    }
}

fn env_trimmed(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn env_timeout_secs(key: &str, default: u64) -> Duration {
    let secs = std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(default);
    Duration::from_secs(secs)
}

/// Resolve the primary engine from `KALEIDO_EXTRACT_ENGINE`.
/// Returns `None` for builtin mode (the default).
pub(crate) fn resolve_external_engine() -> Option<ExternalEngine> {
    let kind = env_trimmed("KALEIDO_EXTRACT_ENGINE")?.to_ascii_lowercase();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .ok()?;
    match kind.as_str() {
        "tika" => {
            let base_url = env_trimmed("KALEIDO_TIKA_URL")?.trim_end_matches('/').to_string();
            Some(ExternalEngine::Tika(TikaClient {
                base_url,
                auth_token: env_trimmed("KALEIDO_TIKA_TOKEN").unwrap_or_default(),
                timeout: env_timeout_secs("KALEIDO_TIKA_TIMEOUT_SECS", 60),
                client,
            }))
        }
        "docling" => {
            let base_url = env_trimmed("KALEIDO_DOCLING_URL")?.trim_end_matches('/').to_string();
            Some(ExternalEngine::Docling(DoclingClient {
                base_url,
                auth_token: env_trimmed("KALEIDO_DOCLING_TOKEN").unwrap_or_default(),
                timeout: env_timeout_secs("KALEIDO_DOCLING_TIMEOUT_SECS", 60),
                client,
            }))
        }
        _ => None,
    }
}

impl TikaClient {
    /// DEEIX `ExtractText`: PUT the raw file at `/tika`, read plain text.
    pub(crate) async fn extract(
        &self,
        path: &Path,
        mime: &str,
        file_name: &str,
    ) -> Result<String, String> {
        let data = tokio::fs::read(path).await.map_err(|e| format!("tika_read: {e}"))?;
        let mut req = self
            .client
            .put(format!("{}/tika", self.base_url))
            .timeout(self.timeout)
            .header(reqwest::header::ACCEPT, "text/plain");
        if !mime.is_empty() {
            req = req.header(reqwest::header::CONTENT_TYPE, mime);
        }
        if !file_name.is_empty() {
            req = req.header("X-Tika-Filename", file_name);
        }
        let req = apply_auth(req, &self.auth_token).body(data);
        let resp = req.send().await.map_err(|e| format!("tika_http: {e}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Err("tika_empty_content".into());
        }
        if !status.is_success() {
            return Err(tika_status_error(status).to_string());
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("tika_read_body: {e}"))?;
        let text = normalize_ocr_text(&String::from_utf8_lossy(&body));
        if text.is_empty() {
            return Err("tika_empty_content".into());
        }
        Ok(text)
    }
}

/// DEEIX status mapping for the Tika client.
fn tika_status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "tika_unauthorized".into(),
        403 => "tika_forbidden".into(),
        415 => "tika_unsupported_media_type".into(),
        422 => "tika_unprocessable".into(),
        n => format!("tika_http_{n}"),
    }
}

impl DoclingClient {
    /// Health URL: a base ending in `/ocr` swaps that suffix for `/healthz`
    /// (DEEIX `resolveHealthURL`).
    pub(crate) fn health_url(base_url: &str) -> String {
        if let Some(prefix) = base_url.strip_suffix("/ocr") {
            format!("{prefix}/healthz")
        } else {
            format!("{base_url}/healthz")
        }
    }

    /// DEEIX `ExtractText`: multipart POST `file` + `file_name` + `mime_type`.
    pub(crate) async fn extract(
        &self,
        path: &Path,
        mime: &str,
        file_name: &str,
    ) -> Result<String, String> {
        let data = tokio::fs::read(path).await.map_err(|e| format!("docling_read: {e}"))?;
        let part = reqwest::multipart::Part::bytes(data).file_name(file_name.to_string());
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if !file_name.is_empty() {
            form = form.text("file_name", file_name.to_string());
        }
        if !mime.is_empty() {
            form = form.text("mime_type", mime.to_string());
        }
        let req = apply_auth(
            self.client
                .post(&self.base_url)
                .timeout(self.timeout)
                .header(reqwest::header::ACCEPT, "application/json, text/plain"),
            &self.auth_token,
        )
        .multipart(form);
        let resp = req.send().await.map_err(|e| format!("docling_http: {e}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NO_CONTENT {
            return Err("docling_empty_content".into());
        }
        if !status.is_success() {
            return Err(docling_status_error(status).to_string());
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let body = resp
            .bytes()
            .await
            .map_err(|e| format!("docling_read_body: {e}"))?;
        // Same loose payload shape as the OCR wire — reuse the PR6 parser.
        let parsed = crate::extract_ocr::parse_ocr_body(&body, &content_type)
            .map_err(|e: OcrError| e.to_string())?;
        if parsed.text.is_empty() {
            return Err("docling_empty_content".into());
        }
        Ok(parsed.text)
    }
}

/// DEEIX status mapping for the Docling client.
fn docling_status_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "docling_unauthorized".into(),
        403 => "docling_forbidden".into(),
        422 => "docling_unprocessable".into(),
        n => format!("docling_http_{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docling_supports_pdf_only() {
        let engine = ExternalEngine::Docling(DoclingClient {
            base_url: "http://x".into(),
            auth_token: String::new(),
            timeout: Duration::from_secs(1),
            client: reqwest::Client::new(),
        });
        assert!(engine.supports(FileCategory::Pdf));
        assert!(!engine.supports(FileCategory::Word));
        assert!(!engine.supports(FileCategory::Image));
    }

    #[test]
    fn docling_health_url_swaps_ocr_suffix() {
        assert_eq!(
            DoclingClient::health_url("http://127.0.0.1:8005/ocr"),
            "http://127.0.0.1:8005/healthz"
        );
        assert_eq!(
            DoclingClient::health_url("http://docling:5001"),
            "http://docling:5001/healthz"
        );
    }

    #[test]
    fn status_codes_match_deeix_error_names() {
        assert_eq!(
            tika_status_error(reqwest::StatusCode::UNAUTHORIZED),
            "tika_unauthorized"
        );
        assert_eq!(
            tika_status_error(reqwest::StatusCode::UNSUPPORTED_MEDIA_TYPE),
            "tika_unsupported_media_type"
        );
        assert_eq!(
            docling_status_error(reqwest::StatusCode::UNPROCESSABLE_ENTITY),
            "docling_unprocessable"
        );
        assert_eq!(docling_status_error(reqwest::StatusCode::NOT_FOUND), "docling_http_404");
    }

    #[test]
    fn normalize_strips_blank_lines_like_deeix() {
        assert_eq!(normalize_ocr_text("  a  \n\n b "), "a\nb");
        assert_eq!(normalize_ocr_text(""), "");
    }
}
