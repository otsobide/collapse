//! Tests for `collapse_desktop::paths::same_file`, the predicate that stands
//! between a user and irrecoverable data loss: `compress_path` refuses to run
//! when the archive it is about to write resolves to the source it is about to
//! read, because every backend creates (and therefore truncates) the output
//! before the source has been read. A false negative here destroys the only
//! copy of the user's data, so the predicate is tested from both sides: it must
//! say "same" for every spelling, symlink and hardlink that reaches one file,
//! and it must say "different" for anything that does not, including paths that
//! do not exist yet (otherwise no new archive could ever be written).
//!
//! Every assertion is made in both argument orders: the caller passes
//! (source, output), and a predicate that is only half right would let the
//! guard depend on which side the dangerous path happens to be on.

use std::path::{Path, PathBuf};

use collapse_desktop::paths::same_file;
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers --

fn write(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent directory");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

/// Assert the two paths are the same file, whichever way round they are given.
fn assert_same(a: &Path, b: &Path, why: &str) {
    assert!(same_file(a, b), "{why}: {a:?} vs {b:?}");
    assert!(
        same_file(b, a),
        "{why} (arguments swapped): {b:?} vs {a:?}"
    );
}

/// Assert the two paths are NOT the same file, whichever way round they are
/// given. A false positive is less dangerous than a false negative, but it
/// still blocks a legitimate compression, so it is pinned too.
fn assert_not_same(a: &Path, b: &Path, why: &str) {
    assert!(!same_file(a, b), "{why}: {a:?} vs {b:?}");
    assert!(
        !same_file(b, a),
        "{why} (arguments swapped): {b:?} vs {a:?}"
    );
}

/// A relative spelling of an absolute path, resolved against the process's
/// current directory. Built by walking up from the cwd rather than by changing
/// it: cargo runs the tests of one binary in parallel threads that share a
/// single cwd, so `set_current_dir` would make every other test in this file
/// racy.
#[cfg(unix)]
fn relative_from_cwd(target: &Path) -> PathBuf {
    let cwd = std::env::current_dir().expect("current directory");
    assert!(cwd.is_absolute(), "getcwd must return an absolute path");
    // getcwd returns a fully resolved path (no symlink components), so one
    // `..` per component walks back to the root without surprises.
    let mut rel = PathBuf::new();
    for _ in cwd.components().skip(1) {
        rel.push("..");
    }
    rel.push(target.strip_prefix("/").expect("absolute target path"));
    rel
}

// ----------------------------------------------------------------- identity --

#[test]
fn a_file_is_the_same_file_as_itself() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    assert_same(&file, &file, "a path must match itself");
}

#[test]
fn a_directory_is_the_same_as_itself() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder).unwrap();

    // `compress_path` accepts directories as sources, so the guard has to work
    // for them as well as for files.
    assert_same(&folder, &folder, "a directory must match itself");
}

#[test]
fn a_directory_path_with_a_trailing_separator_still_matches_itself() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder).unwrap();

    // A path coming back from a native folder picker may or may not carry the
    // trailing separator; the guard must not depend on that.
    let with_slash = PathBuf::from(format!("{}{}", folder.display(), std::path::MAIN_SEPARATOR));
    assert_same(
        &folder,
        &with_slash,
        "a trailing separator must not change the identity of a directory",
    );
}

// ---------------------------------------------------------------- spellings --

#[test]
fn the_same_file_reached_through_a_dot_component_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    let dotted = dir.path().join(".").join("notes.txt");
    assert_same(&file, &dotted, "`./name` must resolve to `name`");
}

#[test]
fn the_same_file_reached_through_a_parent_component_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    std::fs::create_dir(dir.path().join("sub")).unwrap();

    // `dir/sub/../notes.txt` is the classic way a hand-typed or concatenated
    // output path sneaks back onto its own source.
    let round_trip = dir.path().join("sub").join("..").join("notes.txt");
    assert_same(
        &file,
        &round_trip,
        "`sub/../name` must resolve back to `name`",
    );
}

#[test]
fn the_same_file_reached_through_redundant_separators_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    let doubled = PathBuf::from(format!(
        "{}{}{}notes.txt",
        dir.path().display(),
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR
    ));
    assert_same(
        &file,
        &doubled,
        "repeated separators must not change the identity of a file",
    );
}

