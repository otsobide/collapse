mod algorithm;
mod sevenz;
mod tar;
mod verify;
mod walk;
mod zip;

pub use self::algorithm::Algorithm;
pub use self::sevenz::{compress_7z, compress_7z_dir, extract_7z};
pub use self::tar::{compress_tar, compress_tar_dir, extract_tar};
pub use self::verify::{verify_archive, Verify};
pub use self::zip::{compress_zip, compress_zip_dir, extract_zip};

pub(crate) use self::sevenz::read_7z_entries;
pub(crate) use self::tar::read_tar_entries;
pub(crate) use self::walk::walk_tree;
pub(crate) use self::zip::read_zip_entries;

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

/// Resolve an archive entry name to a path safe to join onto the output dir.
///
/// Returns the name reduced to its `Normal` components (dropping `.`), or
/// `None` when it is absolute or contains a `..` component — i.e. a path
/// traversal attempt, or an entry that resolves to no file at all.
pub(crate) fn sanitize_entry_path(name: &str) -> Option<PathBuf> {
    let mut safe = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if safe.as_os_str().is_empty() {
        return None;
    }
    Some(safe)
}

/// Errors that can occur during compression.
#[derive(Debug, Error)]
pub enum CompressionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Compression failed: {0}")]
    Failed(String),

    #[error("Invalid compression level: {0}. Must be between 1 and 5.")]
    InvalidLevel(u32),

    /// The compressor reported success, but reading the archive back showed it
    /// was not what it was meant to be, so it was discarded and never reached
    /// `archive`.
    ///
    /// Deliberately not a [`Self::Failed`] with a string: "the compressor
    /// errored" and "the archive I just wrote is wrong" call for different
    /// answers from a caller, and only this one means a bug or a corruption
    /// rather than a file it could not read.
    #[error("Verification of {} failed: {reason}", archive.display())]
    VerificationFailed { archive: PathBuf, reason: String },
}

/// Serial number for staged file names, so two compressions running inside one
/// process cannot pick the same temporary.
static STAGED_SERIAL: AtomicU64 = AtomicU64::new(0);

/// Bytes of the output's own name kept in the temporary's name.
///
/// A file name has a length limit of its own (255 bytes on Linux and macOS), so
/// a long but perfectly legal output name would otherwise fail to stage. The
/// suffix below is at most 46 bytes, and 200 leaves room to spare.
const STAGED_NAME_BUDGET: usize = 200;

/// An archive being written next to where it belongs, not at it.
///
/// zip and tar finalise on drop, so a compression that died halfway used to
/// leave a *valid* archive at the destination silently missing entries: the
/// user saw an error, opened the archive, found it opened fine, and could
/// delete the originals. The bytes therefore go to a temporary file which is
/// renamed into place only once the archive checks out, so the destination only
/// ever holds a finished archive, and a failed run leaves whatever was already
/// there untouched.
///
/// The temporary is a sibling of the output because a rename is only atomic,
/// and on most platforms only possible at all, within one filesystem.
///
/// It is a guard rather than a pair of calls so that an early return through
/// `?`, a panic, or a future edit that adds a step between writing and renaming
/// cannot leak it.
struct StagedOutput {
    path: PathBuf,
    committed: bool,
}

