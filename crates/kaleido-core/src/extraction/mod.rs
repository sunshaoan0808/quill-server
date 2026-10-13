//! Builtin document extraction (PR5).
//!
//! DEEIX-Chat discipline ported to Rust: magic-byte probe picks the
//! engine, every engine emits normalized plain text so downstream
//! summarization sees a uniform shape. External engines (Tika/Docling)
//! and OCR fallback are wired in the orchestration PR, not here.

use serde::{Deserialize, Serialize};

pub mod docx;
pub mod ocr;
pub mod orchestrate;
pub mod normalize;
pub mod pdf;
pub mod sheet;

/// Document category decided from magic bytes first, filename last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileCategory {
    Text,
    Pdf,
    Word,
    Excel,
    Presentation,
    Image,
    Unknown,
}

/// One extracted page. `extract_failed` marks a page whose native text
/// came back empty — the OCR fallback in PR6 scans exactly these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageText {
    pub page_number: usize,
    pub text: String,
    pub extract_failed: bool,
}

/// Uniform result of a builtin extraction run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedDoc {
    pub text: String,
    pub pages: Vec<PageText>,
    pub kind: FileCategory,
    /// Engine that produced the text (`builtin_docx`, ...).
    pub engine: &'static str,
    /// Always false for builtin engines; kept so orchestration can
    /// aggregate builtin + OCR results into one shape.
    pub ocr_used: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("unsupported document kind: {0}")]
    Unsupported(&'static str),
    #[error("docx extraction failed: {0}")]
    Docx(String),
    #[error("pdf extraction failed: {0}")]
    Pdf(String),
    #[error("sheet extraction failed: {0}")]
    Sheet(String),
}

/// Probe the document kind. Magic bytes win; the filename extension is
/// only a tie-breaker for plain-text formats that carry no signature.
pub fn probe_category(filename: &str, data: &[u8]) -> FileCategory {
    if data.starts_with(b"%PDF") {
        return FileCategory::Pdf;
    }
    if data.len() >= 4 && &data[..2] == b"PK" {
        // OOXML container: which part files are present tells us the app.
        if let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(data)) {
            let names: Vec<String> = archive.file_names().map(str::to_string).collect();
            if names.iter().any(|n| n == "word/document.xml") {
                return FileCategory::Word;
            }
            if names.iter().any(|n| n.starts_with("xl/")) {
                return FileCategory::Excel;
            }
            if names.iter().any(|n| n.starts_with("ppt/")) {
                return FileCategory::Presentation;
            }
        }
        return FileCategory::Unknown;
    }
    if data.starts_with(&[0xD0, 0xCF, 0x11, 0xE0]) {
        // OLE2 compound file: legacy .xls vs .doc — extension decides.
        return match extension(filename).as_str() {
            "doc" | "docm" | "dot" => FileCategory::Word,
            _ => FileCategory::Excel,
        };
    }
    match extension(filename).as_str() {
        "txt" | "md" => FileCategory::Text,
        "pdf" => FileCategory::Pdf,
        "docx" | "doc" => FileCategory::Word,
        "xlsx" | "xls" | "xlsm" | "csv" => FileCategory::Excel,
        "pptx" | "ppt" => FileCategory::Presentation,
        "png" | "jpg" | "jpeg" | "gif" | "webp" => FileCategory::Image,
        _ => FileCategory::Unknown,
    }
}

/// Run the builtin engine matching `category`.
pub fn extract_builtin(
    filename: &str,
    data: &[u8],
    kind: FileCategory,
) -> Result<ExtractedDoc, ExtractError> {
    match kind {
        FileCategory::Word => {
            let text = docx::extract_docx_text(data)?;
            Ok(ExtractedDoc {
                text,
                pages: Vec::new(),
                kind,
                engine: "builtin_docx",
                ocr_used: false,
            })
        }
        FileCategory::Excel => {
            let text = sheet::extract_excel_text(filename, data)?;
            Ok(ExtractedDoc {
                text,
                pages: Vec::new(),
                kind,
                engine: "builtin_sheet",
                ocr_used: false,
            })
        }
        FileCategory::Pdf => pdf::extract_pdf(data),
        FileCategory::Text => {
            let raw = String::from_utf8_lossy(data);
            Ok(ExtractedDoc {
                text: normalize::normalize_document_text(&raw),
                pages: Vec::new(),
                kind,
                engine: "builtin_text",
                ocr_used: false,
            })
        }
        FileCategory::Presentation => Err(ExtractError::Unsupported(
            "presentation needs an external engine (tika/docling)",
        )),
        FileCategory::Image => {
            Err(ExtractError::Unsupported("image needs the OCR engine"))
        }
        FileCategory::Unknown => Err(ExtractError::Unsupported(
            "unknown document kind (no magic bytes, no known extension)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_prefers_magic_bytes_over_extension() {
        // PDF magic wins even with a misleading name.
        assert_eq!(probe_category("notes.txt", b"%PDF-1.4 abc"), FileCategory::Pdf);
        // Zip container with no known part is unknown, not text.
        assert_eq!(probe_category("a.zip", b"PK\x03\x04junkjunk"), FileCategory::Unknown);
        // Extension only decides signature-less formats.
        assert_eq!(probe_category("readme.md", "hi".as_bytes()), FileCategory::Text);
        assert_eq!(probe_category("data.csv", "a,b".as_bytes()), FileCategory::Excel);
        assert_eq!(probe_category("blob.bin", [0u8; 8].as_slice()), FileCategory::Unknown);
    }

    #[test]
    fn text_engine_normalizes() {
        let doc = extract_builtin("a.txt", b"  hello \r\n\r\n world  ", FileCategory::Text)
            .unwrap();
        assert_eq!(doc.engine, "builtin_text");
        assert!(!doc.ocr_used);
        assert_eq!(doc.text, "hello\nworld");
    }

    #[test]
    fn presentation_and_image_are_external_only() {
        assert!(matches!(
            extract_builtin("a.pptx", b"PK\x03\x04", FileCategory::Presentation),
            Err(ExtractError::Unsupported(_))
        ));
        assert!(matches!(
            extract_builtin("a.png", b"\x89PNG", FileCategory::Image),
            Err(ExtractError::Unsupported(_))
        ));
    }
}

fn extension(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}
