//! Query-parameter validators, ported from `util/request.ts`.

use crate::error::ApiError;

fn is_ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// `positiveInteger`: non-empty ASCII digits, safe integer, at least 1.
pub fn positive_integer(value: Option<&str>, label: &str) -> Result<i64, ApiError> {
    let invalid = || ApiError::bad_request(format!("Invalid {label}"));
    let value = value.ok_or_else(invalid)?;
    if !is_ascii_digits(value) {
        return Err(invalid());
    }
    let parsed: i64 = value.parse().map_err(|_| invalid())?;
    if parsed < 1 {
        return Err(invalid());
    }
    Ok(parsed)
}

/// `paginationValue`: missing/empty → fallback, invalid → 400, otherwise
/// clamped to `maximum`.
pub fn pagination_value(value: Option<&str>, fallback: i64, maximum: i64, label: &str) -> Result<i64, ApiError> {
    let value = match value {
        None | Some("") => return Ok(fallback),
        Some(value) => value,
    };
    let invalid = || ApiError::bad_request(format!("Invalid {label}"));
    if !is_ascii_digits(value) {
        return Err(invalid());
    }
    let parsed: i64 = value.parse().map_err(|_| invalid())?;
    let minimum = if label == "limit" { 1 } else { 0 };
    if parsed < minimum {
        return Err(invalid());
    }
    Ok(parsed.min(maximum))
}

pub use crate::text::{collapse_whitespace, normalize as normalize_query};

/// Escapes `\`, `%` and `_` for use inside a `LIKE ... ESCAPE '\'` pattern.
pub fn escape_like_pattern(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_integer_matches_node_rules() {
        assert_eq!(positive_integer(Some("12"), "era id").unwrap(), 12);
        assert!(positive_integer(Some("0"), "era id").is_err());
        assert!(positive_integer(Some("-1"), "era id").is_err());
        assert!(positive_integer(Some("1.5"), "era id").is_err());
        assert!(positive_integer(Some(""), "era id").is_err());
        assert!(positive_integer(None, "era id").is_err());
        assert!(positive_integer(Some("99999999999999999999"), "era id").is_err());
    }

    #[test]
    fn pagination_clamps_and_rejects() {
        assert_eq!(pagination_value(None, 100, 500, "limit").unwrap(), 100);
        assert_eq!(pagination_value(Some(""), 100, 500, "limit").unwrap(), 100);
        assert_eq!(pagination_value(Some("9999"), 100, 500, "limit").unwrap(), 500);
        assert_eq!(pagination_value(Some("0"), 0, 10_000, "offset").unwrap(), 0);
        assert!(pagination_value(Some("0"), 100, 500, "limit").is_err());
        assert!(pagination_value(Some("abc"), 100, 500, "limit").is_err());
    }

    #[test]
    fn whitespace_and_like_escaping() {
        assert_eq!(normalize_query("  A \t B\nC "), "a b c");
        assert_eq!(escape_like_pattern("50%_\\x"), "50\\%\\_\\\\x");
    }
}