#[cfg(unix)]
#[test]
fn an_absolute_and_a_relative_spelling_of_one_file_are_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    // The webview always hands over absolute paths today, but the predicate is
    // a plain path function and nothing in its signature says so; a relative
    // output would have to be caught just the same.
    let relative = relative_from_cwd(&file);
    assert!(relative.is_relative(), "fixture must be a relative path");
    assert_same(
        &file,
        &relative,
        "a relative spelling resolved from the cwd must match the absolute one",
    );
}

#[cfg(unix)]
#[test]
fn a_temp_dir_path_matches_its_fully_resolved_form() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    // On macOS the temp dir lives under `/var/...`, which is itself a symlink
    // to `/private/var/...`. This is the everyday case where the two paths a
    // user's dialogs produce differ as strings but name one file, and it is
    // exactly what canonicalization is there for.
    let resolved = file.canonicalize().unwrap();
    assert_same(
        &file,
        &resolved,
        "a path containing a symlinked ancestor must match its resolved form",
    );
}

// -------------------------------------------------------------------- links --

#[cfg(unix)]
#[test]
fn a_symlink_to_a_file_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let link = dir.path().join("shortcut.txt");
    std::os::unix::fs::symlink(&file, &link).unwrap();

    assert_same(&file, &link, "a symlink must resolve to its target");
}

#[cfg(unix)]
#[test]
fn a_chain_of_symlinks_to_a_file_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let first = dir.path().join("one.txt");
    let second = dir.path().join("two.txt");
    std::os::unix::fs::symlink(&file, &first).unwrap();
    std::os::unix::fs::symlink(&first, &second).unwrap();

    // Resolution has to follow the whole chain, not just one hop.
    assert_same(&file, &second, "a symlink chain must resolve to its target");
}

#[cfg(unix)]
#[test]
fn two_symlinks_to_one_file_are_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    std::os::unix::fs::symlink(&file, &a).unwrap();
    std::os::unix::fs::symlink(&file, &b).unwrap();

    assert_same(&a, &b, "two symlinks to one target must match each other");
}

#[cfg(unix)]
#[test]
fn a_symlink_to_a_directory_is_the_same_directory() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder).unwrap();
    let link = dir.path().join("pictures");
    std::os::unix::fs::symlink(&folder, &link).unwrap();

    assert_same(
        &folder,
        &link,
        "a symlink to a directory must resolve to that directory",
    );
}

#[cfg(unix)]
#[test]
fn a_hardlink_to_a_file_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let link = dir.path().join("also-notes.txt");
    std::fs::hard_link(&file, &link).unwrap();

    // This is the whole reason the predicate compares inode and device rather
    // than stopping at the resolved path: a hardlink canonicalizes to its own
    // distinct path, so a pure string comparison would call these two files
    // different and let the archive truncate the user's data.
    assert_ne!(
        file.canonicalize().unwrap(),
        link.canonicalize().unwrap(),
        "the fixture is pointless unless the two hardlinks canonicalize apart"
    );
    assert_same(
        &file,
        &link,
        "two hardlinks to one inode must be the same file",
    );
}

#[cfg(unix)]
#[test]
fn a_hardlink_in_another_directory_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("source").join("notes.txt");
    write(&file, b"hello");
    let elsewhere = dir.path().join("output");
    std::fs::create_dir(&elsewhere).unwrap();
    let link = elsewhere.join("archive.zip");
    std::fs::hard_link(&file, &link).unwrap();

    // Same inode, different directory, different name and different extension:
    // nothing about the two spellings hints they are one file.
    assert_same(
        &file,
        &link,
        "a hardlink is the same file no matter where it lives",
    );
}

#[cfg(unix)]
#[test]
fn a_broken_symlink_is_never_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let dangling = dir.path().join("dangling.txt");
    std::os::unix::fs::symlink(dir.path().join("gone.txt"), &dangling).unwrap();

    // A dangling link cannot be canonicalized, so it falls into the "nothing to
    // resolve" branch. That is the documented behaviour and it is safe here:
    // the link's target does not exist, so writing through it creates a new
    // file rather than truncating an existing one.
    assert_not_same(
        &file,
        &dangling,
        "a broken symlink resolves to nothing and cannot match a real file",
    );
    assert_not_same(
        &dangling,
        &dangling,
        "a broken symlink does not even match itself",
    );
}

// ---------------------------------------------------------- distinct files --

#[test]
fn two_files_with_identical_content_are_not_the_same_file() {
    let dir = TempDir::new().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    write(&a, b"identical bytes");
    write(&b, b"identical bytes");

    // The predicate is about identity, not equality: compressing one file into
    // an archive that happens to have the same bytes as another must be allowed.
    assert_not_same(&a, &b, "equal content does not make one file");
}

