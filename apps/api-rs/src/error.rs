//! Error responses.
//!
//! Hono's `HTTPException` serialises as `text/plain;charset=UTF-8` with the
//! thrown message as the body; anything else that escapes a handler lands in
//! the global error handler and becomes `500 {"error":"Internal server error"}`.

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

#[derive(Debug)]
pub enum ApiError {
    /// Mirrors `HTTPException`: status + exact message as a plain-text body.
    Http { status: StatusCode, message: String },
    /// Mirrors an unhandled exception: logged, then 500 JSON.
    Unexpected(String),
}

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

    pub fn unexpected(detail: impl std::fmt::Display) -> Self {
        ApiError::Unexpected(detail.to_string())
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        ApiError::Unexpected(format!("sqlite error: {error}"))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::Http { status, message } => {
                let mut response = (status, message).into_response();
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/plain;charset=UTF-8"),
                );
                response
            }
            ApiError::Unexpected(detail) => {
                eprintln!("request failed {detail}");
                let mut response = (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({ "error": "Internal server error" })),
                )
                    .into_response();
                response.headers_mut().insert(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                );
                response
            }
        }
    }
}
