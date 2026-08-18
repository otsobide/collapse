use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use collapse_core::Algorithm;

/// The upload's name on disk. Fixed on purpose: see [`Storage::input_path`].
pub const UPLOAD_FILE: &str = "upload";

/// On-disk staging for jobs: one directory per job under a base directory,
/// holding the uploaded input and the produced archive, so deleting a job is
/// a single `remove_dir_all`.
///
/// **Every path here is built from values this server chose**: a job id it
/// generated, and fixed names. Nothing a client sent becomes a path
/// component, which is what makes the layout safe by construction rather than
/// by validation holding.
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

    /// Where a job's uploaded input lives.
    ///
    /// The name the caller sent is deliberately **not** part of this path. It
    /// is not needed: the name the archive carries inside is passed to the
    /// compressor separately, so staging under a fixed name loses nothing and
    /// keeps a string that came off the wire from ever being a path component.
    /// What validation still owes us is a sane arcname, not a safe path.
    ///
    /// The upload also gets its own subdirectory so it can never land on the
    /// archive path: an upload called `archive.zip` compressed to zip would
    /// otherwise be its own output, and the backends that create the output
    /// before reading the source would truncate it to nothing.
    pub fn input_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("input").join(UPLOAD_FILE)
    }

    /// Where a job's produced archive lives.
    pub fn output_path(&self, job_id: &str, algorithm: Algorithm) -> PathBuf {
        self.job_dir(job_id)
            .join(format!("archive.{}", algorithm.extension()))
    }

    /// Where a tar envelope is unpacked, kept apart from both the upload and
    /// the archive so the three can never collide.
    pub fn tree_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("tree")
    }

    /// Persist an uploaded input, creating the job directory.
    pub fn save_input(&self, job_id: &str, data: &[u8]) -> io::Result<()> {
        let path = self.input_path(job_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, data)
    }

    /// Remove a job's directory (input, unpacked tree and archive). Returns
    /// `true` if it existed and is now gone.
    ///
    /// Takes anything that names a directory, not just a `&str`: a name read
    /// off the filesystem is bytes, and going through a `String` on the way
    /// back would rebuild a path that does not exist.
    pub fn delete_job(&self, job_id: impl AsRef<OsStr>) -> bool {
        let dir = self.base.join(job_id.as_ref());
        Path::new(&dir).exists() && fs::remove_dir_all(&dir).is_ok()
    }

    /// Whether a job still has files staged.
    pub fn has_job(&self, job_id: impl AsRef<OsStr>) -> bool {
        self.base.join(job_id.as_ref()).is_dir()
    }

    /// The names of the directories in the staging area, as the filesystem
    /// holds them.
    ///
    /// Only directories count, which is what keeps the registry's own
    /// database (a file, plus the two SQLite writes beside it) from ever
    /// looking like an abandoned job. The names stay `OsString`: every job id
    /// this server writes is hex, but the directory next to them might have
    /// been put there by anything, and a name that is not valid UTF-8 must
    /// still be nameable well enough to delete.
    pub fn staged_jobs(&self) -> io::Result<Vec<OsString>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(&self.base)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                names.push(entry.file_name());
            }
        }
        Ok(names)
    }
}

/// The single top-level directory an unpacked tar envelope must contain.
///
/// The tar arrives from a client, so nothing about its shape is taken on
/// trust: it has to hold exactly one entry, that entry has to be a directory,
/// and it has to be the name the job was created for. Anything else is
/// reported instead of compressing whatever happens to be there.
pub fn single_root_dir(tree: &Path, expected: &str) -> Result<PathBuf, String> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(tree).map_err(|e| format!("cannot read the unpacked upload: {e}"))? {
        entries.push(entry.map_err(|e| format!("cannot read the unpacked upload: {e}"))?);
    }

    let [only] = entries.as_slice() else {
        return Err(format!(
            "the tar envelope must hold exactly one directory, found {}",
            entries.len()
        ));
    };

    let name = only.file_name().to_string_lossy().into_owned();
    if !only.path().is_dir() {
        return Err(format!(
            "the tar envelope must hold a directory, found the file {name:?}"
        ));
    }
    if name != expected {
        return Err(format!(
            "the tar envelope holds {name:?} but the job was created for {expected:?}"
        ));
    }
    Ok(only.path())
}
