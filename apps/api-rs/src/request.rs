//! Query-string parsing and parameter validation shared by the JSON routes.
//!
//! Every invalid value is a `400` with a `text/plain` message naming the
//! parameter (`Invalid era id`, `Invalid offset`, …).

use std::collections::HashMap;

use crate::error::ApiError;
use crate::text;

/// JavaScript's `Number.MAX_SAFE_INTEGER` (2^53 − 1). Larger ids and
/// pagination values are rejected: browsers could not represent them exactly.
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Largest accepted `offset`; deeper pages are refused rather than clamped.
pub const MAX_OFFSET: i64 = 10_000;

/// Longest accepted search query, in Unicode scalar values after
/// whitespace normalization.
pub const MAX_QUERY_CHARS: usize = 100;

/// An id: `^[1-9][0-9]*$` (no sign, no leading zeros) and at most
/// [`MAX_SAFE_INTEGER`], else 400 `Invalid <label>`.
pub fn positive_integer(value: Option<&str>, label: &str) -> Result<i64, ApiError> {
    let invalid = || ApiError::bad_request(format!("Invalid {label}"));
    let value = value.ok_or_else(invalid)?;
    let well_formed = value.as_bytes().first().is_some_and(|first| *first != b'0')
        && value.bytes().all(|byte| byte.is_ascii_digit());
    if !well_formed {
        return Err(invalid());
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|parsed| *parsed <= MAX_SAFE_INTEGER)
        .ok_or_else(invalid)
}

fn digits(value: &str) -> Option<i64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|parsed| *parsed <= MAX_SAFE_INTEGER)
}

/// `limit`: missing or empty → `fallback`; at least 1 and clamped to
/// `maximum`; anything else → 400 `Invalid limit`.
pub fn limit_value(value: Option<&str>, fallback: i64, maximum: i64) -> Result<i64, ApiError> {
    match value {
        None | Some("") => Ok(fallback),
        Some(value) => digits(value)
            .filter(|limit| *limit >= 1)
            .map(|limit| limit.min(maximum))
            .ok_or_else(|| ApiError::bad_request("Invalid limit")),
    }
}

/// `offset`: missing or empty → 0; 0–[`MAX_OFFSET`]; anything else → 400
/// `Invalid offset`.
pub fn offset_value(value: Option<&str>) -> Result<i64, ApiError> {
    match value {
        None | Some("") => Ok(0),
        Some(value) => digits(value)
            .filter(|offset| *offset <= MAX_OFFSET)
            .ok_or_else(|| ApiError::bad_request("Invalid offset")),
    }
}

/// Order of a song list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SongSort {
    /// Sheet order (`catalog`; `id` is an alias kept for old links).
    Catalog,
    /// Category: best-of, special, grails, wanted, unmarked, worst-of, AI.
    Category,
    LeakNewest,
    LeakOldest,
    FileNewest,
    /// Natural title order (`[V3] < [V9] < [V39]`).
    Name,
}

/// `sort`: missing or blank → catalog order, unknown → 400 `Invalid sort`.
pub fn sort_value(value: Option<&str>) -> Result<SongSort, ApiError> {
    let value = value.map(str::trim).unwrap_or_default();
    Ok(match value {
        "" | "catalog" | "id" => SongSort::Catalog,
        "category" => SongSort::Category,
        "leak-newest" => SongSort::LeakNewest,
        "leak-oldest" => SongSort::LeakOldest,
        "file-newest" => SongSort::FileNewest,
        "name" => SongSort::Name,
        _ => return Err(ApiError::bad_request("Invalid sort")),
    })
}

/// `q`: trimmed with inner whitespace collapsed; `None` when blank; longer
/// than [`MAX_QUERY_CHARS`] → 400 `Search query is too long`.
pub fn search_query(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let query = text::collapse_whitespace(value.unwrap_or_default());
    if query.is_empty() {
        return Ok(None);
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(ApiError::bad_request("Search query is too long"));
    }
    Ok(Some(query))
}

/// The parameters `/songs` reads (plain list and search/filter mode).
pub const SONG_LIST_PARAMS: &[&str] = &[
    "q",
    "era",
    "eraFrom",
    "eraTo",
    "quality",
    "availability",
    "playable",
    "category",
    "sort",
    "limit",
    "offset",
];

/// The parameters `/eras/{id}/songs` reads; `/songs`-only filters such as
/// `era` or `eraFrom` are ignored there like any unknown parameter.
pub const ERA_SONG_PARAMS: &[&str] = &["q", "category", "sort", "limit", "offset"];

/// The parameters the JSON routes read, with the label of their `400`.
const PARAM_LABELS: [(&str, &str); 11] = [
    ("q", "search query"),
    ("era", "era filter"),
    ("eraFrom", "starting era filter"),
    ("eraTo", "ending era filter"),
    ("quality", "quality filter"),
    ("availability", "availability filter"),
    ("playable", "playable filter"),
    ("category", "category filter"),
    ("sort", "sort"),
    ("limit", "limit"),
    ("offset", "offset"),
];

