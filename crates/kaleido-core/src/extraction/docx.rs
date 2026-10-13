//! DOCX extraction: read `word/document.xml` out of the OOXML zip and
//! stream-parse paragraphs and tables with quick-xml.
//!
//! Output shape follows DEEIX `builtin/word.go`: every paragraph becomes
//! one line; table cells become one line each, laid out row by row;
//! horizontal-merge continuation cells are skipped (their text lives in
//! the origin cell).

use std::io::Cursor;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::{normalize, ExtractError};

pub fn extract_docx_text(data: &[u8]) -> Result<String, ExtractError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(data)).map_err(|e| ExtractError::Docx(e.to_string()))?;
    let xml = {
        let mut file = archive
            .by_name("word/document.xml")
            .map_err(|e| ExtractError::Docx(e.to_string()))?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut buf)
            .map_err(|e| ExtractError::Docx(e.to_string()))?;
        buf
    };
    let xml = String::from_utf8_lossy(&xml);
    let body = parse_document_xml(&xml)?;
    Ok(normalize::normalize_document_text(&body))
}

fn parse_document_xml(xml: &str) -> Result<String, ExtractError> {
    let mut reader = Reader::from_str(xml);

    let mut out: Vec<String> = Vec::new();
    // Current paragraph buffer; None while outside any w:p.
    let mut para: Option<String> = None;
    // True while inside a <w:t> run (text nodes elsewhere are ignored,
    // e.g. field instructions in <w:instrText>).
    let mut in_text = false;
    // Stack of table-cell states: (is_merge_continuation, cell_paragraphs).
    // Pushed on w:tc, popped on /w:tc.
    let mut cells: Vec<(bool, Vec<String>)> = Vec::new();
    // Current cell depth while scanning w:tcPr for hMerge.
    let mut in_tc_props = false;
    let mut in_tc_prop = false;
    let mut tc_prop_is_hmerge = false;

    loop {
        let event = reader
            .read_event()
            .map_err(|e| ExtractError::Docx(e.to_string()))?;
        match event {
            Event::Start(e) => {
                // Bind the QName: as_ref() borrows the temporary otherwise.
                let qname = e.name();
                let name = local_name(qname.as_ref());
                match name {
                    b"w:p" => {
                        para.get_or_insert_with(String::new);
                    }
                    b"w:t" => in_text = true,
                    b"w:tab" => {
                        if let Some(p) = para.as_mut() {
                            p.push('\t');
                        }
                    }
                    b"w:tc" => cells.push((false, Vec::new())),
                    b"w:tcPr" => in_tc_props = true,
                    b"w:gridSpan" | b"w:hMerge" if in_tc_props => {
                        in_tc_prop = true;
                        tc_prop_is_hmerge = name == b"w:hMerge";
                    }
                    _ => {}
                }
                // Detect horizontal-merge continuation: <w:hMerge
                // w:val="continue"> (the default val is also "continue").
                if tc_prop_is_hmerge && in_tc_prop {
                    let is_restart = e.attributes().flatten().any(|a| {
                        a.key.as_ref() == b"w:val"
                            && matches!(
                                a.value.as_ref(),
                                b"restart" | b"clear" | b"none" | b"continuous"
                            )
                    });
                    if !is_restart {
                        if let Some(state) = cells.last_mut() {
                            state.0 = true;
                        }
                    }
                }
            }
            Event::Empty(e) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                match name {
                    b"w:tab" => {
                        if let Some(p) = para.as_mut() {
                            p.push('\t');
                        }
                    }
                    b"w:tc" => cells.push((false, Vec::new())),
                    b"w:tcPr" => {}
                    b"w:gridSpan" | b"w:hMerge" if in_tc_props => {
                        if name == b"w:hMerge" {
                            let is_restart = e.attributes().flatten().any(|a| {
                                a.key.as_ref() == b"w:val"
                                    && matches!(
                                        a.value.as_ref(),
                                        b"restart" | b"clear" | b"none" | b"continuous"
                                    )
                            });
                            if !is_restart {
                                if let Some(state) = cells.last_mut() {
                                    state.0 = true;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::Text(t) if in_text => {
                // EncodingError: Debug only (quick-xml 0.41)
                let text = t
                    .xml10_content()
                    .map_err(|e| ExtractError::Docx(format!("{e:?}")))?;
                if let Some(p) = para.as_mut() {
                    p.push_str(&text);
                }
            }
            Event::End(e) => {
                let qname = e.name();
                match local_name(qname.as_ref()) {
                b"w:t" => in_text = false,
                b"w:p" => {
                    if let Some(text) = para.take() {
                        let text = text.trim();
                        if !text.is_empty() {
                            match cells.last_mut() {
                                Some((_, cell_paras)) => cell_paras.push(text.to_string()),
                                None => out.push(text.to_string()),
                            }
                        }
                    }
                }
                b"w:tc" => {
                    if let Some((is_continuation, cell_paras)) = cells.pop() {
                        if !is_continuation {
                            out.extend(cell_paras);
                        }
                    }
                }
                b"w:tcPr" => {
                    in_tc_props = false;
                    in_tc_prop = false;
                    tc_prop_is_hmerge = false;
                }
                b"w:gridSpan" | b"w:hMerge" => {
                    in_tc_prop = false;
                    tc_prop_is_hmerge = false;
                }
                _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(out.join("\n"))
}

/// Strip the namespace prefix: "w:p" -> "p".
fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().position(|b| *b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn build_docx(files: &[(&str, &str)]) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in files {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn paragraphs_become_lines() {
        let doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p><w:r><w:t>First line</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Second </w:t><w:t>line</w:t></w:r></w:p>
<w:p><w:r><w:t>   </w:t></w:r></w:p>
<w:p/>
</w:body></w:document>"#;
        let data = build_docx(&[("word/document.xml", doc)]);
        assert_eq!(extract_docx_text(&data).unwrap(), "First line\nSecond line");
    }

    #[test]
    fn table_cells_emit_one_line_each() {
        let doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p><w:r><w:t>Before table</w:t></w:r></w:p>
<w:tbl>
<w:tr>
<w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc>
<w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc>
</w:tr>
<w:tr>
<w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc>
<w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc>
</w:tr>
</w:tbl>
<w:p><w:r><w:t>After table</w:t></w:r></w:p>
</w:body></w:document>"#;
        let data = build_docx(&[("word/document.xml", doc)]);
        assert_eq!(
            extract_docx_text(&data).unwrap(),
            "Before table\nA1\nB1\nA2\nB2\nAfter table"
        );
    }

    #[test]
    fn hmerge_continuation_cells_are_skipped() {
        let doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:tbl>
<w:tr>
<w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>Merged</w:t></w:r></w:p></w:tc>
</w:tr>
<w:tr>
<w:tc><w:tcPr><w:hMerge w:val="continue"/></w:tcPr><w:p><w:r><w:t>dup</w:t></w:r></w:p></w:tc>
<w:tc><w:p><w:r><w:t>fresh</w:t></w:r></w:p></w:tc>
</w:tr>
</w:tbl>
</w:body></w:document>"#;
        let data = build_docx(&[("word/document.xml", doc)]);
        assert_eq!(extract_docx_text(&data).unwrap(), "Merged\nfresh");
    }

    #[test]
    fn field_instructions_are_ignored() {
        let doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
<w:p>
<w:r><w:instrText>PAGEREF _Toc123 \h</w:instrText></w:r>
<w:r><w:t>42</w:t></w:r>
</w:p>
</w:body></w:document>"#;
        let data = build_docx(&[("word/document.xml", doc)]);
        assert_eq!(extract_docx_text(&data).unwrap(), "42");
    }

    #[test]
    fn missing_document_xml_errors() {
        let data = build_docx(&[("other.xml", "<x/>")]);
        assert!(extract_docx_text(&data).is_err());
    }
}
