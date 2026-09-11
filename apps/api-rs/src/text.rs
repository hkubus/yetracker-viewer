//! String helpers that reproduce JavaScript's `\s` semantics and
//! `String#length` (UTF-16 code units) rather than Rust's byte/codepoint
//! notions, so ports of the Node code stay byte-identical on edge inputs.

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

/// `String.prototype.trimStart()`.
pub fn trim_start(value: &str) -> &str {
    value.trim_start_matches(is_js_whitespace)
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

/// `value.trim().replace(/\s+/g, ' ').toLowerCase()`.
pub fn normalize(value: &str) -> String {
    collapse_whitespace(value).to_lowercase()
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
}