/// A parameter value that is not valid UTF-8 once decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Undecodable;

/// A decoded `application/x-www-form-urlencoded` query string, read the same
/// way by every route: the last occurrence of a repeated parameter wins,
/// values are trimmed, and a blank value counts as absent.
#[derive(Debug, Clone, Default)]
pub struct Query {
    /// `None` for a value that is not valid UTF-8 once decoded.
    values: HashMap<String, Option<String>>,
}

impl Query {
    pub fn parse(raw: Option<&str>) -> Self {
        let mut values = HashMap::new();
        for pair in raw.unwrap_or_default().split('&') {
            if pair.is_empty() {
                continue;
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let Ok(name) = String::from_utf8(percent_decode(name)) else {
                continue;
            };
            values.insert(name, String::from_utf8(percent_decode(value)).ok());
        }
        Self { values }
    }

    /// The trimmed value of `name`: `None` when missing or blank.
    pub fn value(&self, name: &str) -> Result<Option<&str>, Undecodable> {
        match self.values.get(name) {
            None => Ok(None),
            Some(None) => Err(Undecodable),
            Some(Some(value)) => Ok(Some(text::trim(value)).filter(|value| !value.is_empty())),
        }
    }
}

/// [`Query::parse`] for a JSON route that reads the parameters `used`: one of
/// them whose value is not valid UTF-8 once decoded is a 400 naming it
/// (`Invalid search query`); other parameters are never looked at.
pub fn parse_query(raw: Option<&str>, used: &[&str]) -> Result<Query, ApiError> {
    let query = Query::parse(raw);
    for (name, label) in PARAM_LABELS {
        if used.contains(&name) && query.value(name).is_err() {
            return Err(ApiError::bad_request(format!("Invalid {label}")));
        }
    }
    Ok(query)
}

/// Form decoding: `+` is a space, `%XY` a byte; a `%` without two hex digits
/// is kept as is.
fn percent_decode(input: &str) -> Vec<u8> {
    let hex = |byte: Option<&u8>| byte.and_then(|byte| (*byte as char).to_digit(16));
    let bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let escaped = (byte == b'%')
            .then(|| hex(bytes.get(index + 1)).zip(hex(bytes.get(index + 2))))
            .flatten();
        if let Some((high, low)) = escaped {
            decoded.push((high * 16 + low) as u8);
            index += 3;
        } else {
            decoded.push(if byte == b'+' { b' ' } else { byte });
            index += 1;
        }
    }
    decoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(error: ApiError) -> String {
        error.to_string()
    }

    #[test]
    fn ids_are_strict() {
        assert_eq!(positive_integer(Some("12"), "era id").unwrap(), 12);
        assert_eq!(
            positive_integer(Some("9007199254740991"), "era id").unwrap(),
            MAX_SAFE_INTEGER
        );
        for bad in [
            "0",
            "01",
            "007",
            "-1",
            "+1",
            "1.5",
            " 1",
            "1 ",
            "",
            "abc",
            "9007199254740992",
            "99999999999999999999",
        ] {
            let error = positive_integer(Some(bad), "era id").unwrap_err();
            assert_eq!(message(error), "400 Invalid era id", "{bad:?}");
        }
        assert!(positive_integer(None, "song id").is_err());
    }

