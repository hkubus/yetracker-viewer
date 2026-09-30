//! Error responses.
//!
//! A route error is its status plus the message as a `text/plain` body (e.g.
//! `400 Invalid era id`); anything unexpected is logged and becomes `500
//! {"error":"Internal server error"}`. Errors are never cacheable
//! (`Cache-Control: no-store`), so a transient failure can't stick in a
//! browser or CDN cache.

use std::fmt;

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tracing::error;

#[derive(Debug)]
pub enum ApiError {
    /// Status + exact message as a plain-text body.
    Http { status: StatusCode, message: String },
    /// `503` + `Retry-After`: busy right now, worth retrying shortly.
    Busy {
        message: String,
        retry_after_secs: u32,
    },
    /// An unhandled failure: logged, then 500 JSON.
    Unexpected(String),
}

/// The detail of a 500, attached to the response's extensions so request
/// logging can report it next to the method and path.
#[derive(Debug, Clone)]
pub struct ErrorDetail(pub String);

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        ApiError::Http {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        ApiError::new(400, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        ApiError::new(404, message)
    }

    pub fn unprocessable(message: impl Into<String>) -> Self {
        ApiError::new(422, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        ApiError::new(500, message)
    }

    /// `503` without `Retry-After`: the feature is unavailable on this server
    /// (e.g. a missing tool), retrying soon won't help.
    pub fn unavailable(message: impl Into<String>) -> Self {
        ApiError::new(503, message)
    }

    /// `503` with `Retry-After`: every slot is busy right now.
    pub fn busy(message: impl Into<String>, retry_after_secs: u32) -> Self {
        ApiError::Busy {
            message: message.into(),
            retry_after_secs,
        }
    }

    pub fn unexpected(detail: impl std::fmt::Display) -> Self {
        ApiError::Unexpected(detail.to_string())
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Http { status, message } => {
                write!(formatter, "{} {message}", status.as_u16())
            }
            ApiError::Busy { message, .. } => write!(formatter, "503 {message}"),
            ApiError::Unexpected(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        ApiError::Unexpected(format!("sqlite error: {error}"))
    }
}

fn plain_text(status: StatusCode, message: String) -> Response {
    let mut response = (status, message).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain;charset=UTF-8"),
    );
    response
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = match self {
            ApiError::Http { status, message } => plain_text(status, message),
            ApiError::Busy {
                message,
                retry_after_secs,
            } => {
                let mut response = plain_text(StatusCode::SERVICE_UNAVAILABLE, message);
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
                response
            }
            ApiError::Unexpected(detail) => {
                error!(error = %detail, "request failed");
                let mut response = (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({ "error": "Internal server error" })),
                )
                    .into_response();
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response.extensions_mut().insert(ErrorDetail(detail));
                response
            }
        };
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_are_plain_text_and_never_cached() {
        let response = ApiError::bad_request("Invalid era id").into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain;charset=UTF-8"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");

        let busy = ApiError::busy("Search is busy, try again", 2).into_response();
        assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(busy.headers()[header::RETRY_AFTER], "2");
        assert_eq!(busy.headers()[header::CACHE_CONTROL], "no-store");

        let failed = ApiError::unexpected("disk on fire").into_response();
        assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(failed.headers()[header::CONTENT_TYPE], "application/json");
        assert_eq!(failed.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            failed
                .extensions()
                .get::<ErrorDetail>()
                .map(|detail| detail.0.as_str()),
            Some("disk on fire")
        );
    }
}