#[test]
fn two_empty_files_are_not_the_same_file() {
    let dir = TempDir::new().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    write(&a, b"");
    write(&b, b"");

    // Zero-length files are the degenerate case of identical content, and the
    // one most likely to fool a size-or-content based shortcut.
    assert_not_same(&a, &b, "two empty files are still two files");
}

#[test]
fn the_same_name_in_two_directories_is_not_the_same_file() {
    let dir = TempDir::new().unwrap();
    let a = dir.path().join("one").join("notes.txt");
    let b = dir.path().join("two").join("notes.txt");
    write(&a, b"hello");
    write(&b, b"hello");

    assert_not_same(&a, &b, "a shared file name does not make one file");
}

#[test]
fn a_file_and_a_directory_are_not_the_same() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder).unwrap();

    assert_not_same(&file, &folder, "a file is never a directory");
}

#[test]
fn a_file_is_not_the_same_as_the_directory_that_contains_it() {
    let dir = TempDir::new().unwrap();
    let folder = dir.path().join("photos");
    let file = folder.join("notes.txt");
    write(&file, b"hello");

    // Compressing a folder into an archive placed inside it is allowed (and
    // odd, but not destructive), so containment must not read as identity.
    assert_not_same(
        &folder,
        &file,
        "a directory is not the same as its own child",
    );
}

#[test]
fn a_new_archive_beside_its_source_is_not_the_same_file() {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    write(&source, b"hello");
    let archive = dir.path().join("notes.zip");

    // The ordinary happy path of the app: the guard must stay out of its way,
    // and the source must still be there afterwards.
    assert_not_same(
        &source,
        &archive,
        "the default output beside the source must be allowed",
    );
    assert_eq!(std::fs::read(&source).unwrap(), b"hello");
    assert!(!archive.exists(), "the predicate must not create anything");
}

// ------------------------------------------------------------ missing paths --

#[test]
fn a_path_that_does_not_exist_is_never_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let missing = dir.path().join("notes.zip");
    assert!(!missing.exists(), "fixture must not exist");

    // Load bearing rather than a detail: every first-time compression writes to
    // a path that does not exist yet, so if this ever returned true the app
    // could not produce a single archive. Both argument positions are checked
    // because `compress_path` passes (source, output) and only the output is
    // normally the missing one.
    assert_not_same(
        &file,
        &missing,
        "a path with nothing behind it cannot match an existing file",
    );
}

#[test]
fn neither_path_existing_is_not_the_same_file() {
    let dir = TempDir::new().unwrap();
    let a = dir.path().join("gone-a.txt");
    let b = dir.path().join("gone-b.txt");

    assert_not_same(&a, &b, "two paths that resolve to nothing do not match");
}

#[test]
fn one_missing_path_does_not_match_itself() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("gone.txt");

    // Textual equality is not enough: with nothing on disk there is no file to
    // be the same as, and the guard must let the write through.
    assert_not_same(
        &missing,
        &missing,
        "an identical spelling of a missing path is still not a file",
    );
}

#[test]
fn a_path_under_a_missing_directory_is_not_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    // Lexically `gone/../notes.txt` is `notes.txt`, but resolution needs every
    // component to exist, so the predicate says "different". That is not a hole
    // in the guard: the OS refuses to open such a path for the same reason, so
    // the write fails instead of truncating the source.
    let through_missing = dir.path().join("gone").join("..").join("notes.txt");
    assert_not_same(
        &file,
        &through_missing,
        "a path whose parent does not exist cannot be resolved",
    );
    assert!(
        std::fs::File::create(&through_missing).is_err(),
        "the OS must also refuse this path, otherwise the guard has a hole"
    );
}

#[test]
fn an_empty_path_is_not_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    // An empty string is what an unset field in the UI would send; it must not
    // resolve to the current directory or to anything else.
    let empty = PathBuf::new();
    assert_not_same(&file, &empty, "an empty path resolves to nothing");
    assert_not_same(&empty, &empty, "two empty paths still resolve to nothing");
}

#[test]
fn a_deleted_file_stops_matching_itself() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    assert_same(&file, &file, "sanity: the fixture matches while it exists");

    std::fs::remove_file(&file).unwrap();

    // The predicate reads the filesystem on every call rather than caching, so
    // the answer follows what is actually on disk right now.
    assert_not_same(
        &file,
        &file,
        "a path stops being a file the moment the file is gone",
    );
}
