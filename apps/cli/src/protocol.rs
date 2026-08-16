//! The pure half of the remote protocol: URL building, reading the server's
//! JSON, and the decision the poll loop makes on each job status. Split out
//! of `remote.rs` (which does the HTTP) so the test crate can exercise it
//! without a server: source files here carry no inline `mod tests`.

use crate::CliError;

/// Normalize the user-supplied server URL into a base the endpoints are
/// joined onto, so `http://host:8000/` does not yield `//compress`.
pub fn base_url(server: &str) -> &str {
    server.trim_end_matches('/')
}

/// The `job_id` out of the 202 body.
pub fn job_id_of(job: &serde_json::Value) -> Result<&str, CliError> {
    job["job_id"]
        .as_str()
        .ok_or_else(|| CliError::Remote("malformed server response: no job_id".to_string()))
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
pub fn progress_of(job: &serde_json::Value) -> Result<Progress, CliError> {
    match job["status"].as_str().unwrap_or("") {
        "completed" => Ok(Progress::Ready),
        "failed" => {
            let message = job["error_message"]
                .as_str()
                .unwrap_or("compression failed on the server");
            Err(CliError::Remote(format!("server-side error: {message}")))
        }
        // queued / compressing: keep waiting.
        _ => Ok(Progress::Waiting),
    }
}

/// Render an HTTP error response for the user: the server's JSON `detail`
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
