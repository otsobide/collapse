use std::str::FromStr;

use collapse_core::Algorithm;
use serde::Serialize;

/// How the uploaded bytes should be read.
///
/// A client cannot express a directory over HTTP, so it packs one into a tar
/// and says so here. The flag is required rather than sniffed: `photos.tar`
/// may equally be a file the caller wants compressed as it is, and guessing
/// would make that case impossible to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Envelope {
    /// The body is the file to compress, as it is.
    None,
    /// The body is a tar holding exactly one directory: unpack it and
    /// compress that tree.
    Tar,
}

/// Renders what the wire carries (`none`, `tar`), not the Rust variant name,
/// so a log line can be read against the request that produced it.
impl std::fmt::Display for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Envelope::None => f.write_str("none"),
            Envelope::Tar => f.write_str("tar"),
        }
    }
}

impl FromStr for Envelope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(Envelope::None),
            "tar" => Ok(Envelope::Tar),
            other => Err(format!(
                "Unknown envelope: {other}. Must be \"none\" or \"tar\"."
            )),
        }
    }
}

/// Lifecycle states of a compression job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Compressing,
    Completed,
    Failed,
}

impl JobStatus {
    /// Whether the job is done with, one way or the other. The worker owns
    /// every other state, so anything that reaps or rewrites jobs from the
    /// outside has to leave those alone.
    pub fn is_terminal(&self) -> bool {
        matches!(self, JobStatus::Completed | JobStatus::Failed)
    }
}

/// One spelling for the wire, the log and the database column, so a status
/// read out of any of the three means the same thing.
impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            JobStatus::Queued => "queued",
            JobStatus::Compressing => "compressing",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
        })
    }
}

impl FromStr for JobStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "queued" => Ok(JobStatus::Queued),
            "compressing" => Ok(JobStatus::Compressing),
            "completed" => Ok(JobStatus::Completed),
            "failed" => Ok(JobStatus::Failed),
            other => Err(format!("Unknown job status: {other}.")),
        }
    }
}

/// One compression job tracked by the in-memory registry. Serialized as-is
/// in the 202 response and by the status endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub job_id: String,
    /// The file name, or the directory name when the upload is a tar
    /// envelope; also the arcname inside the archive.
    pub name: String,
    /// `<name>.<ext>` — the download file name.
    pub archive_name: String,
    pub algorithm: Algorithm,
    pub level: u32,
    pub envelope: Envelope,
    pub status: JobStatus,
    pub error_message: Option<String>,
}

impl Job {
    pub fn new(
        job_id: String,
        name: String,
        algorithm: Algorithm,
        level: u32,
        envelope: Envelope,
    ) -> Self {
        let archive_name = format!("{name}.{}", algorithm.extension());
        Self {
            job_id,
            name,
            archive_name,
            algorithm,
            level,
            envelope,
            status: JobStatus::Queued,
            error_message: None,
        }
    }
}
