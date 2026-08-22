//! Tests for `collapse_desktop::paths::same_file`, the predicate that stands
//! between a user and irrecoverable data loss: `compress_path` refuses to run
//! when the archive it is about to write resolves to the source it is about to
//! read. Letting that pair through is not survivable, and each backend loses
//! the data its own way (all three measured against `collapse_core::compress`
//! with `output == source`): `compress_zip` creates the archive before it opens
//! the source, so the entry it stores is the zip's own freshly written header
//! bytes instead of the user's file; `compress_tar` creates the output before
//! `append_file` reads a byte, so the file feeds its own growing archive and
//! the call never returns (21 GB written in two minutes before the probe was
//! killed); `compress_7z` alone reads the whole source into memory first, so
//! the content survives inside the archive, but the file the user picked is
//! still replaced by it. So the predicate is tested from both sides: it must
//! say "same" for every spelling, symlink and hardlink that reaches one file,
//! and it must say "different" for anything that does not, including paths that
//! do not exist yet (otherwise no new archive could ever be written).
//!
//! Every assertion is made in both argument orders, and the swap is coverage
//! rather than decoration. `same_file` as written is symmetric (it resolves
//! both sides the same way, then compares the results), so no failure of the
//! shipped code depends on the order; the swap is aimed at the refactor that
//! would break that symmetry, one resolving only one side (canonicalizing the
//! output, say, and comparing it against the source exactly as it was typed).
//! Measured against precisely that mutant, two tests here fail on the swapped
//! call alone: `a_path_through_a_symlinked_ancestor_matches_its_resolved_form`
//! and `a_symlink_is_the_same_as_its_target_for_files_and_directories`. Both
//! are shapes where one spelling resolves to the other side's literal path, so
//! the half-right predicate answers correctly in one direction only. Note it is
//! not the mixed-kind cases that catch it: a real file against a dangling link,
//! or against a path that does not exist, survives that mutant in both orders.

use std::path::{Path, PathBuf, MAIN_SEPARATOR};

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
    assert!(same_file(b, a), "{why} (arguments swapped): {b:?} vs {a:?}");
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
fn a_file_or_a_directory_is_the_same_as_itself() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder).unwrap();

    // `compress_path` takes directories as sources too, and `same_file` never
    // branches on the kind of thing a path names (canonicalize and metadata
    // treat both alike), so one table says it for both.
    for path in [&file, &folder] {
        assert_same(path, path, "a path must match itself");
    }
}

// ---------------------------------------------------------------- spellings --

#[test]
fn the_spellings_that_path_equality_already_folds_are_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder).unwrap();

    // Honest scope: the `assert_eq!` pins `std::path::Path`'s normalization,
    // NOT the canonicalization inside `same_file`. `Path` compares component by
    // component, so a `.` component, a repeated separator and a trailing
    // separator are gone before `same_file` is even called; each case below
    // survives a `same_file` reduced to `a == b`, and the `assert_eq!` is what
    // says so out loud and what fails if std ever stops folding them. The
    // `assert_same` is not idle either: it kills the other cheap shortcut, a
    // predicate that dropped canonicalization and compared the caller's raw
    // strings, for which all three spellings differ (measured). The spelling
    // with teeth against `a == b` is `..`, which `Path` does not fold: see the
    // next test.
    let dotted = dir.path().join(".").join("notes.txt");
    let doubled = PathBuf::from(format!(
        "{}{MAIN_SEPARATOR}{MAIN_SEPARATOR}notes.txt",
        dir.path().display()
    ));
    // A path coming back from a native folder picker may or may not carry the
    // trailing separator, and this is why that cannot break the guard.
    let trailing = PathBuf::from(format!("{}{MAIN_SEPARATOR}", folder.display()));

    for (spelling, target, what) in [
        (dotted, &file, "a `.` component"),
        (doubled, &file, "a repeated separator"),
        (trailing, &folder, "a trailing separator"),
    ] {
        assert_eq!(
            spelling.as_path(),
            target.as_path(),
            "{what}: `Path` equality is expected to fold this spelling by itself"
        );
        assert_same(
            &spelling,
            target,
            &format!("{what} must not change the identity of a path"),
        );
    }
}

