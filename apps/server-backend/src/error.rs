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
/// Clients (the CLI, the web app, `curl`) print the client half verbatim, so it
/// is written for a person, and it is a `Display` rather than a `Debug` dump
/// for the same reason.
///
/// **The two halves exist because the audiences differ.** An operator reading
/// the log wants the whole truth, including which file on their disk was at
/// fault. A client wants to know what *it* did wrong, and must not be told
/// where anything lives on a machine it has never heard of and cannot reach.
/// The server has no authentication (issue #72), so "a client" is anyone who
/// can reach the port.
pub struct Failure {
    /// What the job's `error_message` becomes, and so what `GET /jobs/{id}`
    /// hands back. Never names a location on this machine.
    pub client: String,
    /// The same failure, whole, for the log.
    pub log: String,
}

impl Failure {
    /// Both halves from one message.
    pub fn from_message(message: String) -> Self {
        Self {
            client: without_locations(&message),
            log: message,
        }
    }
}

/// So a `?` on any of the plain-`String` failures in this crate still works,
/// and still gets redacted. Fail closed: a new error path is safe by default
/// rather than safe only if someone remembers.
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::from_message(message)
    }
}

/// Turn a core error into what the client is told and what the log records.
///
/// Two variants are rewritten rather than redacted, because a curated sentence
/// is more useful than a redacted one:
///
/// * a verification failure names the file it read back, which is a path inside
///   the job's staging directory; the archive's own name is the same fact in the
///   client's vocabulary;
/// * a per-entry write failure names the entry, which is the client's own
///   content and worth keeping, and its destination, which is not.
///
/// **Everything else is redacted rather than passed through.** That is the
/// change: the old code passed every other variant through untouched, on the
/// stated reasoning that they "already read as a sentence about something the
/// client did". That was not true. Unpacking a client's tar envelope reaches
/// `extract_tar`, whose failure reads ``failed to unpack `/…/out/root/a/b` ``,
/// and it did not even go through here (issue #66).
///
/// Enumerating the leaky variants would have been the smaller change and the
/// wrong one: `CompressionError` gains variants, and the next one would leak
/// until somebody noticed.
pub fn failure(archive_name: &str, error: &CompressionError) -> Failure {
    let client = match error {
        CompressionError::VerificationFailed { reason, .. } => format!(
            "{archive_name} was compressed but did not check out, so it was discarded: {reason}"
        ),
        CompressionError::Entry { entry, source, .. } => {
            format!("the entry {entry:?} could not be written: {source}")
        }
        other => without_locations(&other.to_string()),
    };
    Failure {
        client,
        log: error.to_string(),
    }
}

/// Remove anything shaped like a path on this machine.
///
/// Deliberately blunt. The server has no reason to tell a client where anything
/// lives, so removing every absolute path is correct rather than merely
/// convenient, and it does not depend on knowing which variant produced the
/// message or where the staging directory happens to be mounted.
///
/// Relative paths are left alone: those are the client's own entry names, which
/// are exactly what it needs to see.
fn without_locations(message: &str) -> String {
    message
        .split_whitespace()
        .map(|word| {
            // A path is usually wrapped in the punctuation of the sentence
            // around it: backticks, quotes, a trailing comma or colon.
            let trimmed = word.trim_matches(|c: char| {
                c == '`' || c == '"' || c == '\'' || c == ',' || c == ':' || c == '.'
            });
            if looks_like_a_location(trimmed) {
                word.replace(trimmed, "<path>")
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Absolute on Unix, or carrying a Windows drive or UNC prefix.
fn looks_like_a_location(word: &str) -> bool {
    if word.starts_with('/') || word.starts_with("\\\\") {
        return true;
    }
    // `C:\...` or the verbatim `\\?\C:\...` that canonicalize returns.
    let mut chars = word.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some(letter), Some(':'), Some('\\' | '/')) if letter.is_ascii_alphabetic()
    )
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
