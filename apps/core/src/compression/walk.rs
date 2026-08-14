use std::fs;
use std::path::{Path, PathBuf};

use super::CompressionError;

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
    let mut children: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
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
