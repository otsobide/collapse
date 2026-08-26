//! HTTP plumbing for the server's job flow: `POST /compress` queues the job
//! (202), the job is polled until it leaves the in-progress states, the
//! archive is downloaded, and the job is deleted server-side once the bytes
//! are safely in hand. The decisions this loop makes live in
//! [`crate::protocol`]; this module only performs them.

use std::io::Read;
use std::path::Path;

use collapse_core::compression::compress_tar_dir;
use collapse_core::Algorithm;

use crate::protocol::{self, Progress};
use crate::RemoteError;

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
///
/// A blank `server` is refused ([`RemoteError::BlankServer`]) rather than
/// read as "compress locally": that decision belongs here, once, not to each
/// front-end (see [`protocol::base_url`]).
pub fn compress_path(
    server: &str,
    source: &Path,
    algorithm: Algorithm,
    level: u32,
) -> Result<Vec<u8>, RemoteError> {
    // First, ahead of even looking at the source: a blank address is the
    // thing that is wrong, and packing a whole directory into a tar envelope
    // before saying so throws away the work for nothing.
    let base = protocol::base_url(server)?;

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

    upload_and_collect(base, &name, algorithm, level, envelope, &data)
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

/// `base` is already normalized by [`protocol::base_url`], which is also what
/// rejects an unusable address, so every caller has to go through it first.
fn upload_and_collect(
    base: &str,
    name: &str,
    algorithm: Algorithm,
    level: u32,
    envelope: &str,
    data: &[u8],
) -> Result<Vec<u8>, RemoteError> {
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
    let base = protocol::base_url(server)?;
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
///
/// The wait between polls starts at [`protocol::FIRST_POLL_DELAY`] and doubles
/// to [`protocol::MAX_POLL_DELAY`]. It used to be that ceiling from the first
/// wait onwards, which meant a job the server had already finished still cost
/// the caller 200 ms of this function sleeping (issue #48). The schedule
/// itself lives in `protocol` so it can be checked without a server.
fn wait_for_completion(base: &str, job_id: &str) -> Result<(), RemoteError> {
    let mut delay = protocol::FIRST_POLL_DELAY;
    loop {
        let response = ureq::get(&format!("{base}/jobs/{job_id}"))
            .call()
            .map_err(|e| remote_error(base, e))?;

        match protocol::progress_of(&parse_json(response)?)? {
            Progress::Ready => return Ok(()),
            Progress::Waiting => {
                std::thread::sleep(delay);
                delay = protocol::next_delay(delay);
            }
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
