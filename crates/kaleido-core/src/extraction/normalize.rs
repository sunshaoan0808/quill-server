//! Text normalization ported from DEEIX-Chat `infra/extraction/builtin`.
//!
//! Two levels, mirroring the Go originals:
//! - [`normalize_document_text`] for word/excel/text engines
//! - [`normalize_extracted_pdf_text`] for pdf output (NFKC + single-rune
//!   run merging + spacing tightening)

use unicode_normalization::UnicodeNormalization;

/// Go `normalizeDocumentText`: strip NULs, normalize newlines, drop
/// blank lines, trim every line.
pub fn normalize_document_text(raw: &str) -> String {
    let mut value = raw.replace("\r\n", "\n");
    if value.contains('\r') {
        value = value.replace('\r', "\n");
    }
    if value.contains('\x00') {
        value = value.replace('\x00', "");
    }

    let mut cleaned = Vec::new();
    for line in value.split('\n') {
        let line = line.trim();
        if !line.is_empty() {
            cleaned.push(line);
        }
    }
    cleaned.join("\n")
}

/// Go `normalizeExtractedPDFText`.
pub fn normalize_extracted_pdf_text(raw: &str) -> String {
    let mut value = raw.replace('\x00', "");
    let nfkc: String = value.nfkc().collect();
    value = nfkc;
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }

    let mut normalized = Vec::new();
    for line in value.split('\n') {
        // Go strings.Fields + strings.Join: collapse whitespace runs.
        let mut line: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            continue;
        }
        line = merge_spaced_single_rune_tokens(&line);
        line = tighten_extracted_spacing(&line);
        let line = line.trim();
        if !line.is_empty() {
            normalized.push(line.to_string());
        }
    }
    normalized.join("\n")
}

