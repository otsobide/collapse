//! The pure half of the remote protocol: URL building, reading the server's
//! JSON, and the decision the poll loop makes on each job status. Kept apart
//! from the HTTP plumbing so the test crate can exercise it without a server:
//! source files here carry no inline `mod tests`.

use crate::RemoteError;

/// Normalize the user-supplied server URL into a base the endpoints are
/// joined onto, so `http://host:8000/` does not yield `//compress`.
pub fn base_url(server: &str) -> &str {
    server.trim_end_matches('/')
}

/// The `job_id` out of the 202 body.
pub fn job_id_of(job: &serde_json::Value) -> Result<&str, RemoteError> {
    job["job_id"]
        .as_str()
        .ok_or_else(|| RemoteError::Malformed("malformed server response: no job_id".to_string()))
}

/// What the client should do after reading a job's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Still queued or compressing: poll again.
    Waiting,
    /// Completed: the archive can be downloaded.
    Ready,
}

/// Decide whether to keep polling, stop, or give up, from a job's JSON.
///
/// Only the two in-progress states mean "wait". Anything else — an unknown
/// status, or a body with no status at all — is an error: a server answering
/// that is not speaking this protocol, and treating it as in-progress would
/// poll until the caller gives up.
pub fn progress_of(job: &serde_json::Value) -> Result<Progress, RemoteError> {
    match job["status"].as_str() {
        Some("queued") | Some("compressing") => Ok(Progress::Waiting),
        Some("completed") => Ok(Progress::Ready),
        Some("failed") => {
            let message = job["error_message"]
                .as_str()
                .unwrap_or("compression failed on the server");
            Err(RemoteError::Failed(message.to_string()))
        }
        Some(other) => Err(RemoteError::Malformed(format!(
            "unexpected job status from the server: {other:?}"
        ))),
        None => Err(RemoteError::Malformed(
            "malformed server response: no status".to_string(),
        )),
    }
}

/// Render an HTTP error response for a human: the server's JSON `detail`
/// when there is one, else the raw body, else just the status code.
pub fn rejection_message(code: u16, body: &str) -> String {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("detail")
                .and_then(|d| d.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| body.to_string());

    if detail.is_empty() {
        format!("the server rejected the request (HTTP {code})")
    } else {
        format!("the server rejected the request (HTTP {code}): {detail}")
    }
}
