//! HTTP client for a remote collapse-api server, following its job flow:
//! `POST /compress` queues the job (202), the job is polled until it leaves
//! the in-progress states, the archive is downloaded, and the job is deleted
//! server-side once the bytes are safely in hand. The decisions this loop
//! makes live in [`crate::protocol`]; this module is only the plumbing.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use collapse_core::Algorithm;

use crate::protocol::{self, Progress};
use crate::CliError;

/// How often the job status is polled while the server compresses.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

pub(crate) fn compress_remote(
    server: &str,
    source: &Path,
    arcname: &str,
    algorithm: Algorithm,
    level: u32,
) -> Result<Vec<u8>, CliError> {
    let data = std::fs::read(source)?;
    let base = protocol::base_url(server);

    let job = create_job(base, arcname, algorithm, level, &data)?;
    let job_id = protocol::job_id_of(&job)?.to_string();

    wait_for_completion(base, &job_id)?;
    let archive = download(base, &job_id)?;

    // Best-effort cleanup: the archive is already downloaded, so a failed
    // delete should not fail the command.
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
) -> Result<serde_json::Value, CliError> {
    let response = ureq::post(&format!("{base}/compress"))
        .query("name", arcname)
        .query("algorithm", algorithm.extension())
        .query("level", &level.to_string())
        .send_bytes(data)
        .map_err(|e| remote_error(base, e))?;
    parse_json(response)
}

/// Poll `GET /jobs/{id}` until the job is ready (Ok) or gives up (Err).
fn wait_for_completion(base: &str, job_id: &str) -> Result<(), CliError> {
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
fn download(base: &str, job_id: &str) -> Result<Vec<u8>, CliError> {
    let response = ureq::get(&format!("{base}/jobs/{job_id}/download"))
        .call()
        .map_err(|e| remote_error(base, e))?;
    let mut archive = Vec::new();
    response.into_reader().read_to_end(&mut archive)?;
    Ok(archive)
}

fn parse_json(response: ureq::Response) -> Result<serde_json::Value, CliError> {
    let body = response
        .into_string()
        .map_err(|e| CliError::Remote(format!("cannot read the server response: {e}")))?;
    serde_json::from_str(&body)
        .map_err(|e| CliError::Remote(format!("malformed server response: {e}")))
}

/// Map a ureq error: HTTP error statuses render the server's JSON `detail`,
/// transport errors point at the unreachable server.
fn remote_error(server: &str, err: ureq::Error) -> CliError {
    match err {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            CliError::Remote(protocol::rejection_message(code, &body))
        }
        other => CliError::Remote(format!("cannot reach the server at {server}: {other}")),
    }
}
