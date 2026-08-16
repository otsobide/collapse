//! HTTP plumbing for the server's job flow: `POST /compress` queues the job
//! (202), the job is polled until it leaves the in-progress states, the
//! archive is downloaded, and the job is deleted server-side once the bytes
//! are safely in hand. The decisions this loop makes live in
//! [`crate::protocol`]; this module only performs them.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use collapse_core::Algorithm;

use crate::protocol::{self, Progress};
use crate::RemoteError;

/// How often the job status is polled while the server compresses.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Compress one file on a remote server and return the archive bytes.
///
/// `arcname` is the name the content will carry inside the archive, and must
/// be a bare file name: the server rejects anything else. Blocks until the
/// job settles, so callers that need to stay responsive should run it off
/// their main thread.
pub fn compress_file(
    server: &str,
    source: &Path,
    arcname: &str,
    algorithm: Algorithm,
    level: u32,
) -> Result<Vec<u8>, RemoteError> {
    let data = std::fs::read(source)?;
    let base = protocol::base_url(server);

    let job = create_job(base, arcname, algorithm, level, &data)?;
    let job_id = protocol::job_id_of(&job)?.to_string();

    wait_for_completion(base, &job_id)?;
    let archive = download(base, &job_id)?;

    // Best-effort cleanup: the archive is already downloaded, so a failed
    // delete should not fail the operation.
    let _ = ureq::delete(&format!("{base}/jobs/{job_id}")).call();

    Ok(archive)
}

/// `POST /compress`: send the bytes, get the queued job back (202).
fn create_job(
    base: &str,
    arcname: &str,
    algorithm: Algorithm,
    level: u32,
    data: &[u8],
) -> Result<serde_json::Value, RemoteError> {
    let response = ureq::post(&format!("{base}/compress"))
        .query("name", arcname)
        .query("algorithm", algorithm.extension())
        .query("level", &level.to_string())
        .send_bytes(data)
        .map_err(|e| remote_error(base, e))?;
    parse_json(response)
}

/// Poll `GET /jobs/{id}` until the job is ready (Ok) or gives up (Err).
fn wait_for_completion(base: &str, job_id: &str) -> Result<(), RemoteError> {
    loop {
        let response = ureq::get(&format!("{base}/jobs/{job_id}"))
            .call()
            .map_err(|e| remote_error(base, e))?;

        match protocol::progress_of(&parse_json(response)?)? {
            Progress::Ready => return Ok(()),
            Progress::Waiting => std::thread::sleep(POLL_INTERVAL),
        }
    }
}

/// `GET /jobs/{id}/download`: the archive bytes.
fn download(base: &str, job_id: &str) -> Result<Vec<u8>, RemoteError> {
    let response = ureq::get(&format!("{base}/jobs/{job_id}/download"))
        .call()
        .map_err(|e| remote_error(base, e))?;
    let mut archive = Vec::new();
    response.into_reader().read_to_end(&mut archive)?;
    Ok(archive)
}

fn parse_json(response: ureq::Response) -> Result<serde_json::Value, RemoteError> {
    let body = response
        .into_string()
        .map_err(|e| RemoteError::Malformed(format!("cannot read the server response: {e}")))?;
    serde_json::from_str(&body)
        .map_err(|e| RemoteError::Malformed(format!("malformed server response: {e}")))
}

/// Map a ureq error: HTTP error statuses render the server's JSON `detail`,
/// transport errors point at the unreachable server.
fn remote_error(server: &str, err: ureq::Error) -> RemoteError {
    match err {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            RemoteError::Rejected {
                status,
                message: protocol::rejection_message(status, &body),
            }
        }
        other => RemoteError::Unreachable {
            server: server.to_string(),
            reason: other.to_string(),
        },
    }
}
