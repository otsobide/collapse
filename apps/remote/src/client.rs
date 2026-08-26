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
use crate::waiting::{self, Poller};
use crate::RemoteError;

/// How long to wait on each stage of **one exchange** with the server.
///
/// These bound the server's *responsiveness*, never the job. That distinction
/// is the whole design: compression can legitimately run for minutes or hours,
/// and nothing here shortens it.
///
/// * a status poll is a tiny request the server answers at once, so silence on
///   a live socket for [`Timeouts::read`] means the far side is gone, not busy;
/// * `read` and `write` are **per socket operation**, not per response, which
///   was verified rather than assumed: a response dribbled out over 2.7 s in
///   fast chunks passes a 1 s read timeout untouched. A large upload or
///   download that keeps moving therefore never trips one.
///
/// The values are deliberately generous. Hitting one has to be unambiguous
/// evidence that something is wrong, not evidence that we were impatient.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// Getting a socket open at all.
    pub connect: Duration,
    /// Waiting on any single read from an open socket.
    pub read: Duration,
    /// Waiting on any single write to an open socket.
    pub write: Duration,
}

/// Long enough that a loaded server or a slow link is not mistaken for a dead
/// one.
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Silence this long, on a socket that is open, is not a busy server.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// A peer that has stopped draining what we send is in the same state.
pub const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: DEFAULT_CONNECT_TIMEOUT,
            read: DEFAULT_READ_TIMEOUT,
            write: DEFAULT_WRITE_TIMEOUT,
        }
    }
}

impl Timeouts {
    /// One agent for the whole exchange, so every request carries the same
    /// limits. Building it per call would also throw away the connection pool.
    fn agent(&self) -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout_connect(self.connect)
            .timeout_read(self.read)
            .timeout_write(self.write)
            .build()
    }
}

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
    compress_path_with(server, source, algorithm, level, Timeouts::default())
}

/// [`compress_path`] with the exchange limits spelled out.
///
/// Exists for the same reason `collapse_core::extract_with` does: the default
/// is right for every front-end, and a test cannot afford to wait 30 seconds
/// to prove what happens after 30 seconds of silence.
pub fn compress_path_with(
    server: &str,
    source: &Path,
    algorithm: Algorithm,
    level: u32,
    timeouts: Timeouts,
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

    let agent = timeouts.agent();
    upload_and_collect(
        &agent, timeouts, base, &name, algorithm, level, envelope, &data,
    )
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
#[allow(clippy::too_many_arguments)]
fn upload_and_collect(
    agent: &ureq::Agent,
    timeouts: Timeouts,
    base: &str,
    name: &str,
    algorithm: Algorithm,
    level: u32,
    envelope: &str,
    data: &[u8],
) -> Result<Vec<u8>, RemoteError> {
    let job = create_job(
        agent, timeouts, base, name, algorithm, level, envelope, data,
    )?;
    let job_id = protocol::job_id_of(&job)?.to_string();

    wait_for_completion(agent, timeouts, base, &job_id)?;
    let archive = download(agent, timeouts, base, &job_id)?;

    // Best-effort cleanup: the archive is already downloaded, so a failed
    // delete should not fail the operation.
    let _ = agent.delete(&format!("{base}/jobs/{job_id}")).call();

    Ok(archive)
}

/// Check that a Collapse server is reachable and speaking this protocol.
///
/// Used before adding a server to a UI's list, so a typo shows up there
/// instead of at the end of an upload.
pub fn check_health(server: &str) -> Result<(), RemoteError> {
    check_health_with(server, Timeouts::default())
}

/// [`check_health`] with the exchange limits spelled out.
pub fn check_health_with(server: &str, timeouts: Timeouts) -> Result<(), RemoteError> {
    let base = protocol::base_url(server)?;
    let response = timeouts
        .agent()
        .get(&format!("{base}/health"))
        .call()
        .map_err(|e| remote_error(base, timeouts, e))?;

    protocol::healthy(&parse_json(response)?)
}

/// `POST /compress`: send the bytes, get the queued job back (202).
#[allow(clippy::too_many_arguments)]
fn create_job(
    agent: &ureq::Agent,
    timeouts: Timeouts,
    base: &str,
    name: &str,
    algorithm: Algorithm,
    level: u32,
    envelope: &str,
    data: &[u8],
) -> Result<serde_json::Value, RemoteError> {
    let response = agent
        .post(&format!("{base}/compress"))
        .query("name", name)
        .query("algorithm", algorithm.extension())
        .query("level", &level.to_string())
        .query("envelope", envelope)
        .send_bytes(data)
        .map_err(|e| remote_error(base, timeouts, e))?;
    parse_json(response)
}

/// Ask `GET /jobs/{id}` once. The loop that decides when to ask again lives in
/// [`crate::waiting`]; this is only the half that needs a socket.
struct HttpPoller<'a> {
    agent: &'a ureq::Agent,
    timeouts: Timeouts,
    base: &'a str,
    job_id: &'a str,
}

impl Poller for HttpPoller<'_> {
    fn poll(&self) -> Result<Progress, RemoteError> {
        let response = self
            .agent
            .get(&format!("{}/jobs/{}", self.base, self.job_id))
            .call()
            .map_err(|e| remote_error(self.base, self.timeouts, e))?;
        protocol::progress_of(&parse_json(response)?)
    }
}

/// Poll `GET /jobs/{id}` until the job is ready (Ok) or gives up (Err).
///
/// The wait starts at [`waiting::FIRST_POLL_DELAY`] and doubles to
/// [`waiting::MAX_POLL_DELAY`]. It used to be that ceiling from the first wait
/// onwards, so a job the server had already finished still cost the caller
/// 200 ms of sleeping (issue #48).
fn wait_for_completion(
    agent: &ureq::Agent,
    timeouts: Timeouts,
    base: &str,
    job_id: &str,
) -> Result<(), RemoteError> {
    let poller = HttpPoller {
        agent,
        timeouts,
        base,
        job_id,
    };
    waiting::wait_for(&waiting::RealSleeper, &poller).map(|_| ())
}

/// `GET /jobs/{id}/download`: the archive bytes.
fn download(
    agent: &ureq::Agent,
    timeouts: Timeouts,
    base: &str,
    job_id: &str,
) -> Result<Vec<u8>, RemoteError> {
    let response = agent
        .get(&format!("{base}/jobs/{job_id}/download"))
        .call()
        .map_err(|e| remote_error(base, timeouts, e))?;
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
fn remote_error(server: &str, timeouts: Timeouts, err: ureq::Error) -> RemoteError {
    match err {
        ureq::Error::Status(status, response) => {
            let body = response.into_string().unwrap_or_default();
            RemoteError::Rejected {
                status,
                message: protocol::rejection_message(status, &body),
            }
        }
        // "I could not get a socket open" and "I had one and the far side went
        // quiet" are different diagnoses and want different sentences. ureq
        // separates them for us: a refused connection and a connect timeout are
        // both `ConnectionFailed`, an unresolvable name is `Dns`, and anything
        // that goes wrong on an established socket, a read timeout included, is
        // `Io`. Checked against the real crate rather than assumed.
        ureq::Error::Transport(transport) if transport.kind() == ureq::ErrorKind::Io => {
            RemoteError::Unresponsive {
                server: server.to_string(),
                after: timeouts.read,
                reason: transport.to_string(),
            }
        }
        other => RemoteError::Unreachable {
            server: server.to_string(),
            reason: other.to_string(),
        },
    }
}
