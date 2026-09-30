//! String helpers that reproduce JavaScript's `\s` semantics and
//! `String#length` (UTF-16 code units) rather than Rust's byte/codepoint
//! notions, so ports of the Node code stay byte-identical on edge inputs, plus
//! the cell-text cleanup the importer applies to sheet values.

use std::borrow::Cow;

/// Zero-width space, non-joiner and joiner, the word joiner and the BOM.
pub fn is_zero_width(character: char) -> bool {
    matches!(character, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}')
}

/// `value` without zero-width characters (borrowed when there are none).
pub fn strip_zero_width(value: &str) -> Cow<'_, str> {
    if value.chars().any(is_zero_width) {
        Cow::Owned(value.chars().filter(|&c| !is_zero_width(c)).collect())
    } else {
        Cow::Borrowed(value)
    }
}

/// Single-line cell text: zero-width characters removed, whitespace runs
/// (line breaks included) collapsed to one space, trimmed.
pub fn clean_line(value: &str) -> String {
    collapse_whitespace(&strip_zero_width(value))
}

/// Multi-line cell text: every line cleaned like [`clean_line`], blank lines
/// at either end dropped and runs of blank lines inside collapsed to one (the
/// sheet uses a single blank line as a paragraph break).
pub fn clean_multiline(value: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in strip_zero_width(value).split('\n') {
        let line = collapse_whitespace(line);
        if line.is_empty() && lines.last().is_none_or(|last| last.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// The first line of `value` (all of it when there is no line break).
pub fn first_line(value: &str) -> &str {
    value.split('\n').next().unwrap_or_default()
}

/// JavaScript `\s` (whitespace + line terminators, including U+FEFF).
pub fn is_js_whitespace(character: char) -> bool {
    matches!(character,
        '\u{0009}'..='\u{000D}'
        | '\u{0020}'
        | '\u{00A0}'
        | '\u{1680}'
        | '\u{2000}'..='\u{200A}'
        | '\u{2028}'
        | '\u{2029}'
        | '\u{202F}'
        | '\u{205F}'
        | '\u{3000}'
        | '\u{FEFF}')
}

/// `String.prototype.trim()`.
pub fn trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

/// `value.trim().replace(/\s+/g, ' ')`.
pub fn collapse_whitespace(value: &str) -> String {
    let trimmed = trim(value);
    let mut collapsed = String::with_capacity(trimmed.len());
    let mut in_run = false;
    for character in trimmed.chars() {
        if is_js_whitespace(character) {
            in_run = true;
            continue;
        }
        if in_run && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        in_run = false;
        collapsed.push(character);
    }
    collapsed
}

/// `String.prototype.length` — UTF-16 code units.
pub fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_matches_js_whitespace_rules() {
        assert_eq!(collapse_whitespace("  a \t b\n c  "), "a b c");
        assert_eq!(collapse_whitespace("\u{00a0}a\u{3000}b\u{feff}"), "a b");
        assert_eq!(collapse_whitespace(""), "");
        assert_eq!(collapse_whitespace("   "), "");
    }

    #[test]
    fn utf16_len_counts_surrogates() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("🗑️"), 3);
    }

    #[test]
    fn zero_width_characters_are_removed_not_turned_into_spaces() {
        assert_eq!(
            strip_zero_width("RONNY J & \u{200B}shadyboy"),
            "RONNY J & shadyboy"
        );
        assert!(matches!(strip_zero_width("plain"), Cow::Borrowed("plain")));
        assert_eq!(clean_line("a\u{200C}b\u{200D}c\u{2060}d\u{FEFF}e"), "abcde");
        assert_eq!(clean_line("  two\nlines  "), "two lines");
    }

    #[test]
    fn multiline_cleanup_keeps_line_breaks() {
        assert_eq!(
            clean_multiline("\n  Title  [V2] \n(feat.   X)\r\n\n"),
            "Title [V2]\n(feat. X)"
        );
        assert_eq!(
            clean_multiline("para one\n\n\n \npara\u{200B} two"),
            "para one\n\npara two"
        );
        assert_eq!(clean_multiline(" \n \n"), "");
        assert_eq!(first_line("a\nb"), "a");
        assert_eq!(first_line(""), "");
    }
}
