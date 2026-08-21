mod algorithm;
mod sevenz;
mod tar;
mod walk;
mod zip;

pub use self::algorithm::Algorithm;
pub use self::sevenz::{compress_7z, compress_7z_dir, extract_7z};
pub use self::tar::{compress_tar, compress_tar_dir, extract_tar};
pub use self::zip::{compress_zip, compress_zip_dir, extract_zip};

pub(crate) use self::walk::walk_tree;

use std::path::{Component, Path, PathBuf};

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
}

/// Compress a file using the given algorithm and level (1–5).
///
/// The file is stored inside the archive under `arcname`.
/// `tar` archives without compressing, so it ignores the level
/// (which must still be in range).
pub fn compress(
    source: &Path,
    output: &Path,
    arcname: &str,
    algorithm: Algorithm,
    level: u32,
) -> Result<(), CompressionError> {
    if !(1..=5).contains(&level) {
        return Err(CompressionError::InvalidLevel(level));
    }
    match algorithm {
        Algorithm::SevenZ => compress_7z(source, output, arcname, level),
        Algorithm::Tar => compress_tar(source, output, arcname),
        Algorithm::Zip => compress_zip(source, output, arcname, level),
    }
}

/// Compress a whole directory tree into an archive.
///
/// Entries keep their paths relative to (and prefixed with) the directory's
/// own name, producing a standard archive other tools can read. As with
/// [`compress`], `level` must be 1–5; `tar` ignores it.
pub fn compress_dir(
    source_dir: &Path,
    output: &Path,
    algorithm: Algorithm,
    level: u32,
) -> Result<(), CompressionError> {
    if !(1..=5).contains(&level) {
        return Err(CompressionError::InvalidLevel(level));
    }
    match algorithm {
        Algorithm::SevenZ => compress_7z_dir(source_dir, output, level),
        Algorithm::Tar => compress_tar_dir(source_dir, output),
        Algorithm::Zip => compress_zip_dir(source_dir, output, level),
    }
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
