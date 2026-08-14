mod sevenz;
mod tar;
mod zip;

pub use self::sevenz::{compress_7z, extract_7z};
pub use self::tar::{compress_tar, compress_tar_dir, extract_tar};
pub use self::zip::{compress_zip, compress_zip_dir, extract_zip};

use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One node of a directory tree to be archived.
pub(crate) struct TreeEntry {
    /// Path inside the archive: forward-slash separated and prefixed with the
    /// source directory's own name (e.g. `photos/sub/a.txt`).
    pub archive_name: String,
    /// Where the node lives on disk.
    pub disk_path: PathBuf,
    pub is_dir: bool,
}

/// Walk a directory into a deterministic, depth-first list of entries whose
/// names are prefixed with the directory's own name (the `tar`/`zip -r`
/// convention). Symlinks are skipped (never followed) and children are sorted
/// for reproducible archives. Backends turn this into format-specific entries.
pub(crate) fn walk_tree(source_dir: &Path) -> Result<Vec<TreeEntry>, CompressionError> {
    if !source_dir.is_dir() {
        return Err(CompressionError::Failed(format!(
            "Not a directory: {}",
            source_dir.display()
        )));
    }
    let root = source_dir
        .file_name()
        .ok_or_else(|| {
            CompressionError::Failed("Cannot determine directory name to archive".to_string())
        })?
        .to_string_lossy()
        .to_string();

    let mut out = vec![TreeEntry {
        archive_name: root.clone(),
        disk_path: source_dir.to_path_buf(),
        is_dir: true,
    }];
    walk_into(source_dir, &root, &mut out)?;
    Ok(out)
}

fn walk_into(dir: &Path, prefix: &str, out: &mut Vec<TreeEntry>) -> Result<(), CompressionError> {
    let mut children: Vec<_> = std::fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    children.sort_by_key(|e| e.file_name());
    for child in children {
        let file_type = child.file_type()?;
        if file_type.is_symlink() {
            continue; // do not follow or store symlinks
        }
        let archive_name = format!("{prefix}/{}", child.file_name().to_string_lossy());
        let disk_path = child.path();
        if file_type.is_dir() {
            out.push(TreeEntry {
                archive_name: archive_name.clone(),
                disk_path: disk_path.clone(),
                is_dir: true,
            });
            walk_into(&disk_path, &archive_name, out)?;
        } else if file_type.is_file() {
            out.push(TreeEntry {
                archive_name,
                disk_path,
                is_dir: false,
            });
        }
    }
    Ok(())
}

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

/// Supported compression algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Algorithm {
    #[serde(rename = "7z")]
    SevenZ,
    #[serde(rename = "tar")]
    Tar,
    #[serde(rename = "zip")]
    Zip,
}

impl Algorithm {
    /// File extension for archives produced by this algorithm.
    pub fn extension(&self) -> &str {
        match self {
            Algorithm::SevenZ => "7z",
            Algorithm::Tar => "tar",
            Algorithm::Zip => "zip",
        }
    }

    /// MIME type for archives produced by this algorithm.
    pub fn media_type(&self) -> &str {
        match self {
            Algorithm::SevenZ => "application/x-7z-compressed",
            Algorithm::Tar => "application/x-tar",
            Algorithm::Zip => "application/zip",
        }
    }
}

impl fmt::Display for Algorithm {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.extension())
    }
}

impl FromStr for Algorithm {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "7z" => Ok(Algorithm::SevenZ),
            "tar" => Ok(Algorithm::Tar),
            "zip" => Ok(Algorithm::Zip),
            other => Err(format!("Unknown algorithm: {other}")),
        }
    }
}

impl Algorithm {
    /// Try to detect the algorithm from a file extension.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "7z" => Some(Algorithm::SevenZ),
            "tar" => Some(Algorithm::Tar),
            "zip" => Some(Algorithm::Zip),
            _ => None,
        }
    }
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
///
/// Only `tar` is supported for now; other formats return an error.
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
        Algorithm::Tar => compress_tar_dir(source_dir, output),
        Algorithm::Zip => compress_zip_dir(source_dir, output, level),
        other => Err(CompressionError::Failed(format!(
            "Directory compression is not yet supported for {other}"
        ))),
    }
}

/// Extract an archive into `output_dir`.
///
/// Returns the list of extracted file paths (relative to `output_dir`).
/// The algorithm is detected from the archive file extension.
pub fn extract(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    let ext = archive
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let algorithm = Algorithm::from_extension(ext).ok_or_else(|| {
        CompressionError::Failed(format!("Unknown archive extension: .{ext}"))
    })?;

    match algorithm {
        Algorithm::SevenZ => extract_7z(archive, output_dir),
        Algorithm::Tar => extract_tar(archive, output_dir),
        Algorithm::Zip => extract_zip(archive, output_dir),
    }
}
