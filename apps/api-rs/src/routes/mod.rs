//! Route table and shared response helpers.

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use serde_json::Value;

use crate::state::SharedState;

pub mod eras;
pub mod songs;

pub const JSON_CACHE: &str = "public, max-age=60, s-maxage=300, stale-while-revalidate=600";
pub const COVER_CACHE: &str = "public, max-age=86400, immutable";
pub const MEDIA_CACHE: &str = "public, max-age=31536000, immutable";
pub const DURATION_CACHE: &str = "public, max-age=86400, immutable";

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

pub fn router(state: SharedState) -> axum::Router {
    use axum::routing::get;

    axum::Router::new()
        .route("/health", get(health))
        .route("/hello", get(hello))
        .route("/eras", get(eras::list_eras))
        .route("/eras/{id}", get(eras::get_era))
        .route("/eras/{id}/songs", get(eras::list_era_songs))
        .route("/eras/{id}/cover", get(eras::get_era_cover))
        .route("/songs", get(songs::list_songs))
        .route("/songs/{id}", get(songs::get_song))
        .route("/songs/{id}/stream", get(songs::stream_song))
        .route("/songs/{id}/download", get(songs::download_song))
        .route("/songs/{id}/duration", get(songs::get_song_duration))
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(state)
}

async fn health() -> Response {
    json_response(serde_json::json!({ "status": "ok" }))
}

async fn hello() -> Response {
    json_response(serde_json::json!({ "hello": "world" }))
}

async fn not_found() -> Response {
    let mut response = json_response(serde_json::json!({ "error": "Not found" }));
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}
