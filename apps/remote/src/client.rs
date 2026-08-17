//! HTTP plumbing for the server's job flow: `POST /compress` queues the job
//! (202), the job is polled until it leaves the in-progress states, the
//! archive is downloaded, and the job is deleted server-side once the bytes
//! are safely in hand. The decisions this loop makes live in
//! [`crate::protocol`]; this module only performs them.

use std::io::Read;
use std::path::Path;
use std::time::Duration;

use collapse_core::compression::compress_tar_dir;
use collapse_core::Algorithm;

use crate::protocol::{self, Progress};
use crate::RemoteError;

/// How often the job status is polled while the server compresses.
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Compress a file or a whole directory on a remote server and return the
/// archive bytes.
///
/// A file is uploaded as it is. A directory cannot be expressed over HTTP, so
/// it is packed into a **tar envelope** first and the server is told to unwrap
/// it: tar is the right envelope precisely because it does not compress, so
/// the CPU work still happens on the far side and the server's upload cap
/// still bounds how much can be unpacked.
///
/// The name stored inside the archive is the source's own file or directory
/// name. Blocks until the job settles, so callers that must stay responsive
/// should run it off their main thread.
pub fn compress_path(
    server: &str,
    source: &Path,
    algorithm: Algorithm,
    level: u32,
) -> Result<Vec<u8>, RemoteError> {
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| RemoteError::Packing {
            path: source.display().to_string(),
            reason: "it has no file name".to_string(),
        })?;

    let (data, envelope) = if source.is_dir() {
        (pack_directory(source, &name)?, "tar")
    } else {
        (std::fs::read(source)?, "none")
    };

    upload_and_collect(server, &name, algorithm, level, envelope, &data)
}

/// Pack a directory into a tar, on disk, and hand back its bytes. The archive
/// carries the directory's own name as its single top-level entry, which is
/// what the server checks the upload against.
fn pack_directory(source: &Path, name: &str) -> Result<Vec<u8>, RemoteError> {
    let staging = tempfile::tempdir()?;
    let tar = staging.path().join(format!("{name}.tar"));

    compress_tar_dir(source, &tar).map_err(|e| RemoteError::Packing {
        path: source.display().to_string(),
        reason: e.to_string(),
    })?;

    Ok(std::fs::read(&tar)?)
}

fn upload_and_collect(
    server: &str,
    name: &str,
    algorithm: Algorithm,
    level: u32,
    envelope: &str,
    data: &[u8],
) -> Result<Vec<u8>, RemoteError> {
    let base = protocol::base_url(server);

    let job = create_job(base, name, algorithm, level, envelope, data)?;
    let job_id = protocol::job_id_of(&job)?.to_string();

    wait_for_completion(base, &job_id)?;
    let archive = download(base, &job_id)?;

    // Best-effort cleanup: the archive is already downloaded, so a failed
    // delete should not fail the operation.
    let _ = ureq::delete(&format!("{base}/jobs/{job_id}")).call();

    Ok(archive)
}

/// Check that a Collapse server is reachable and speaking this protocol.
///
/// Used before adding a server to a UI's list, so a typo shows up there
/// instead of at the end of an upload.
pub fn check_health(server: &str) -> Result<(), RemoteError> {
    let base = protocol::base_url(server);
    let response = ureq::get(&format!("{base}/health"))
        .call()
        .map_err(|e| remote_error(base, e))?;

    protocol::healthy(&parse_json(response)?)
}

/// `POST /compress`: send the bytes, get the queued job back (202).
fn create_job(
    base: &str,
    name: &str,
    algorithm: Algorithm,
    level: u32,
    envelope: &str,
    data: &[u8],
) -> Result<serde_json::Value, RemoteError> {
    let response = ureq::post(&format!("{base}/compress"))
        .query("name", name)
        .query("algorithm", algorithm.extension())
        .query("level", &level.to_string())
        .query("envelope", envelope)
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