#[test]
fn the_same_file_reached_through_a_parent_component_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    std::fs::create_dir(dir.path().join("sub")).unwrap();

    // `dir/sub/../notes.txt` is the classic way a hand-typed or concatenated
    // output path sneaks back onto its own source, and unlike `.` or a doubled
    // separator it is a spelling `Path` refuses to fold on its own (it cannot:
    // `sub` could be a symlink). Resolving it is the predicate's own work, so
    // deleting the canonicalization fails this test.
    let round_trip = dir.path().join("sub").join("..").join("notes.txt");
    assert_ne!(
        round_trip.as_path(),
        file.as_path(),
        "the fixture is pointless unless the two spellings differ lexically"
    );
    assert_same(
        &file,
        &round_trip,
        "`sub/../name` must resolve back to `name`",
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
fn a_path_through_a_symlinked_ancestor_matches_its_resolved_form() {
    let dir = TempDir::new().unwrap();
    let real = dir.path().join("real");
    let file = real.join("notes.txt");
    write(&file, b"hello");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    // The everyday case this predicate exists for: two paths that differ as
    // strings and name one file, because a directory along the way is a link.
    // On macOS the temp dir itself is one (`/var` -> `/private/var`), but the
    // Linux CI runner's is not, and leaning on the ambient temp dir there would
    // silently degrade this into comparing a path with itself. Hence the
    // hand-built fixture and the `assert_ne!` that proves it is not degenerate.
    let through_link = link.join("notes.txt");
    assert_ne!(
        through_link,
        through_link.canonicalize().unwrap(),
        "the fixture is pointless unless the spelling differs from its resolved form"
    );
    assert_same(
        &through_link,
        &file,
        "a symlinked ancestor must be resolved away",
    );
}

#[test]
fn a_case_differing_spelling_follows_the_filesystem() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let shouted = dir.path().join("NOTES.TXT");

    // Probed at runtime rather than gated by `cfg`, because the answer belongs
    // to the volume and not to the OS: APFS and NTFS are case-insensitive by
    // default, ext4 is not, and the desktop app ships on all three.
    if shouted.exists() {
        // One file under two spellings, and the guard has to see through it:
        // a user who picks `notes.txt` and types `NOTES.TXT` as the output
        // would otherwise watch the source be truncated. Resolution is what
        // catches it rather than the inode/device fallback, because macOS
        // reports the name as it is stored on disk (measured: both spellings
        // canonicalize to `.../notes.txt`), so a predicate that compared the
        // caller's strings would fail here.
        assert_same(
            &file,
            &shouted,
            "a case-insensitive volume reaches one file through both spellings",
        );
    } else {
        // Case-sensitive volume (this is the branch CI takes): the shouted
        // spelling names nothing at all, so the assertion below is the
        // missing-path case again and has no teeth of its own here.
        assert_not_same(&file, &shouted, "a case-sensitive volume has no such file");
    }
}

// -------------------------------------------------------------------- links --

#[cfg(unix)]
#[test]
fn a_symlink_is_the_same_as_its_target_for_files_and_directories() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder).unwrap();
    let to_file = dir.path().join("shortcut.txt");
    let to_folder = dir.path().join("pictures");
    std::os::unix::fs::symlink(&file, &to_file).unwrap();
    std::os::unix::fs::symlink(&folder, &to_folder).unwrap();

    // Both kinds in one table for the same reason as the identity test: the
    // predicate has no file-type branch, so the directory case cannot fail
    // while the file case passes.
    for (link, target) in [(&to_file, &file), (&to_folder, &folder)] {
        assert_same(link, target, "a symlink must resolve to its target");
    }
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

    // Resolution has to follow the whole chain, not just one hop: a predicate
    // that reads a single `read_link` and compares the results passes the
    // single-link case above and fails here (measured, not assumed).
    assert_same(&file, &second, "a symlink chain must resolve to its target");
}

