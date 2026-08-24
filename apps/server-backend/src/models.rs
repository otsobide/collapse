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

/// How thoroughly the archive is checked before the job is called done.
///
/// The engine always reads the finished archive's own listing back, so the
/// failure this exists for (a compression that stopped early and finalised
/// anyway, leaving a valid archive silently missing entries) is caught whatever
/// is asked for here. What this chooses is whether to go further and decompress
/// every entry, which roughly doubles the work.
///
/// It is a wire type of this crate's own rather than [`collapse_core::Verify`]
/// re-exported, for the same reason [`Envelope`] is: the engine's enum has no
/// spelling on the wire, in a log line or in a database column, and giving it
/// one would put this server's HTTP contract inside the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verify {
    /// Read the listing back and confirm it names exactly the entries that
    /// were meant to go in. Nothing is decompressed. The default.
    Index,
    /// The listing, and then every entry decompressed and discarded, so the
    /// format's own checksums fire.
    ///
    /// What that buys depends on the format, and the difference is worth
    /// stating rather than implying all three are equal: zip and 7z store a
    /// CRC per entry and catch a flipped bit in the data, while tar checksums
    /// only its headers, so there it confirms the archive is well formed and
    /// complete and nothing more.
    Contents,
}

/// Renders what the wire carries (`index`, `contents`), not the Rust variant
/// name, so a log line can be read against the request that produced it.
impl std::fmt::Display for Verify {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verify::Index => f.write_str("index"),
            Verify::Contents => f.write_str("contents"),
        }
    }
}

impl FromStr for Verify {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "index" => Ok(Verify::Index),
            "contents" => Ok(Verify::Contents),
            other => Err(format!(
                "Unknown verify: {other}. Must be \"index\" or \"contents\"."
            )),
        }
    }
}

impl From<Verify> for collapse_core::Verify {
    fn from(verify: Verify) -> Self {
        match verify {
            Verify::Index => collapse_core::Verify::Index,
            Verify::Contents => collapse_core::Verify::Contents,
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
    /// How deeply the finished archive is checked. Stored on the job rather
    /// than carried to the worker, because the worker is handed a job id and
    /// nothing else, and reads everything it needs from the registry.
    pub verify: Verify,
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
        verify: Verify,
    ) -> Self {
        let archive_name = format!("{name}.{}", algorithm.extension());
        Self {
            job_id,
            name,
            archive_name,
            algorithm,
            level,
            envelope,
            verify,
            status: JobStatus::Queued,
            error_message: None,
        }
    }
}
