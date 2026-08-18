use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// Application-level errors, mapped to HTTP responses with a JSON
/// `{"detail": "..."}` body (the shape clients parse for error messages).
#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    NotFound(String),
    Conflict(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, detail) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg),
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

/// A registry that cannot answer is a server that cannot say what it did with
/// a job, so it is an internal error rather than a silent miss.
impl From<rusqlite::Error> for ApiError {
    fn from(err: rusqlite::Error) -> Self {
        ApiError::Internal(err.to_string())
    }
}

/// What can stop the server from coming up: the two things it owns are the
/// job registry and the staging directory, and it refuses to serve without
/// either rather than starting half-configured.
#[derive(Debug)]
pub enum StartupError {
    Registry(rusqlite::Error),
    Storage(std::io::Error),
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupError::Registry(err) => write!(f, "Cannot open the job registry: {err}"),
            StartupError::Storage(err) => write!(f, "Cannot use the staging directory: {err}"),
        }
    }
}

impl std::error::Error for StartupError {}

impl From<rusqlite::Error> for StartupError {
    fn from(err: rusqlite::Error) -> Self {
        StartupError::Registry(err)
    }
}

impl From<std::io::Error> for StartupError {
    fn from(err: std::io::Error) -> Self {
        StartupError::Storage(err)
    }
}