#[test]
fn a_hardlink_to_a_file_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let link = dir.path().join("also-notes.txt");
    // Not `cfg(unix)`: `hard_link` needs no privilege on Windows either, and
    // Windows is the platform where this used to answer wrongly, so gating it
    // would hide the fix exactly where it was missing. `hard_link` also panics
    // rather than falling back to a copy, so the fixture cannot degrade into
    // two unrelated files.
    std::fs::hard_link(&file, &link).unwrap();

    // This is the whole reason the predicate compares filesystem identity
    // rather than stopping at the resolved path: a hardlink canonicalizes to
    // its own distinct path, so a pure string comparison would call these two
    // files different and let the archive truncate the user's data.
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

#[test]
fn a_hardlink_in_another_directory_is_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("source").join("notes.txt");
    write(&file, b"hello");
    let elsewhere = dir.path().join("output");
    std::fs::create_dir(&elsewhere).unwrap();
    let link = elsewhere.join("archive.zip");
    std::fs::hard_link(&file, &link).unwrap();

    // Same file, different directory, different name and different extension:
    // nothing about the two spellings hints they are one file. This is the
    // shape a save dialog produces, which is why it is not `cfg(unix)`.
    assert_ne!(
        file.canonicalize().unwrap(),
        link.canonicalize().unwrap(),
        "the fixture is pointless unless the two hardlinks canonicalize apart"
    );
    assert_same(
        &file,
        &link,
        "a hardlink is the same file no matter where it lives",
    );
}

#[test]
fn a_hardlink_is_the_same_file_on_windows_too() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let link = dir.path().join("also-notes.zip");
    std::fs::hard_link(&file, &link).unwrap();

    // This is the reproduction that was destroying data, kept as its own test
    // because it is the one the defect was reported as: `notes.txt` and an
    // `alias.zip` a save dialog would happily hand back, one file under two
    // names. It replaces a `#[cfg(windows)]` test that asserted the opposite
    // and called it a known defect: the identity comparison used to be
    // `#[cfg(unix)]`, so Windows compared resolved paths only, `canonicalize`
    // there answers with the name the handle was opened with rather than one
    // canonical name per file, and the two names resolved apart. The guard
    // then let `compress_path` write the archive straight over the user's
    // source (see the module header for what each backend does to it) on the
    // platform the .msi and the NSIS installer ship to.
    //
    // Deliberately not gated: the assertion is the same claim on every
    // platform, and a `cfg` here is precisely how the Windows half went
    // unnoticed.
    assert_ne!(
        file.canonicalize().unwrap(),
        link.canonicalize().unwrap(),
        "the fixture is pointless unless the two names canonicalize apart"
    );
    assert_same(
        &file,
        &link,
        "an archive name hardlinked to the source is that source",
    );
}

#[cfg(unix)]
#[test]
fn a_hardlink_that_cannot_be_opened_for_reading_is_still_the_same_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let link = dir.path().join("also-notes.zip");
    std::fs::hard_link(&file, &link).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o222)).unwrap();

    // A write-only file is the shape that slips through an identity check
    // built on open handles: `same_file::is_same_file` opens both paths for
    // reading, which mode 0o222 refuses, so the predicate needs its `stat`
    // layer to answer at all. Unreadable is not unwritable, and the assertion
    // below says so out loud: this file can still be truncated, so a guard
    // that shrugged here would lose it. (Running the suite as root would let
    // the open succeed and the earlier layer answer instead, which costs this
    // test its teeth but not its result; no runner here does.)
    assert!(
        std::fs::File::options().write(true).open(&link).is_ok(),
        "the fixture must still be writable, otherwise there is nothing to lose"
    );
    assert_ne!(
        file.canonicalize().unwrap(),
        link.canonicalize().unwrap(),
        "the fixture is pointless unless the two hardlinks canonicalize apart"
    );
    assert_same(
        &file,
        &link,
        "a file that cannot be read can still be truncated, so it must be recognised",
    );
}

