use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use collapse_core::CompressionError;

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
///
/// **500 on purpose**, including for a row this build cannot interpret. The
/// server does have a state it cannot make sense of, which is its problem and
/// not the caller's, and a 4xx would tell a client to retry differently when
/// nothing it does can help. What was wrong with the old behaviour was the
/// *message*, not the status: `RegistryError` renders one that names the
/// version that wrote the row and the field that could not be read.
impl From<crate::registry::RegistryError> for ApiError {
    fn from(err: crate::registry::RegistryError) -> Self {
        if let crate::registry::RegistryError::Unreadable { job_id, .. } = &err {
            tracing::error!(job = %job_id, "{err}");
        }
        ApiError::Internal(err.to_string())
    }
}

/// The `error_message` a failed job carries, given what the engine returned.
///
/// Clients (the CLI, the web app, `curl`) print this verbatim, so it is written
/// for a person, and it is a `Display` rather than a `Debug` dump for the same
/// reason.
///
/// Only a verification failure is rewritten, and only because the engine's own
/// message names the file it read back, which here is a path inside the job's
/// staging directory: a location the client has never heard of, cannot reach,
/// and should not be told about. The archive's own name is the same fact said
/// in the client's vocabulary. Every other error already reads as a sentence
/// about something the client did (an unreadable upload, a tar that is not a
/// tar), so it is passed through untouched.
pub fn failure_message(archive_name: &str, error: &CompressionError) -> String {
    match error {
        CompressionError::VerificationFailed { reason, .. } => format!(
            "{archive_name} was compressed but did not check out, so it was discarded: {reason}"
        ),
        other => other.to_string(),
    }
}

/// What can stop the server from coming up: the two things it owns are the
/// job registry and the staging directory, and it refuses to serve without
/// either rather than starting half-configured.
#[derive(Debug)]
pub enum StartupError {
    Registry(crate::registry::RegistryError),
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

impl From<crate::registry::RegistryError> for StartupError {
    fn from(err: crate::registry::RegistryError) -> Self {
        StartupError::Registry(err)
    }
}

impl From<std::io::Error> for StartupError {
    fn from(err: std::io::Error) -> Self {
        StartupError::Storage(err)
    }
}
