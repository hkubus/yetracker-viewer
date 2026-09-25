//! Query-parameter validators, ported from `util/request.ts`.

use crate::error::ApiError;

fn is_ascii_digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

/// JavaScript's `Number.MAX_SAFE_INTEGER`. The TS validators use
/// `Number.isSafeInteger`, so anything above this must be rejected.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// `positiveInteger`: non-empty ASCII digits, safe integer, at least 1.
pub fn positive_integer(value: Option<&str>, label: &str) -> Result<i64, ApiError> {
    let invalid = || ApiError::bad_request(format!("Invalid {label}"));
    let value = value.ok_or_else(invalid)?;
    if !is_ascii_digits(value) {
        return Err(invalid());
    }
    let parsed: i64 = value.parse().map_err(|_| invalid())?;
    if parsed < 1 || parsed > MAX_SAFE_INTEGER {
        return Err(invalid());
    }
    Ok(parsed)
}

/// `paginationValue`: missing/empty → fallback, invalid → 400, otherwise
/// clamped to `maximum`.
pub fn pagination_value(
    value: Option<&str>,
    fallback: i64,
    maximum: i64,
    label: &str,
) -> Result<i64, ApiError> {
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
    if parsed < minimum || parsed > MAX_SAFE_INTEGER {
        return Err(invalid());
    }
    Ok(parsed.min(maximum))
}

/// Sort keys accepted by the song list endpoints. `id` is the import order
/// and stays the default so callers that never pass `sort` keep the historical
/// ordering.
pub const SORT_KEYS: [&str; 5] = ["id", "leak-newest", "leak-oldest", "file-newest", "name"];

/// `sortValue`: missing/empty → `id`, unknown → 400 `Invalid sort`.
pub fn sort_value(value: Option<&str>) -> Result<&'static str, ApiError> {
    let value = match value {
        None => return Ok("id"),
        Some(value) => value.trim(),
    };
    if value.is_empty() {
        return Ok("id");
    }
    SORT_KEYS
        .iter()
        .find(|key| **key == value)
        .copied()
        .ok_or_else(|| ApiError::bad_request("Invalid sort"))
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
    fn rejects_values_above_js_safe_integer_ceiling() {
        assert_eq!(
            positive_integer(Some("9007199254740991"), "era id").unwrap(),
            MAX_SAFE_INTEGER
        );
        assert!(positive_integer(Some("9007199254740992"), "era id").is_err());
        assert!(positive_integer(Some("9223372036854775807"), "era id").is_err());
        assert!(pagination_value(Some("9007199254740992"), 100, 500, "limit").is_err());
        assert!(pagination_value(Some("9007199254740992"), 0, 10_000, "offset").is_err());
        assert_eq!(
            pagination_value(Some("9007199254740991"), 100, 500, "limit").unwrap(),
            500
        );
    }

    #[test]
    fn pagination_clamps_and_rejects() {
        assert_eq!(pagination_value(None, 100, 500, "limit").unwrap(), 100);
        assert_eq!(pagination_value(Some(""), 100, 500, "limit").unwrap(), 100);
        assert_eq!(
            pagination_value(Some("9999"), 100, 500, "limit").unwrap(),
            500
        );
        assert_eq!(pagination_value(Some("0"), 0, 10_000, "offset").unwrap(), 0);
        assert!(pagination_value(Some("0"), 100, 500, "limit").is_err());
        assert!(pagination_value(Some("abc"), 100, 500, "limit").is_err());
    }

    #[test]
    fn sort_defaults_and_rejects_unknown_keys() {
        assert_eq!(sort_value(None).unwrap(), "id");
        assert_eq!(sort_value(Some("")).unwrap(), "id");
        assert_eq!(sort_value(Some("  ")).unwrap(), "id");
        assert_eq!(sort_value(Some("leak-newest")).unwrap(), "leak-newest");
        assert_eq!(sort_value(Some(" name ")).unwrap(), "name");
        assert!(sort_value(Some("bogus")).is_err());
        assert!(sort_value(Some("ID")).is_err());
        assert!(SORT_KEYS.contains(&"id"));
    }

    #[test]
    fn whitespace_and_like_escaping() {
        assert_eq!(normalize_query("  A \t B\nC "), "a b c");
        assert_eq!(escape_like_pattern("50%_\\x"), "50\\%\\_\\\\x");
    }
}
