use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use collapse_core::Algorithm;

/// On-disk staging for jobs: one directory per job under a base directory,
/// holding the uploaded input and the produced archive, so deleting a job is
/// a single `remove_dir_all`.
pub struct Storage {
    base: PathBuf,
}

impl Storage {
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }

    fn job_dir(&self, job_id: &str) -> PathBuf {
        self.base.join(job_id)
    }

    /// Where a job's uploaded input lives. `name` is validated upstream to be
    /// a bare file name, so the join cannot escape the job directory.
    ///
    /// The upload gets its own subdirectory so it can never land on the
    /// archive path: an upload called `archive.zip` compressed to zip would
    /// otherwise be its own output, and the backends that create the output
    /// before reading the source would truncate it to nothing.
    pub fn input_path(&self, job_id: &str, name: &str) -> PathBuf {
        self.job_dir(job_id).join("input").join(name)
    }

    /// Where a job's produced archive lives.
    pub fn output_path(&self, job_id: &str, algorithm: Algorithm) -> PathBuf {
        self.job_dir(job_id)
            .join(format!("archive.{}", algorithm.extension()))
    }

    /// Persist an uploaded input, creating the job directory.
    pub fn save_input(&self, job_id: &str, name: &str, data: &[u8]) -> io::Result<()> {
        let path = self.input_path(job_id, name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, data)
    }

    /// Remove a job's directory (input and archive). Returns `true` if it
    /// existed.
    pub fn delete_job(&self, job_id: &str) -> bool {
        let dir = self.job_dir(job_id);
        Path::new(&dir).exists() && fs::remove_dir_all(&dir).is_ok()
    }
}
