//! The pure half of the remote protocol: URL building, reading the server's
//! JSON, and the decision the poll loop makes on each job status. Kept apart
//! from the HTTP plumbing so the test crate can exercise it without a server:
//! source files here carry no inline `mod tests`.

use crate::RemoteError;

/// Normalize the user-supplied server URL into a base the endpoints are
/// joined onto, so `http://host:8000/` does not yield `//compress`.
///
/// An address that is empty, or nothing but whitespace, is refused here
/// instead of being sent. This is the one place that answer is written, so
/// the front-ends cannot disagree about it the way they used to: the desktop
/// read `""` as "compress locally" and `"   "` as a server, the CLI read both
/// as a server.
///
/// Refused rather than read as "compress locally" on purpose. A blank cannot
/// come from the desktop's own UI (`sources.js` normalizes an address to
/// `null` or a real URL), so it means a stale stored value or a caller's bug;
/// on the CLI it means a flag someone typed wrong. Quietly compressing
/// locally would hide both.
pub fn base_url(server: &str) -> Result<&str, RemoteError> {
    // Normalize first, then decide. Asking `is_empty` of the trimmed input but
    // returning the untrimmed one let two shapes through that are not
    // addresses: `"///"` is not blank, so it passed, and then lost its slashes
    // and reached the caller as an empty base, producing the very "cannot reach
    // the server at : ..." this function exists to prevent; and a trailing
    // space defeated the slash trim, so `"http://host:8000/ "` kept the
    // separator. One rule covers both: an address made of nothing but
    // whitespace and slashes is not an address.
    let base = server.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(RemoteError::BlankServer);
    }
    Ok(base)
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

/// Read a `GET /health` body: it must say the server is ok.
///
/// Anything else means something answered, but not a Collapse server, which
/// is worth catching when a user is typing a URL into a settings dialog.
pub fn healthy(body: &serde_json::Value) -> Result<(), RemoteError> {
    match body["status"].as_str() {
        Some("ok") => Ok(()),
        _ => Err(RemoteError::Malformed(
            "the address answered but does not look like a Collapse server".to_string(),
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
