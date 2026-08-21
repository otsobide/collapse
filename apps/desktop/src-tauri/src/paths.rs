//! Pure path predicates, split out of the commands so they can be tested
//! without a Tauri runtime (the same split as the frontend's `src/paths.js`
//! and the server backend's `validate.rs`).

use std::path::Path;

/// True when both paths resolve to the same existing file: by resolved path
/// (symlinks, `.`/`..`) and, on Unix, by inode/device so two hardlinks to the
/// same file are also caught.
///
/// Canonicalization is what makes this reliable, and it is also why a path
/// that does not exist yet is never "the same file": there is nothing to
/// resolve, so writing a brand new archive is always allowed.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) else {
        return false;
    };
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(&a), std::fs::metadata(&b)) {
            return ma.ino() == mb.ino() && ma.dev() == mb.dev();
        }
    }
    false
}

/// True when `candidate` resolves to something strictly inside `dir`.
///
/// Only meaningful for paths that exist, which is exactly when it matters: an
/// archive aimed at a file that is part of the tree being archived destroys it
/// and stores its own header bytes under that entry, because the backends list
/// the tree before creating the output. A name nothing occupies yet cannot be
/// harmed that way, so this answers `false` for it and lets it through.
pub fn inside(dir: &Path, candidate: &Path) -> bool {
    let (Ok(dir), Ok(candidate)) = (dir.canonicalize(), candidate.canonicalize()) else {
        return false;
    };
    candidate != dir && candidate.starts_with(&dir)
}