#[cfg(unix)]
#[test]
fn a_broken_symlink_matches_nothing_until_its_target_exists() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let gone = dir.path().join("gone.txt");
    let dangling = dir.path().join("dangling.txt");
    std::os::unix::fs::symlink(&gone, &dangling).unwrap();

    // A dangling link cannot be canonicalized, so it falls into the "nothing to
    // resolve" branch. That is safe: nothing exists behind it, so writing
    // through the link creates a file instead of truncating one.
    assert_not_same(
        &file,
        &dangling,
        "a broken symlink resolves to nothing and cannot match a real file",
    );

    // The half that pins the cause: it is the brokenness, not the link-ness,
    // that produced the "different" above. Create the target (which is exactly
    // what writing through the link would do) and the same two paths become one
    // file. A predicate that answered "different" whenever either side is a
    // symlink would pass the first assertion and fail this one.
    write(&gone, b"created later");
    assert_same(
        &gone,
        &dangling,
        "once its target exists, the link is that file",
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

    // Containment must not read as identity: an archive written into the very
    // folder being compressed has to reach the backend, and this predicate
    // answers about identity, not about clobbering. That request is not
    // harmless, mind: with this exact fixture `compress_path` returns Ok and
    // leaves `notes.txt` holding the archive instead of its own bytes
    // (measured, and pinned in `tests/commands.rs` as
    // `an_output_written_inside_the_source_tree_destroys_the_file_it_lands_on`).
    // Refusing it would take a clobber check `compress_path` does not have, not
    // a different answer from `same_file`.
    assert_not_same(
        &folder,
        &file,
        "a directory is not the same as its own child",
    );
}

// ------------------------------------------------------------ missing paths --

#[test]
fn a_path_that_does_not_exist_is_never_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");
    let missing = dir.path().join("notes.zip");
    assert!(!missing.exists(), "fixture must not exist");

    // This fixture is the app's ordinary happy path (an archive named after its
    // source, written beside it) and it is load bearing rather than a detail:
    // every first-time compression writes to a path that does not exist yet, so
    // if this ever returned true the app could not produce a single archive.
    // Both argument positions are checked because `compress_path` passes
    // (source, output) and only the output is normally the missing one. That
    // the write then really happens is `tests/commands.rs`'s business: this
    // predicate reads the filesystem and cannot touch it.
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

#[cfg(unix)]
#[test]
fn a_path_under_a_missing_directory_is_not_the_same_file() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    // Lexically `gone/../notes.txt` is `notes.txt`, but a Unix kernel resolves
    // a path component by component and needs every one of them to exist, so
    // the predicate says "different". That is not a hole in the guard: the OS
    // refuses to open such a path for the same reason, so the write fails
    // instead of truncating the source, and the assertion below is what says
    // the two refusals have the same cause. The error kind is asserted, not
    // merely "some error": a permission or name-too-long failure would not
    // support that claim. Windows folds the `..` before the filesystem ever
    // sees it and therefore answers the other way round: see the twin below.
    let through_missing = dir.path().join("gone").join("..").join("notes.txt");
    assert_not_same(
        &file,
        &through_missing,
        "a path whose parent does not exist cannot be resolved",
    );
    assert_eq!(
        std::fs::File::create(&through_missing)
            .expect_err("the OS must also refuse this path, otherwise the guard has a hole")
            .kind(),
        std::io::ErrorKind::NotFound,
        "the OS must refuse it for the same reason the predicate does"
    );
}

#[cfg(windows)]
#[test]
fn a_path_under_a_missing_directory_is_the_same_file_on_windows() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    write(&file, b"hello");

    // The twin of the test above, and the opposite answer, because the
    // platforms genuinely differ. Win32 normalizes a non-verbatim path
    // lexically before the object manager sees it, so `gone\..\notes.txt`
    // becomes `notes.txt` and the missing `gone` is never looked up: both
    // spellings canonicalize to the same file and the predicate must say so.
    // It has to, too, because here the OS really would open (and truncate)
    // the source through that spelling, which is exactly what the second
    // assertion proves. A `same_file` that answered "different" on this path,
    // as the Unix branch does, would be a hole in the guard on Windows only.
    let through_missing = dir.path().join("gone").join("..").join("notes.txt");
    assert_same(
        &file,
        &through_missing,
        "Windows folds `..` lexically, so this spelling names the source",
    );
    // Truncating, not merely creating: the file that comes back empty is the
    // fixture, which is what makes the hazard concrete rather than notional.
    std::fs::File::create(&through_missing)
        .expect("Win32 resolves this spelling, so the OS will not refuse it");
    assert!(
        std::fs::read(&file).unwrap().is_empty(),
        "the write went somewhere else: the premise of this test is wrong"
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
