//! Path predicates the front ends need before they write an archive.
//!
//! They live here, in the engine both front ends already depend on, because
//! they existed twice and drifted: the desktop compared filesystem identity
//! but only on Unix, and the CLI compared resolved paths and nothing else, so
//! `--force` destroyed its own source through a hardlink on every platform.
//! One copy cannot disagree with itself.
//!
//! The rule they encode is that **comparing paths is not comparing files**. A
//! hardlink is not a pointer to a file, it *is* a name of that file, so two
//! hardlinks resolve to two different paths on every operating system. Only
//! the filesystem's own identity (device and inode on Unix, volume serial and
//! file index on Windows) tells them apart, and getting that wrong here costs
//! the user their data: every backend creates the output before, or while,
//! reading the source.

use std::path::{Path, PathBuf};

/// True when both paths resolve to the same existing file.
///
/// Three layers, cheapest first:
///
/// 1. Resolved-path equality, which folds `.`, `..`, duplicate separators and
///    symlinks, and needs no permission to read the file.
/// 2. Filesystem identity, which catches what paths cannot. This compares
///    device and inode on Unix and volume serial plus file index on Windows,
///    opening a handle for each (a directory handle too, which the guard needs
///    because a folder can be the source).
/// 3. On Unix, the same comparison through `stat` rather than an open handle.
///    Layer 2 has to open both paths, and a write-only file (mode `0o222`)
///    refuses that. Such a file is unreadable but still truncatable, which is
///    exactly the shape that loses data, so `stat` answers where `open` will
///    not.
///
/// A path that does not exist is never "the same file": there is nothing to
/// resolve, so writing a brand new archive is always allowed.
pub fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) else {
        return false;
    };
    if a == b {
        return true;
    }
    if let Ok(same) = same_file::is_same_file(&a, &b) {
        return same;
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

/// True when `candidate` is a file the archiving of `dir` would read: either
/// it sits inside `dir` by path, or it is another name for something that
/// does.
///
/// The second half is the whole point. A hardlink placed outside the folder
/// still shares an inode with a file inside it, so truncating the link
/// truncates the member, and a path-only check waves it through because the
/// link's path is nowhere near the folder.
///
/// Only meaningful for a `candidate` that already exists, which is exactly
/// when it matters: a name nothing occupies yet cannot be truncated, and the
/// callers check that first. The walk it does in the worst case is therefore
/// paid only when overwriting an existing file with a folder as the source,
/// and it stops at the first match.
pub fn inside(dir: &Path, candidate: &Path) -> bool {
    let (Ok(dir), Ok(candidate)) = (dir.canonicalize(), candidate.canonicalize()) else {
        return false;
    };
    if candidate != dir && candidate.starts_with(&dir) {
        return true;
    }
    shares_a_file_with_tree(&dir, &candidate)
}

/// Depth-first search for a member of `dir` that is the same file as
/// `candidate`. Symlinked children are not followed, matching the walk the
/// backends use to build the archive: a link is never read, so it can never be
/// the file that gets truncated. Unreadable directories are skipped rather
/// than reported, because a guard that fails closed on a permission error
/// would refuse compressions that are perfectly safe.
fn shares_a_file_with_tree(dir: &Path, candidate: &Path) -> bool {
    let mut pending: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(children) = std::fs::read_dir(&current) else {
            continue;
        };
        for child in children.flatten() {
            let Ok(kind) = child.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = child.path();
            if kind.is_dir() {
                pending.push(path);
            } else if same_file(&path, candidate) {
                return true;
            }
        }
    }
    false
}
