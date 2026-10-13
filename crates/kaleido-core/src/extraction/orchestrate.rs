//! Extraction orchestration — deterministic decision layer.
//!
//! Direct translation of DEEIX `application/extraction/service.go` logic that
//! needs no I/O: which pages of a PDF deserve OCR, how native and OCR page
//! texts merge, and what the top-level plan is for a given category. The HTTP
//! OCR client and the extraction engine live in kaleido-server / `super`;
//! this module only decides.
//!
//! Reference discipline (DEEIX `collectPDFOCRCandidatePages`):
//! - empty / whitespace-only page → OCR
//! - fewer than 24 letters+digits → OCR
//! - garbled Chinese PDF heuristics (file name gated) → OCR
//! - mojibake runes → OCR

use std::collections::HashMap;

use super::{FileCategory, PageText};

/// Inclusive 1-based page interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    pub start: usize,
    pub end: usize,
}

/// Below this many letters+digits a page is considered image-like.
const MIN_MEANINGFUL_CHARS: usize = 24;

/// Count runes that carry actual content (letters or numbers).
pub fn count_meaningful_chars(s: &str) -> usize {
    s.chars().filter(|c| c.is_alphabetic() || c.is_numeric()).count()
}

/// True when a PDF page's native text looks too thin or corrupt for chat use.
pub fn should_ocr_page(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return true;
    }
    if count_meaningful_chars(trimmed) < MIN_MEANINGFUL_CHARS {
        return true;
    }
    looks_like_mojibake_pdf_text(trimmed)
}

/// True when the page smells like a mis-decoded Chinese PDF.
///
/// DEEIX gates the whole heuristic on the file name containing Han characters —
/// without that gate ordinary Latin documents with dense layout would be
/// misflagged. Statistics then classify: replacement/private-use runes always
/// flag; a majority-Han page never flags; otherwise a Latin-dense page with
/// almost no spaces (CID font junk) flags.
pub fn looks_like_garbled_chinese_pdf_text(file_name: &str, text: &str) -> bool {
    let meaningful = count_meaningful_chars(text);
    if meaningful == 0 || !file_name.chars().any(contains_han) {
        return false;
    }
    let mut han = 0usize;
    let mut latin_dense = 0usize;
    let mut whitespace = 0usize;
    let mut mojibake = 0usize;
    let mut replacement = 0usize;
    let mut private_use = 0usize;
    let mut non_ascii = 0usize;
    let mut other = 0usize;
    let mut symbols = 0usize;
    let total = text.chars().count();
    for c in text.chars() {
        if c.is_whitespace() {
            whitespace += 1;
            continue;
        }
        if c == '\u{FFFD}' {
            replacement += 1;
        }
        if is_private_use_rune(c) {
            private_use += 1;
        }
        if !c.is_ascii() {
            non_ascii += 1;
        }
        if c.is_ascii_punctuation() {
            symbols += 1;
        }
        if is_likely_mojibake_rune(c) {
            mojibake += 1;
        }
        if contains_han(c) {
            han += 1;
        } else if c.is_ascii_alphabetic() {
            latin_dense += 1;
        }
    }
    if total == 0 {
        return false;
    }
    if replacement > 0 || private_use > 0 {
        return true;
    }
    if han * 10 >= meaningful * 2 {
        return false;
    }
    let latin_dense_hit = meaningful > 0 && latin_dense * 5 >= meaningful * 4;
    let mojibake_heavy = meaningful > 0 && mojibake * 10 >= meaningful * 3;
    let non_ascii_heavy = meaningful > 0 && non_ascii * 10 >= meaningful * 3;
    if !latin_dense_hit && !mojibake_heavy && !non_ascii_heavy {
        return false;
    }
    let too_few_spaces = total <= 8 || whitespace * 20 < total;
    let symbol_heavy = meaningful > 0 && symbols * 10 >= meaningful * 3;
    too_few_spaces || symbol_heavy || mojibake_heavy
}

/// True when the text carries mojibake runes (UTF-8 bytes decoded as Latin-1).
pub fn looks_like_mojibake_pdf_text(text: &str) -> bool {
    let meaningful = count_meaningful_chars(text);
    if meaningful == 0 {
        return false;
    }
    let mut hits = 0usize;
    for c in text.chars() {
        if is_likely_mojibake_rune(c) {
            hits += 1;
        }
    }
    hits > 0 && hits * 10 >= meaningful * 3
}

