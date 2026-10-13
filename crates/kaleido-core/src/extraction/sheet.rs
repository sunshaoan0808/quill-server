//! Excel/CSV extraction.
//!
//! CSV parses natively (quoted fields, flexible record width); xlsx/xls
//! go through calamine. Row formatting mirrors DEEIX
//! `builtin/excel.go`: trim every cell, drop trailing empties, join with
//! ",". Sheet headers are `[Sheet: name]`, first sheet omitted.

use std::io::Cursor;

use calamine::{Data, Reader as _};

use super::ExtractError;

pub fn extract_excel_text(filename: &str, data: &[u8]) -> Result<String, ExtractError> {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    if ext == "csv" || ext == "tsv" {
        return Ok(extract_csv_text(data));
    }
    extract_workbook_text(data)
}

// --- CSV -------------------------------------------------------------------

/// DEEIX fallback discipline: a CSV that does not parse is passed
/// through trimmed instead of failing the whole extraction.
fn extract_csv_text(data: &[u8]) -> String {
    let raw = String::from_utf8_lossy(data);
    match parse_csv(&raw) {
        Ok(rows) => render_rows(rows),
        Err(_) => raw.trim().to_string(),
    }
}

/// Minimal RFC4180 reader: double-quote escaping, embedded commas and
/// newlines, CRLF tolerant. Errors only on an unterminated quote.
fn parse_csv(input: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' => {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        field.push('"');
                    } else {
                        in_quotes = false;
                    }
                }
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => in_quotes = true,
            '"' => field.push('"'),
            ',' => row.push(std::mem::take(&mut field)),
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            _ => field.push(c),
        }
    }
    if in_quotes {
        return Err("unterminated quoted field".into());
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    Ok(rows)
}

// --- xlsx / xls ------------------------------------------------------------

fn extract_workbook_text(data: &[u8]) -> Result<String, ExtractError> {
    // xlsx first (the common case), fall back to legacy OLE2 .xls.
    if let Ok(mut wb) = calamine::Xlsx::new(Cursor::new(data)) {
        return render_workbook(&mut wb);
    }
    let mut wb = calamine::Xls::new(Cursor::new(data))
        .map_err(|e| ExtractError::Sheet(format!("{e:?}")))?;
    render_workbook(&mut wb)
}

fn render_workbook<'a, R>(wb: &mut R) -> Result<String, ExtractError>
where
    R: calamine::Reader<Cursor<&'a [u8]>>,
{
    let names = wb.sheet_names();
    let mut first_sheet = true;
    let mut out = String::new();
    for name in names {
        let Ok(range) = wb.worksheet_range(&name) else {
            continue;
        };
        let rows: Vec<Vec<String>> = range
            .rows()
            .map(|row| row.iter().map(cell_to_string).collect())
            .collect();
        let rendered = render_rows(rows);
        if rendered.is_empty() {
            continue;
        }
        if !first_sheet {
            out.push_str("\n\n");
            out.push_str(&format!("[Sheet: {}]", name));
        }
        first_sheet = false;
        append_block(&mut out, &rendered);
    }
    Ok(out)
}

fn cell_to_string(cell: &Data) -> String {
    match cell {
        Data::String(s) => s.clone(),
        Data::Int(i) => i.to_string(),
        Data::Float(f) => f.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(dt) => dt.to_string(),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
        Data::Error(e) => format!("{e:?}"),
        Data::Empty => String::new(),
    }
}

/// DEEIX `normalizeExcelRow`: trim cells, drop trailing empties, join.
fn render_rows(rows: Vec<Vec<String>>) -> String {
    let mut lines: Vec<String> = Vec::with_capacity(rows.len());
    for row in rows {
        let cells: Vec<String> = row.into_iter().map(|c| c.trim().to_string()).collect();
        let last = cells.iter().rposition(|c| !c.is_empty());
        let Some(last) = last else { continue };
        lines.push(cells[..=last].join(","));
    }
    lines.join("\n")
}

fn append_block(out: &mut String, block: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(block);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn csv_quoted_fields_survive() {
        let text = extract_csv_text(b"a,\"b,c\",d\n\"multi\nline\",x,y\n");
        assert_eq!(text, "a,b,c,d\nmulti\nline,x,y");
    }

    #[test]
    fn csv_trailing_empties_dropped() {
        let text = extract_csv_text(b"1,2,,\n,,,\n3,4,5\n");
        assert_eq!(text, "1,2\n3,4,5");
    }

    #[test]
    fn csv_broken_quotes_fall_back_to_raw() {
        let text = extract_csv_text(b"  a,\"unterminated\n");
        assert_eq!(text, "a,\"unterminated");
    }

    #[test]
    fn csv_crlf_tolerated() {
        let text = extract_csv_text(b"h1,h2\r\nv1,v2\r\n");
        assert_eq!(text, "h1,h2\nv1,v2");
    }

    /// Hand-built xlsx: workbook + one sheet with inline strings.
    fn minimal_xlsx() -> Vec<u8> {
        let files = [
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets>
</workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData>
<row r="1"><c r="A1" t="inlineStr"><is><t>Name</t></is></c><c r="B1" t="inlineStr"><is><t>Score</t></is></c></row>
<row r="2"><c r="A2" t="inlineStr"><is><t>Alice</t></is></c><c r="B2"><v>95</v></c></row>
<row r="3"><c r="A3" t="inlineStr"><is><t>Bob</t></is></c><c r="B3"><v>87.5</v></c></row>
</sheetData>
</worksheet>"#,
            ),
        ];
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            for (name, content) in files {
                zip.start_file(name, opts).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn xlsx_cells_render_as_csv_rows() {
        let data = minimal_xlsx();
        let text = extract_workbook_text(&data).unwrap();
        assert_eq!(text, "Name,Score\nAlice,95\nBob,87.5");
    }

    #[test]
    fn probe_picks_excel_from_magic() {
        let data = minimal_xlsx();
        let kind = super::super::probe_category("file.xlsx", &data);
        assert_eq!(kind, super::super::FileCategory::Excel);
    }

    #[test]
    fn csv_dispatch_via_extension() {
        let text = extract_excel_text("notes.csv", b"x,y\n1,2\n").unwrap();
        assert_eq!(text, "x,y\n1,2");
    }
}
