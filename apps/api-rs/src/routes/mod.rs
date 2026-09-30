//! Route table, the path/query extractors shared by the routes, and the JSON
//! response helpers.

use axum::body::Body;
use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use serde_json::Value;

use crate::error::ApiError;
use crate::request::{Query, parse_query, positive_integer};
use crate::state::SharedState;

pub mod eras;
pub mod media;
pub mod songs;
pub mod status;

pub const JSON_CACHE: &str = "public, max-age=60, s-maxage=300, stale-while-revalidate=600";
pub const NO_STORE: &str = "no-store";

/// Methods every route answers (HEAD and the CORS preflight included).
pub const ALLOWED_METHODS: &str = "GET, HEAD, OPTIONS";

pub fn json_response(value: Value) -> Response {
    let body = serde_json::to_vec(&value).expect("JSON value is always serialisable");
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

pub fn json_cached(value: Value) -> Response {
    let mut response = json_response(value);
    set_header(&mut response, header::CACHE_CONTROL, JSON_CACHE);
    response
}

/// [`json_cached`] plus `X-Total-Count`: the number of items matching the
/// request on every page, also past the end.
pub fn json_list(value: Value, total: i64) -> Response {
    let mut response = json_cached(value);
    response.headers_mut().insert(
        HeaderName::from_static("x-total-count"),
        HeaderValue::from(total),
    );
    response
}

/// A JSON error body (`{"error": …}`) that is never cached.
fn json_error(status: StatusCode, message: &str) -> Response {
    let mut response = json_response(serde_json::json!({ "error": message }));
    *response.status_mut() = status;
    set_header(&mut response, header::CACHE_CONTROL, NO_STORE);
    response
}

pub fn empty(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

/// Serialises a float the way `JSON.stringify` does: whole numbers lose their
/// `.0` suffix (Rust's serde_json would otherwise emit `2.0` where Node emits
/// `2`), and non-finite values become `null`.
pub fn js_float(value: f64) -> Value {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 9_007_199_254_740_992.0 {
        Value::Number(serde_json::Number::from(value as i64))
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

pub fn set_header(response: &mut Response, name: header::HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        response.headers_mut().insert(name, value);
    }
}

/// `{id}` of an `/eras/{id}…` route: 400 `Invalid era id` unless it is a
/// canonical positive integer (see [`positive_integer`]), also when the
/// segment isn't valid UTF-8.
#[derive(Debug, Clone, Copy)]
pub struct EraId(pub i64);

/// `{id}` of a `/songs/{id}…` route: 400 `Invalid song id` otherwise.
#[derive(Debug, Clone, Copy)]
pub struct SongId(pub i64);

async fn path_id<S: Send + Sync>(
    parts: &mut Parts,
    state: &S,
    label: &str,
) -> Result<i64, ApiError> {
    let raw = Path::<String>::from_request_parts(parts, state).await.ok();
    positive_integer(raw.as_ref().map(|Path(id)| id.as_str()), label)
}

impl<S: Send + Sync> FromRequestParts<S> for EraId {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        path_id(parts, state, "era id").await.map(EraId)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for SongId {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        path_id(parts, state, "song id").await.map(SongId)
    }
}

/// The decoded query string of a JSON route.
#[derive(Debug, Clone, Default)]
pub struct Params(pub Query);

impl Params {
    /// Parses the query string of a route that reads the parameters `used`
    /// (e.g. [`crate::request::SONG_LIST_PARAMS`]): one of them whose value
    /// isn't valid UTF-8 is a 400 naming it, the others are never looked at
    /// (see [`parse_query`]).
    pub fn parse(raw: Option<&str>, used: &[&str]) -> Result<Self, ApiError> {
        parse_query(raw, used).map(Params)
    }

    /// The trimmed value; `None` when missing or blank (see [`Query`]).
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.value(name).ok().flatten()
    }
}

pub fn router(state: SharedState) -> axum::Router {
    use axum::routing::get;

    axum::Router::new()
        .route("/health", get(status::health))
        .route("/hello", get(hello))
        .route("/status", get(status::status))
        .route("/eras", get(eras::list_eras))
        .route("/eras/{id}", get(eras::get_era))
        .route("/eras/{id}/songs", get(eras::list_era_songs))
        .route("/eras/{id}/cover", get(media::get_era_cover))
        .route("/songs", get(songs::list_songs))
        .route("/songs/{id}", get(songs::get_song))
        .route("/songs/{id}/stream", get(media::stream_song))
        .route("/songs/{id}/download", get(media::download_song))
        .route("/songs/{id}/duration", get(media::get_song_duration))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
}

async fn hello() -> Response {
    json_response(serde_json::json!({ "hello": "world" }))
}

async fn not_found() -> Response {
    json_error(StatusCode::NOT_FOUND, "Not found")
}

async fn method_not_allowed() -> Response {
    let mut response = json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
    set_header(&mut response, header::ALLOW, ALLOWED_METHODS);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parsing_maps_bad_encodings_to_the_parameter() {
        use crate::request::{ERA_SONG_PARAMS, SONG_LIST_PARAMS};

        let params = Params::parse(
            Some("q=caf%C3%A9&era=+31+&sort=&category=%20"),
            SONG_LIST_PARAMS,
        )
        .unwrap();
        assert_eq!(params.get("q"), Some("café"));
        assert_eq!(params.get("era"), Some("31"));
        assert_eq!(params.get("sort"), None);
        assert_eq!(params.get("category"), None);
        assert_eq!(params.get("missing"), None);

        let error = Params::parse(Some("q=%FF%FE"), SONG_LIST_PARAMS).unwrap_err();
        assert_eq!(error.to_string(), "400 Invalid search query");
        let error = Params::parse(Some("era=%FF"), SONG_LIST_PARAMS).unwrap_err();
        assert_eq!(error.to_string(), "400 Invalid era filter");
        // The era song list doesn't read `era`.
        assert!(Params::parse(Some("era=%FF"), ERA_SONG_PARAMS).is_ok());
    }

    #[test]
    fn totals_and_errors_carry_their_headers() {
        let response = json_list(serde_json::json!([]), 956);
        assert_eq!(response.headers()["x-total-count"], "956");
        assert_eq!(response.headers()[header::CACHE_CONTROL], JSON_CACHE);

        let error = json_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
        assert_eq!(error.headers()[header::CACHE_CONTROL], NO_STORE);
    }
}