/// Latin-1 range runes that typically appear when UTF-8 bytes were decoded
/// one byte per rune (Ã, Â, â, ©…) or private-use replacement glyphs.
fn is_likely_mojibake_rune(c: char) -> bool {
    matches!(c,
        '\u{00C0}'..='\u{00FF}'
            | '\u{00AB}'
            | '\u{00BB}'
            | '\u{2018}'..='\u{201F}'
            | '\u{2026}'
            | '\u{2013}'
            | '\u{2014}'
    ) || is_private_use_rune(c)
}

fn is_private_use_rune(c: char) -> bool {
    matches!(c, '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}')
}

fn contains_han(c: char) -> bool {
    let cp = c as u32;
    (0x3400..=0x4DBF).contains(&cp)
        || (0x4E00..=0x9FFF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0x20000..=0x2A6DF).contains(&cp)
        || (0x2A700..=0x2EBEF).contains(&cp)
}

/// Page numbers (1-based) that should be sent to OCR.
pub fn collect_ocr_candidate_pages(pages: &[PageText]) -> Vec<usize> {
    pages
        .iter()
        .filter(|p| p.extract_failed || should_ocr_page(&p.text))
        .map(|p| p.page_number)
        .collect()
}

/// Sort and run-compress page numbers into inclusive ranges.
pub fn compact_page_numbers_to_ranges(pages: &[usize]) -> Vec<PageRange> {
    if pages.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<usize> = pages.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut result: Vec<PageRange> = Vec::with_capacity(sorted.len());
    let mut start = sorted[0];
    let mut end = sorted[0];
    for &p in &sorted[1..] {
        if p == end + 1 {
            end = p;
            continue;
        }
        result.push(PageRange { start, end });
        start = p;
        end = p;
    }
    result.push(PageRange { start, end });
    result
}