/// Go `mergeSpacedSingleRuneTokens`: "H e l l o" -> "Hello".
fn merge_spaced_single_rune_tokens(line: &str) -> String {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() <= 1 {
        return line.to_string();
    }

    let mut merged: Vec<String> = Vec::with_capacity(fields.len());
    let mut i = 0;
    while i < fields.len() {
        if !is_single_rune_joinable(fields[i]) {
            merged.push(fields[i].to_string());
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < fields.len() && is_single_rune_joinable(fields[j]) {
            j += 1;
        }
        let run = &fields[i..j];
        if should_merge_single_rune_run(run) {
            merged.push(run.concat());
        } else {
            merged.extend(run.iter().map(|s| s.to_string()));
        }
        i = j;
    }
    merged.join(" ")
}

/// Go `isSingleRuneJoinable`.
fn is_single_rune_joinable(token: &str) -> bool {
    let mut chars = token.chars();
    let (Some(first), None) = (chars.next(), chars.next()) else {
        return false;
    };
    if first.is_alphabetic() || first.is_numeric() {
        return true;
    }
    // strings.ContainsRune(`._:/\-+%#@&()[]{}'"`, r)
    matches!(first, '.' | '_' | ':' | '/' | '\\' | '-' | '+' | '%' | '#' | '@' | '&'
        | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"')
}

/// Go `shouldMergeSingleRuneRun`: >=3 joinable always, 2 only when
/// connector/digit-digit/upper-upper.
fn should_merge_single_rune_run(run: &[&str]) -> bool {
    if run.len() >= 3 {
        return true;
    }
    if run.len() != 2 {
        return false;
    }
    let a = run[0].chars().next().unwrap_or('\0');
    let b = run[1].chars().next().unwrap_or('\0');
    if is_word_connector_rune(a) || is_word_connector_rune(b) {
        return true;
    }
    if a.is_numeric() && b.is_numeric() {
        return true;
    }
    if a.is_uppercase() && b.is_uppercase() {
        return true;
    }
    false
}

/// Go `tightenExtractedSpacing`.
fn tighten_extracted_spacing(line: &str) -> String {
    let runes: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());

    for i in 0..runes.len() {
        let r = runes[i];
        if !r.is_whitespace() {
            out.push(r);
            continue;
        }
        let (Some(prev), Some(next)) = (previous_non_space(&runes, i), next_non_space(&runes, i))
        else {
            continue;
        };
        if should_tighten_spacing(prev, next) {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
    }
    out
}

fn previous_non_space(runes: &[char], idx: usize) -> Option<char> {
    runes[..idx].iter().rev().copied().find(|c| !c.is_whitespace())
}

fn next_non_space(runes: &[char], idx: usize) -> Option<char> {
    runes[idx + 1..].iter().copied().find(|c| !c.is_whitespace())
}

/// Go `shouldTightenSpacing`.
fn should_tighten_spacing(prev: char, next: char) -> bool {
    if is_cjk_like_rune(prev)
        && (is_cjk_like_rune(next) || next.is_numeric() || is_tight_punctuation(next))
    {
        return true;
    }
    if is_cjk_like_rune(next) && (prev.is_numeric() || is_tight_punctuation(prev)) {
        return true;
    }
    if prev.is_numeric() && next.is_numeric() {
        return false;
    }
    if is_word_connector_rune(prev) || is_word_connector_rune(next) {
        return true;
    }
    if is_opening_punctuation(prev) || is_closing_punctuation(next) {
        return true;
    }
    false
}

/// Go `isCJKLikeRune`: Han / Hiragana / Katakana / Hangul script tables.
fn is_cjk_like_rune(r: char) -> bool {
    let cp = r as u32;
    matches!(cp,
        // Han
        0x3400..=0x4DBF
        | 0x4E00..=0x9FFF
        | 0xF900..=0xFAFF
        | 0x20000..=0x2A6DF
        | 0x2A700..=0x2B73F
        | 0x2B740..=0x2B81F
        | 0x2B820..=0x2CEAF
        | 0x2CEB0..=0x2EBEF
        | 0x2F800..=0x2FA1F
        // Hiragana
        | 0x3040..=0x309F
        // Katakana (incl. extended)
        | 0x30A0..=0x30FF
        | 0x31F0..=0x31FF
        | 0x32D0..=0x32FE
        | 0x3300..=0x3357
        // Hangul syllables + jamo
        | 0x1100..=0x11FF
        | 0x3130..=0x318F
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7A3
        | 0xD7B0..=0xD7FF
    )
}

/// Go `isWordConnectorRune`.
fn is_word_connector_rune(r: char) -> bool {
    matches!(r, '.' | '_' | ':' | '/' | '\\' | '-' | '+' | '%' | '#' | '@' | '&')
}

/// Go `isOpeningPunctuation`.
fn is_opening_punctuation(r: char) -> bool {
    matches!(r, '(' | '[' | '{' | '"' | '\'' | '\u{201C}' | '\u{2018}' | '《' | '〈' | '「' | '『' | '【')
}

/// Go `isClosingPunctuation`.
fn is_closing_punctuation(r: char) -> bool {
    matches!(r, '.' | ',' | ':' | ';' | '!' | '?' | '%' | ')' | ']' | '}' | '"' | '\''
        | '\u{201D}' | '\u{2019}' | '》' | '〉' | '」' | '』' | '】'
        | '、' | '，' | '。' | '：' | '；' | '！' | '？')
}

/// Go `isTightPunctuation`.
fn is_tight_punctuation(r: char) -> bool {
    is_opening_punctuation(r) || is_closing_punctuation(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_text_drops_blanks_and_trims() {
        let raw = " hello \r\n\r\n world \r\n\x00 tail ";
        assert_eq!(normalize_document_text(raw), "hello\nworld\ntail");
    }

    #[test]
    fn pdf_text_merges_spaced_single_rune_runs() {
        // >=3 joinable runes merge unconditionally.
        assert_eq!(normalize_extracted_pdf_text("H e l l o"), "Hello");
        // multi-rune tokens are never joinable, spacing survives.
        assert_eq!(
            normalize_extracted_pdf_text("Hello  World"),
            "Hello World"
        );
    }

    #[test]
    fn pdf_text_keeps_two_token_runs_without_connector() {
        // "A B": both joinable but no connector/digit/upper pair rule
        // triggers -> kept apart.
        assert_eq!(normalize_extracted_pdf_text("x y"), "x y");
    }

    #[test]
    fn pdf_text_merges_two_digit_run() {
        assert_eq!(normalize_extracted_pdf_text("1 2 3"), "123");
    }

    #[test]
    fn pdf_text_merges_connector_pair() {
        assert_eq!(normalize_extracted_pdf_text("2 0 2 4"), "2024");
        assert_eq!(normalize_extracted_pdf_text("a - b"), "a-b");
    }

    #[test]
    fn pdf_text_tightens_cjk_spacing() {
        // NFKC folds the fullwidth comma to ASCII, same as Go norm.NFKC.
        assert_eq!(normalize_extracted_pdf_text("你 好， 世 界"), "你好,世界");
    }

    #[test]
    fn pdf_text_keeps_latin_word_spacing() {
        assert_eq!(normalize_extracted_pdf_text("hello   world"), "hello world");
    }

    #[test]
    fn pdf_text_nfkc_normalizes_fullwidth() {
        // Fullwidth Latin -> ASCII under NFKC.
        assert_eq!(normalize_extracted_pdf_text("ＡＢ Ｃ"), "AB C");
    }

    #[test]
    fn pdf_text_drops_empty_lines() {
        assert_eq!(normalize_extracted_pdf_text("a\n\n\nb\n"), "a\nb");
    }

    #[test]
    fn cjk_range_classification() {
        assert!(is_cjk_like_rune('中'));
        assert!(is_cjk_like_rune('あ'));
        assert!(is_cjk_like_rune('한'));
        assert!(!is_cjk_like_rune('a'));
        assert!(!is_cjk_like_rune('1'));
    }
}