    #[test]
    fn limits_clamp_and_offsets_are_bounded() {
        assert_eq!(limit_value(None, 100, 500).unwrap(), 100);
        assert_eq!(limit_value(Some(""), 100, 500).unwrap(), 100);
        assert_eq!(limit_value(Some("7"), 100, 500).unwrap(), 7);
        assert_eq!(limit_value(Some("9999"), 100, 500).unwrap(), 500);
        assert_eq!(limit_value(Some("9007199254740991"), 50, 50).unwrap(), 50);
        for bad in ["0", "-1", "abc", "1.5", "9007199254740992"] {
            assert_eq!(
                message(limit_value(Some(bad), 100, 500).unwrap_err()),
                "400 Invalid limit",
                "{bad:?}"
            );
        }

        assert_eq!(offset_value(None).unwrap(), 0);
        assert_eq!(offset_value(Some("0")).unwrap(), 0);
        assert_eq!(offset_value(Some("10000")).unwrap(), 10_000);
        for bad in ["10001", "-1", "abc", "9007199254740992"] {
            assert_eq!(
                message(offset_value(Some(bad)).unwrap_err()),
                "400 Invalid offset",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn sorts_default_to_catalog_order() {
        assert_eq!(sort_value(None).unwrap(), SongSort::Catalog);
        assert_eq!(sort_value(Some("  ")).unwrap(), SongSort::Catalog);
        assert_eq!(sort_value(Some("id")).unwrap(), SongSort::Catalog);
        assert_eq!(sort_value(Some("catalog")).unwrap(), SongSort::Catalog);
        assert_eq!(sort_value(Some(" name ")).unwrap(), SongSort::Name);
        assert_eq!(sort_value(Some("category")).unwrap(), SongSort::Category);
        assert_eq!(
            sort_value(Some("leak-oldest")).unwrap(),
            SongSort::LeakOldest
        );
        assert!(sort_value(Some("ID")).is_err());
        assert!(sort_value(Some("bogus")).is_err());
    }

    #[test]
    fn search_queries_are_normalized_and_bounded() {
        assert_eq!(search_query(None).unwrap(), None);
        assert_eq!(search_query(Some(" \t ")).unwrap(), None);
        assert_eq!(
            search_query(Some("  new \n body ")).unwrap().as_deref(),
            Some("new body")
        );
        let hundred = "é".repeat(100);
        assert_eq!(
            search_query(Some(&hundred)).unwrap().as_deref(),
            Some(hundred.as_str())
        );
        let spaced = format!("  {}  ", "x ".repeat(50));
        assert_eq!(
            search_query(Some(&spaced))
                .unwrap()
                .unwrap()
                .chars()
                .count(),
            99
        );
        assert_eq!(
            message(search_query(Some(&"x".repeat(101))).unwrap_err()),
            "400 Search query is too long"
        );
    }

    fn value<'a>(query: &'a Query, name: &str) -> Option<&'a str> {
        query.value(name).expect("decodable")
    }

    #[test]
    fn query_strings_decode_strictly() {
        let parse_query = |raw| parse_query(raw, SONG_LIST_PARAMS);
        let params = parse_query(Some("q=can%E2%80%99t+tell&era=31&empty=&flag&q2=%ZZ%4")).unwrap();
        assert_eq!(value(&params, "q"), Some("can’t tell"));
        assert_eq!(value(&params, "era"), Some("31"));
        assert_eq!(value(&params, "empty"), None);
        assert_eq!(value(&params, "flag"), None);
        assert_eq!(value(&params, "missing"), None);
        assert_eq!(value(&params, "q2"), Some("%ZZ%4"));
        assert_eq!(value(&parse_query(None).unwrap(), "q"), None);

        assert_eq!(
            message(parse_query(Some("q=%FF")).unwrap_err()),
            "400 Invalid search query"
        );
        assert_eq!(
            message(parse_query(Some("limit=5&eraFrom=%C3")).unwrap_err()),
            "400 Invalid starting era filter"
        );
        // Unknown parameters that don't decode are ignored.
        let params = parse_query(Some("utm=%FF&q=x")).unwrap();
        assert_eq!(value(&params, "q"), Some("x"));
        assert_eq!(params.value("utm"), Err(Undecodable));
    }

    /// A route only checks the parameters it reads: the era song list
    /// ignores `/songs`-only filters, however they are encoded.
    #[test]
    fn routes_only_check_the_parameters_they_read() {
        for name in SONG_LIST_PARAMS {
            let raw = format!("{name}=%FF");
            let label = PARAM_LABELS
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, label)| *label)
                .unwrap();
            assert_eq!(
                message(parse_query(Some(&raw), SONG_LIST_PARAMS).unwrap_err()),
                format!("400 Invalid {label}"),
                "{name}"
            );
            let era_list = parse_query(Some(&raw), ERA_SONG_PARAMS);
            assert_eq!(era_list.is_err(), ERA_SONG_PARAMS.contains(name), "{name}");
        }
        let params = parse_query(
            Some("era=%FF&eraFrom=%C3&eraTo=x&quality=%FE&availability=%FF&playable=%FF&limit=5"),
            ERA_SONG_PARAMS,
        )
        .unwrap();
        assert_eq!(value(&params, "limit"), Some("5"));
    }

    /// Every route resolves a parameter the same way: the last occurrence
    /// wins, values are trimmed and blank ones count as absent.
    #[test]
    fn repeated_and_blank_parameters_resolve_one_way() {
        let query = Query::parse(Some("sort=a&sort=name&era=+31+&category=%20&quality=%09"));
        assert_eq!(value(&query, "sort"), Some("name"));
        assert_eq!(value(&query, "era"), Some("31"));
        assert_eq!(value(&query, "category"), None);
        assert_eq!(value(&query, "quality"), None);
        // A blank last occurrence overrides an earlier value.
        assert_eq!(value(&Query::parse(Some("sort=name&sort=")), "sort"), None);
        // So does an undecodable one, and a decodable one repairs it.
        assert_eq!(
            Query::parse(Some("quality=64&quality=%FF")).value("quality"),
            Err(Undecodable)
        );
        assert_eq!(
            value(&Query::parse(Some("quality=%FF&quality=64")), "quality"),
            Some("64")
        );
        assert!(parse_query(Some("q=%FF&q=ok"), SONG_LIST_PARAMS).is_ok());
        assert!(parse_query(Some("q=ok&q=%FF"), SONG_LIST_PARAMS).is_err());
    }
}