/// `"1-3,5"` wire format understood by the RapidOCR/Tesseract HTTP services.
pub fn encode_page_ranges(ranges: &[PageRange]) -> String {
    ranges
        .iter()
        .filter(|r| r.start > 0)
        .map(|r| {
            let end = if r.end < r.start { r.start } else { r.end };
            if r.start == end {
                format!("{}", r.start)
            } else {
                format!("{}-{}", r.start, end)
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Expand ranges back to a sorted, deduplicated page list.
pub fn resolve_page_numbers(ranges: &[PageRange]) -> Vec<usize> {
    let mut result: Vec<usize> = Vec::new();
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for r in ranges {
        if r.start == 0 {
            continue;
        }
        let end = if r.end < r.start { r.start } else { r.end };
        for p in r.start..=end {
            if seen.insert(p) {
                result.push(p);
            }
        }
    }
    result
}

/// Every page 1..=count as single-page ranges.
pub fn full_page_ranges(count: usize) -> Vec<PageRange> {
    (1..=count).map(|p| PageRange { start: p, end: p }).collect()
}

/// Map page_number → text for OCR results.
pub fn index_ocr_pages(pages: &[PageText]) -> HashMap<usize, String> {
    pages.iter().map(|p| (p.page_number, p.text.clone())).collect()
}

/// DEEIX `mergeBuiltinPDFPages`: native pages stay, non-empty OCR text
/// overwrites page-by-page, joined with newlines.
pub fn join_pages(native: &[PageText], ocr_override: Option<&HashMap<usize, String>>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(native.len());
    for p in native {
        let replacement = ocr_override
            .and_then(|m| m.get(&p.page_number))
            .map(String::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let text = match replacement {
            Some(t) => t.to_string(),
            None => p.text.clone(),
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        parts.push(trimmed.to_string());
    }
    parts.join("\n")
}

/// Top-level plan for one extraction request (DEEIX `extractLocalFile`
/// minus the I/O): the server executes the OCR leg, then merges with
/// [`join_pages`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Native extraction is good enough — return it as-is.
    Done,
    /// Native PDF text has suspect pages: OCR exactly these ranges server-side,
    /// then merge over the native pages.
    OcrCandidatePages(Vec<PageRange>),
    /// Native extraction failed hard but the document is still worth an
    /// OCR attempt over the whole file (nil page ranges on the wire).
    OcrFile,
    /// Image category: OCR the image itself.
    OcrImage,
    /// Terminal failure, DEEIX-style error code.
    Fail(&'static str),
}

/// Everything `decide` needs to know — filled in by the caller from the
/// native extraction run plus the feature flags.
pub struct DecisionInput<'a> {
    pub category: FileCategory,
    pub ocr_fallback_enabled: bool,
    pub image_ocr_enabled: bool,
    /// PDF pages (empty slice for non-PDF or when the native run failed).
    pub pages: &'a [PageText],
    /// Native extraction returned an error (or panicked upstream).
    pub native_failed: bool,
    /// Whole-document native text (already normalized).
    pub native_text: &'a str,
}

/// Deterministic extraction decision. Mirrors DEEIX `extractLocalFile`:
///
/// - image → OCR when enabled, else `image_ocr_disabled`
/// - PDF with fallback → suspect pages get selective OCR; clean PDFs pass
///   through; hard native failures get one full-file OCR attempt
/// - PDF without fallback → empty text is `pdf_no_extractable_text`
/// - anything else → empty or failed is `extract_failed`
pub fn decide(input: &DecisionInput<'_>) -> Decision {
    match input.category {
        FileCategory::Image => {
            if input.image_ocr_enabled {
                Decision::OcrImage
            } else {
                Decision::Fail("image_ocr_disabled")
            }
        }
        FileCategory::Pdf => {
            if input.native_failed {
                return if input.ocr_fallback_enabled {
                    Decision::OcrFile
                } else {
                    Decision::Fail("extract_failed")
                };
            }
            let text_empty = input.native_text.trim().is_empty();
            if input.ocr_fallback_enabled {
                let candidates = collect_ocr_candidate_pages(input.pages);
                if !candidates.is_empty() {
                    return Decision::OcrCandidatePages(compact_page_numbers_to_ranges(&candidates));
                }
                return if text_empty {
                    Decision::Fail("pdf_no_extractable_text")
                } else {
                    Decision::Done
                };
            }
            if text_empty {
                Decision::Fail("pdf_no_extractable_text")
            } else {
                Decision::Done
            }
        }
        _ => {
            if input.native_failed || input.native_text.trim().is_empty() {
                Decision::Fail("extract_failed")
            } else {
                Decision::Done
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn long_ok() -> String {
        "This page contains plenty of readable body text with enough letters and digits to pass the threshold comfortably."
            .to_string()
    }

    fn page(n: usize, text: &str) -> PageText {
        PageText { page_number: n, text: text.to_string(), extract_failed: text.trim().is_empty() }
    }

    #[test]
    fn meaningful_counts_letters_and_digits() {
        assert_eq!(count_meaningful_chars("ab 12 !@#"), 4); // a,b,1,2 — punctuation/spaces do not count
        assert_eq!(count_meaningful_chars(""), 0);
        assert_eq!(count_meaningful_chars("中文也算"), 4);
    }

    #[test]
    fn short_or_empty_page_wants_ocr() {
        assert!(should_ocr_page(""));
        assert!(should_ocr_page("   \n "));
        assert!(should_ocr_page("Too short."));
        assert!(!should_ocr_page(&long_ok()));
    }

    #[test]
    fn mojibake_page_wants_ocr() {
        // UTF-8 bytes of "中文" decoded as Latin-1, dense enough that the
        // mojibake ratio check trips (mixing in normal prose would dilute it).
        let mojibake = "\u{c3}\u{a4}\u{c2}\u{b8}\u{c2}\u{ad}\u{c3}\u{a6}".repeat(8);
        assert!(should_ocr_page(&mojibake));
    }

    #[test]
    fn clean_chinese_page_not_flagged() {
        let text = "这是一段正常的中文文档页面内容足够长可以超过最少字符数量限制确保不会被误判为需要OCR的页面。";
        assert!(!looks_like_garbled_chinese_pdf_text("报告.pdf", text));
        assert!(!should_ocr_page(text));
    }

    #[test]
    fn garbled_chinese_needs_han_filename() {
        let junk = "Ã¤Â¸Â­Ã¦â&#162;&#186;Ã©â&#128;&#153;Ã¥â&#128;&#161;xx";
        // dense Latin junk without Han in the file name → not this heuristic
        assert!(!looks_like_garbled_chinese_pdf_text("report.pdf", junk));
        // Han file name + dense no-space junk → flag
        assert!(looks_like_garbled_chinese_pdf_text("扫描件.pdf", junk));
    }

    #[test]
    fn garbled_detection_is_gated_on_density() {
        // file name with Han, text mostly Han → normal, never flag
        let normal = "报告正文：这是一个正常内容，字符统计显示汉字占绝大多数比例。";
        assert!(!looks_like_garbled_chinese_pdf_text("扫描件.pdf", normal));
        // replacement char always flags
        let repl = format!("{}abc", '\u{FFFD}');
        assert!(looks_like_garbled_chinese_pdf_text("扫描件.pdf", &repl));
    }

    #[test]
    fn candidates_pick_failed_and_thin_pages() {
        let pages = vec![
            page(1, &long_ok()),
            page(2, ""),
            page(3, "tiny"),
            page(4, &long_ok()),
        ];
        assert_eq!(collect_ocr_candidate_pages(&pages), vec![2, 3]);
    }

    #[test]
    fn ranges_roundtrip() {
        let nums = vec![1, 2, 3, 5, 7, 8];
        let ranges = compact_page_numbers_to_ranges(&nums);
        assert_eq!(
            ranges,
            vec![
                PageRange { start: 1, end: 3 },
                PageRange { start: 5, end: 5 },
                PageRange { start: 7, end: 8 },
            ]
        );
        assert_eq!(encode_page_ranges(&ranges), "1-3,5,7-8");
        assert_eq!(resolve_page_numbers(&ranges), nums);
        assert_eq!(encode_page_ranges(&[]), "");
    }

    #[test]
    fn full_ranges_cover_every_page() {
        assert_eq!(full_page_ranges(3).len(), 3);
        assert_eq!(encode_page_ranges(&full_page_ranges(2)), "1,2");
    }

    #[test]
    fn join_ocr_overlays_native_pages() {
        let native = vec![page(1, "first"), page(2, ""), page(3, "third")];
        let mut ocr = std::collections::HashMap::new();
        ocr.insert(2, "second (OCR)".to_string());
        assert_eq!(join_pages(&native, Some(&ocr)), "first\nsecond (OCR)\nthird");
        // empty OCR text must NOT wipe the native page
        ocr.insert(1, "  ".to_string());
        assert_eq!(join_pages(&native, Some(&ocr)), "first\nsecond (OCR)\nthird");
        assert_eq!(join_pages(&native, None), "first\nthird");
    }

    #[test]
    fn index_maps_page_numbers() {
        let pages = vec![page(2, "b"), page(5, "e")];
        let m = index_ocr_pages(&pages);
        assert_eq!(m.get(&2).map(String::as_str), Some("b"));
        assert_eq!(m.get(&5).map(String::as_str), Some("e"));
    }
}

#[cfg(test)]
mod decide_tests {
    use super::*;

    fn long_ok() -> String {
        "This page contains plenty of readable body text with enough letters and digits to pass the threshold comfortably."
            .to_string()
    }

    fn page(n: usize, text: &str) -> PageText {
        PageText { page_number: n, text: text.to_string(), extract_failed: text.trim().is_empty() }
    }

    fn input<'a>(category: FileCategory, pages: &'a [PageText], text: &'a str, fallback: bool) -> DecisionInput<'a> {
        DecisionInput {
            category,
            ocr_fallback_enabled: fallback,
            image_ocr_enabled: true,
            pages,
            native_failed: false,
            native_text: text,
        }
    }

    #[test]
    fn image_routes_to_ocr_or_fails() {
        let mut i = input(FileCategory::Image, &[], "", true);
        assert_eq!(decide(&i), Decision::OcrImage);
        i.image_ocr_enabled = false;
        assert_eq!(decide(&i), Decision::Fail("image_ocr_disabled"));
    }

    #[test]
    fn clean_pdf_passes_through() {
        let pages = vec![page(1, &long_ok())];
        assert_eq!(
            decide(&input(FileCategory::Pdf, &pages, &long_ok(), true)),
            Decision::Done
        );
        // even without fallback a populated PDF is fine
        assert_eq!(
            decide(&input(FileCategory::Pdf, &pages, &long_ok(), false)),
            Decision::Done
        );
    }

    #[test]
    fn thin_pages_get_selective_ocr() {
        let pages = vec![page(1, &long_ok()), page(2, ""), page(3, "tiny")];
        let text = format!("{}\ntiny", long_ok());
        assert_eq!(
            decide(&input(FileCategory::Pdf, &pages, &text, true)),
            // 2 and 3 are adjacent — compact merges them into one run
            Decision::OcrCandidatePages(vec![PageRange { start: 2, end: 3 }])
        );
    }

    #[test]
    fn empty_pdf_without_fallback_is_a_clean_error() {
        assert_eq!(
            decide(&input(FileCategory::Pdf, &[], "", false)),
            Decision::Fail("pdf_no_extractable_text")
        );
    }

    #[test]
    fn failed_native_pdf_gets_full_ocr_attempt() {
        let mut i = input(FileCategory::Pdf, &[], "", true);
        i.native_failed = true;
        assert_eq!(decide(&i), Decision::OcrFile);
        i.ocr_fallback_enabled = false;
        assert_eq!(decide(&i), Decision::Fail("extract_failed"));
    }

    #[test]
    fn text_documents_never_fall_back() {
        let mut i = input(FileCategory::Word, &[], "hello body text", false);
        assert_eq!(decide(&i), Decision::Done);
        i.native_text = "   ";
        assert_eq!(decide(&i), Decision::Fail("extract_failed"));
        i.native_failed = true;
        assert_eq!(decide(&i), Decision::Fail("extract_failed"));
    }
}
