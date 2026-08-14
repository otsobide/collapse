use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};

use tar::{Archive, Builder, EntryType};

use super::CompressionError;

/// tar is an archive container without compression, so there is no level.
pub fn compress_tar(source: &Path, output: &Path, arcname: &str) -> Result<(), CompressionError> {
    // Open the source before creating the output so a missing source
    // does not leave an empty archive behind.
    let mut source_file = File::open(source)?;

    let output_file = File::create(output)?;
    let mut builder = Builder::new(output_file);

    builder
        .append_file(arcname, &mut source_file)
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    builder
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(())
}

/// Archive a whole directory tree into a standard tar file.
///
/// Entries are stored relative to (and prefixed with) the directory's own
/// name, so `photos/` yields `photos/a.jpg`, `photos/sub/b.jpg`, … — the same
/// layout `tar` and other tools produce, and round-trips back via `extract`.
///
/// Driven by the shared [`walk_tree`](super::walk_tree), so symlinks are
/// skipped (same as zip/7z) — the archive never carries a symlink that could
/// point outside the tree.
pub fn compress_tar_dir(source_dir: &Path, output: &Path) -> Result<(), CompressionError> {
    let entries = super::walk_tree(source_dir)?;

    let output_file = File::create(output)?;
    let mut builder = Builder::new(output_file);

    for entry in entries {
        if entry.is_dir {
            builder
                .append_dir(&entry.archive_name, &entry.disk_path)
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
        } else {
            let mut file = File::open(&entry.disk_path)?;
            builder
                .append_file(&entry.archive_name, &mut file)
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
        }
    }

    builder
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(())
}

pub fn extract_tar(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    fs::create_dir_all(output_dir)?;
    let canonical_output = output_dir.canonicalize()?;

    let file = File::open(archive)?;
    let mut ar = Archive::new(file);

    let mut extracted = Vec::new();
    let entries = ar
        .entries()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| CompressionError::Failed(e.to_string()))?;
        let name = entry
            .path()
            .map_err(|e| CompressionError::Failed(e.to_string()))?
            .to_string_lossy()
            .to_string();

        // Only ever materialize regular files and directories. Symlinks,
        // hardlinks and special nodes are skipped so extraction never plants
        // an outbound link in the output tree — the same "no links" guarantee
        // zip/7z give (they write link entries as regular files).
        let entry_type = entry.header().entry_type();
        if entry_type != EntryType::Regular && entry_type != EntryType::Directory {
            continue;
        }

        // unpack_in refuses entries whose path would escape the output dir.
        let unpacked = entry
            .unpack_in(&canonical_output)
            .map_err(|e| CompressionError::Failed(e.to_string()))?;
        if !unpacked {
            return Err(CompressionError::Failed(format!(
                "Path traversal detected in archive entry: {name}"
            )));
        }

        if entry_type == EntryType::Regular {
            // The traversal guard is unpack_in itself (it refuses `..` and
            // strips root/cur-dir before writing). Report the path it actually
            // wrote by keeping only the normal components, relative to output_dir.
            let written: PathBuf = Path::new(&name)
                .components()
                .filter(|c| matches!(c, Component::Normal(_)))
                .collect();
            if !written.as_os_str().is_empty() {
                extracted.push(written.to_string_lossy().to_string());
            }
        }
    }
    Ok(extracted)
}
