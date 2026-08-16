use collapse_core::Algorithm;
use serde::Serialize;

/// Lifecycle states of a compression job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Compressing,
    Completed,
    Failed,
}

/// One compression job tracked by the in-memory registry. Serialized as-is
/// in the 202 response and by the status endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub job_id: String,
    /// Original file name; also the arcname inside the archive.
    pub name: String,
    /// `<name>.<ext>` — the download file name.
    pub archive_name: String,
    pub algorithm: Algorithm,
    pub level: u32,
    pub status: JobStatus,
    pub error_message: Option<String>,
}

impl Job {
    pub fn new(job_id: String, name: String, algorithm: Algorithm, level: u32) -> Self {
        let archive_name = format!("{name}.{}", algorithm.extension());
        Self {
            job_id,
            name,
            archive_name,
            algorithm,
            level,
            status: JobStatus::Queued,
            error_message: None,
        }
    }
}
