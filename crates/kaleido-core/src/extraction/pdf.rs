//! PDF extraction via pdf-extract (lopdf 0.42 line, RUSTSEC-patched).
//!
//! Pages are normalized individually so an empty page keeps its slot in
//! `pages` — the PR6 OCR fallback scans for exactly those slots.

use super::{normalize, ExtractError, ExtractedDoc, PageText};

pub fn extract_pdf(data: &[u8]) -> Result<ExtractedDoc, ExtractError> {
    let raw_pages =
        pdf_extract::extract_text_from_mem_by_pages(data).map_err(|e| ExtractError::Pdf(e.to_string()))?;

    let mut pages = Vec::with_capacity(raw_pages.len());
    let mut kept: Vec<String> = Vec::with_capacity(raw_pages.len());
    for (idx, raw) in raw_pages.into_iter().enumerate() {
        let text = normalize::normalize_extracted_pdf_text(&raw);
        // A page with no native text after normalization is an OCR
        // candidate; DEEIX flags it the same way (PDFText.Pages).
        let extract_failed = text.is_empty();
        if !extract_failed {
            kept.push(text.clone());
        }
        pages.push(PageText {
            page_number: idx + 1,
            text,
            extract_failed,
        });
    }

    Ok(ExtractedDoc {
        text: kept.join("\n"),
        pages,
        kind: super::FileCategory::Pdf,
        engine: "builtin_pdf",
        ocr_used: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal one-page PDF with a single text-showing operator.
    fn minimal_pdf(contents: &str) -> Vec<u8> {
        let stream = format!("BT /F1 24 Tf 72 720 Td ({}) Tj ET", contents);
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
                .to_string(),
            // lopdf: eol after `stream`, take(Length), optional eol, `endstream`.
            format!("<< /Length {} >>\nstream\n{}\nendstream", stream.len(), stream),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];

        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{}\nendobj\n", i + 1, obj));
        }
        let xref_at = out.len();
        out.push_str(&format!("xref\n0 {}\n", objects.len() + 1));
        out.push_str("0000000000 65535 f \n");
        for off in &offsets {
            out.push_str(&format!("{:010} 00000 n \n", off));
        }
        out.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
            objects.len() + 1,
            xref_at
        ));
        out.into_bytes()
    }

    #[test]
    fn extracts_page_text_and_flags_pages() {
        let doc = extract_pdf(&minimal_pdf("Hello")).unwrap();
        assert_eq!(doc.kind, super::super::FileCategory::Pdf);
        assert_eq!(doc.engine, "builtin_pdf");
        assert!(!doc.ocr_used);
        assert_eq!(doc.pages.len(), 1);
        assert!(doc.pages[0].text.contains("Hello"));
        assert!(!doc.pages[0].extract_failed);
        assert!(doc.text.contains("Hello"));
    }

    #[test]
    fn garbage_pdf_never_panics() {
        // lopdf either errors on load or yields an empty document;
        // both must come back without a panic and without fake text.
        match extract_pdf(b"%PDF-1.4\nnot a real pdf") {
            Err(_) => {}
            Ok(doc) => {
                assert!(doc.pages.is_empty());
                assert!(doc.text.is_empty());
            }
        }
    }
}
