//! Unified API error type that converts into HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Errors surfaced to API clients with an appropriate status code.
#[derive(Debug)]
pub enum ApiError {
    /// Bad or missing credentials / token.
    Unauthorized(String),
    /// Malformed request.
    BadRequest(String),
    /// Unexpected server-side failure.
    Internal(String),
}

impl IntoResponse for ApiError {
    /// Maps the error variant to an HTTP status and JSON `{error}` body.
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::Unauthorized(m) => (StatusCode::UNAUTHORIZED, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    /// Treats any anyhow error as an internal server error.
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e.to_string())
    }
}
