use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use collapse_core::CompressionError;

/// Application-level errors, mapped to HTTP responses with a JSON
/// `{"detail": "..."}` body (the shape clients parse for error messages).
#[derive(Debug)]
pub(crate) enum ApiError {
    BadRequest(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, detail) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(json!({ "detail": detail }))).into_response()
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        ApiError::Internal(err.to_string())
    }
}

impl From<CompressionError> for ApiError {
    fn from(err: CompressionError) -> Self {
        match err {
            // The handler validates the level up front, but keep the mapping
            // honest in case core ever rejects something the handler let by.
            CompressionError::InvalidLevel(_) => ApiError::BadRequest(err.to_string()),
            other => ApiError::Internal(other.to_string()),
        }
    }
}