impl StagedOutput {
    /// Pick a staging path beside `output`. Nothing is created here; the
    /// backend creates the file when it writes to [`Self::path`].
    fn beside(output: &Path) -> Result<Self, CompressionError> {
        let file_name = output.file_name().ok_or_else(|| {
            CompressionError::Failed(format!(
                "Not a path an archive can be written to: {}",
                output.display()
            ))
        })?;
        // `parent()` answers `Some("")` for a bare relative name like `out.zip`,
        // and joining onto that would produce a path starting with a separator.
        let dir = match output.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        // Named after the output, marked as ours, and unique: two live
        // processes cannot share a pid, and two compressions in one process
        // cannot share a serial. A leftover after a crash says what it is and
        // which file it was on its way to becoming.
        let file_name = file_name.to_string_lossy();
        let stem = keep_bytes(&file_name, STAGED_NAME_BUDGET);
        let path = dir.join(format!(
            "{stem}.collapse-part-{}-{}",
            std::process::id(),
            STAGED_SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        Ok(Self {
            path,
            committed: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Move the finished archive to where the caller asked for it.
    ///
    /// A rename replaces the destination name rather than writing through it,
    /// which is also what stops a hardlinked output from being written into the
    /// file it shares an inode with.
    fn commit(mut self, output: &Path) -> Result<(), CompressionError> {
        fs::rename(&self.path, output)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        if !self.committed {
            // Best effort on purpose: whatever failure brought us here is what
            // the caller needs to hear about, and there is nothing useful to do
            // with a second one raised while cleaning up.
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// The longest prefix of `name` that fits in `max` bytes, cut on a character
/// boundary so the result is still a string.
fn keep_bytes(name: &str, max: usize) -> &str {
    if name.len() <= max {
        return name;
    }
    let mut end = max;
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    &name[..end]
}

/// Re-point a verification failure at the destination the caller asked for.
///
/// Verification runs on the staged temporary, which is deleted before the error
/// is returned; naming it would send the reader looking for a file that is
/// gone, and for a name they never chose.
fn reported_at(error: CompressionError, output: &Path) -> CompressionError {
    match error {
        CompressionError::VerificationFailed { reason, .. } => {
            CompressionError::VerificationFailed {
                archive: output.to_path_buf(),
                reason,
            }
        }
        other => other,
    }
}

/// Compress a file using the given algorithm and level (1–5).
///
/// The file is stored inside the archive under `arcname`.
/// `tar` archives without compressing, so it ignores the level
/// (which must still be in range).
///
/// Nothing appears at `output` until the archive is written *and* checked at
/// `verify`'s depth: the bytes go to a temporary file beside it and are renamed
/// into place at the end. A failure therefore leaves the destination exactly as
/// it was, whether that is an older archive or nothing at all, and leaves no
/// temporary behind either.
///
/// The backend functions ([`compress_zip`] and friends) write straight to the
/// path they are given and do none of that; this dispatcher is the safe path.
pub fn compress(
    source: &Path,
    output: &Path,
    arcname: &str,
    algorithm: Algorithm,
    level: u32,
    verify: Verify,
) -> Result<(), CompressionError> {
    if !(1..=5).contains(&level) {
        return Err(CompressionError::InvalidLevel(level));
    }
    let staged = StagedOutput::beside(output)?;
    match algorithm {
        Algorithm::SevenZ => compress_7z(source, staged.path(), arcname, level),
        Algorithm::Tar => compress_tar(source, staged.path(), arcname),
        Algorithm::Zip => compress_zip(source, staged.path(), arcname, level),
    }?;
    // One file in, one entry out, under the name the caller chose. Anything
    // else in there, or anything else missing, is not what was asked for.
    let expected = [arcname.to_string()];
    verify_archive(staged.path(), algorithm, &expected, verify)
        .map_err(|e| reported_at(e, output))?;
    staged.commit(output)
}

/// Compress a whole directory tree into an archive.
///
/// Entries keep their paths relative to (and prefixed with) the directory's
/// own name, producing a standard archive other tools can read. As with
/// [`compress`], `level` must be 1–5; `tar` ignores it, the archive is staged
/// beside `output` and renamed in only once it passes at `verify`'s depth, and
/// the backend functions do neither.
pub fn compress_dir(
    source_dir: &Path,
    output: &Path,
    algorithm: Algorithm,
    level: u32,
    verify: Verify,
) -> Result<(), CompressionError> {
    if !(1..=5).contains(&level) {
        return Err(CompressionError::InvalidLevel(level));
    }
    // The tree is walked here as well as inside the backend. The backends take
    // a path and are called directly from elsewhere in the workspace, so their
    // signatures are not ours alone to change, and the dispatcher needs the
    // same list to know what the archive was meant to hold.
    let expected: Vec<String> = walk_tree(source_dir)?
        .into_iter()
        .map(|entry| entry.archive_name)
        .collect();

    let staged = StagedOutput::beside(output)?;
    match algorithm {
        Algorithm::SevenZ => compress_7z_dir(source_dir, staged.path(), level),
        Algorithm::Tar => compress_tar_dir(source_dir, staged.path()),
        Algorithm::Zip => compress_zip_dir(source_dir, staged.path(), level),
    }?;
    verify_archive(staged.path(), algorithm, &expected, verify)
        .map_err(|e| reported_at(e, output))?;
    staged.commit(output)
}

/// Extract an archive into `output_dir`.
///
/// Returns the list of extracted file paths (relative to `output_dir`).
/// The algorithm is detected from the archive file extension.
pub fn extract(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    let ext = archive.extension().and_then(|e| e.to_str()).unwrap_or("");

    let algorithm = Algorithm::from_extension(ext)
        .ok_or_else(|| CompressionError::Failed(format!("Unknown archive extension: .{ext}")))?;

    match algorithm {
        Algorithm::SevenZ => extract_7z(archive, output_dir),
        Algorithm::Tar => extract_tar(archive, output_dir),
        Algorithm::Zip => extract_zip(archive, output_dir),
    }
}
