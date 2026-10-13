//! OCR engine port — the abstraction the orchestration layer calls.
//!
//! Mirrors DEEIX `ports/extraction/ocr.go`: an engine receives a document
//! (already written to disk by the server) plus optional page ranges and
//! returns normalized text. The concrete HTTP client (RapidOCR / Tesseract
//! style services) lives in kaleido-server; core only defines the shape so
//! orchestration tests can use a stub.

use super::PageText;

/// One OCR engine's answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OcrResponse {
    /// Joined text of every page that produced content.
    pub text: String,
    /// How many pages the engine actually rendered / read.
    pub rendered_pages: usize,
    /// Per-page text (page numbers 1-based).
    pub pages: Vec<PageText>,
}

/// Error codes the orchestration layer surfaces to callers. Same strings as
/// DEEIX so frontend handling stays portable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OcrError {
    #[error("ocr_unavailable")]
    Unavailable,
    #[error("ocr_empty")]
    Empty,
    #[error("ocr_unauthorized")]
    Unauthorized,
    #[error("ocr_forbidden")]
    Forbidden,
    #[error("ocr_unprocessable: {0}")]
    Unprocessable(String),
    #[error("ocr_http_{0}")]
    Http(u16),
    #[error("ocr_failed: {0}")]
    Failed(String),
    #[error("{0}")]
    Other(String),
}

/// Trim each line, drop blanks, rejoin with `\n` (DEEIX `normalizeOCRText`).
pub fn normalize_ocr_text(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
