//! Tests for `collapse_core::paths`, the two predicates that stand between a
//! user and losing the file they asked to compress.
//!
//! Both exist because **comparing paths is not comparing files**. A hardlink is
//! not a pointer to a file, it *is* a name of that file, so two hardlinks
//! resolve to two different paths on every operating system and a guard built
//! on resolved paths waves the dangerous pair through. Two reproductions on
//! macOS motivated this module, and both were verified to destroy data before
//! the fix:
//!
//! * `collapse compress notes.txt -o alias.zip --force`, with `alias.zip` a
//!   hardlink of `notes.txt`: the source was left starting with the zip header
//!   `PK`, and its content was in neither the archive nor on disk. That is the
//!   `same_file` half.
//! * `collapse compress photos -o out/archive.zip --force`, with
//!   `out/archive.zip` a hardlink of `photos/a.txt`: `a.txt` was destroyed, and
//!   the containment check missed it because the output's *path* is nowhere
//!   near the folder. That is the `inside` half.
//!
//! So the hardlink cases here are the point of the whole change and are
//! deliberately NOT `cfg(unix)`: `std::fs::hard_link` needs no privilege on
//! Windows either, and Windows is where the old desktop guard had no identity
//! check at all. Only symlinks (which need Developer Mode or admin on Windows)
//! and file modes stay Unix gated.
//!
//! Every hardlink fixture asserts that its two names canonicalize APART. Without
//! that, a platform where they happened to resolve together would keep passing
//! through plain path equality while quietly testing nothing.
//!
//! `same_file` is asserted in both argument orders throughout. The shipped
//! implementation is symmetric (it resolves both sides the same way before
//! comparing anything), so no order can fail today; the swap is aimed at the
//! refactor that resolves one side only, for example canonicalizing the output
//! and comparing it against the source exactly as the caller typed it.
//!
//! `inside` is not symmetric and is not treated as if it were: `inside(dir,
//! candidate)` asks "would archiving `dir` read `candidate`", and the tests
//! that swap the arguments do so to pin that asymmetry, not to expect the same
//! answer.

use std::path::{Path, PathBuf};

use collapse_core::paths::{inside, same_file};
use tempfile::TempDir;

// ------------------------------------------------------------------ helpers --

fn write(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent directory");
    }
    std::fs::write(path, contents).expect("write fixture file");
}

fn make_dir(path: &Path) {
    std::fs::create_dir_all(path).expect("create fixture directory");
}

/// Create a second name for `target` and prove it really is one.
///
/// `hard_link` fails loudly rather than falling back to a copy, so reaching the
/// assertion means one inode with two names. The assertion then proves the
/// fixture is not degenerate: if the two names ever canonicalized together, the
/// resolved-path layer would answer and the identity layers this module exists
/// for would stop being exercised without a single test turning red.
fn hard_link_apart(target: &Path, link: &Path) {
    if let Some(parent) = link.parent() {
        make_dir(parent);
    }
    std::fs::hard_link(target, link).expect("create hardlink fixture");
    assert_ne!(
        target.canonicalize().expect("canonicalize hardlink target"),
        link.canonicalize().expect("canonicalize hardlink"),
        "fixture is degenerate: two hardlink names that resolve to one path \
         would be caught by path equality alone, testing nothing"
    );
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create symlink fixture");
}

/// Assert the two paths are one file, whichever way round they are given.
fn assert_same(a: &Path, b: &Path, why: &str) {
    assert!(same_file(a, b), "{why}: {a:?} vs {b:?}");
    assert!(same_file(b, a), "{why} (arguments swapped): {b:?} vs {a:?}");
}

/// Assert the two paths are not one file, whichever way round they are given.
/// A false positive only blocks a legitimate compression instead of costing
/// data, but the app that cannot write an archive is broken too, so it is
/// pinned just as hard.
fn assert_not_same(a: &Path, b: &Path, why: &str) {
    assert!(!same_file(a, b), "{why}: {a:?} vs {b:?}");
    assert!(
        !same_file(b, a),
        "{why} (arguments swapped): {b:?} vs {a:?}"
    );
}

fn assert_inside(dir: &Path, candidate: &Path, why: &str) {
    assert!(
        inside(dir, candidate),
        "{why}: {candidate:?} within {dir:?}"
    );
}

fn assert_not_inside(dir: &Path, candidate: &Path, why: &str) {
    assert!(
        !inside(dir, candidate),
        "{why}: {candidate:?} within {dir:?}"
    );
}

// =========================================================== same_file ======

// ----------------------------------------------------------------- identity --

#[test]
fn a_file_and_a_directory_are_each_the_same_as_themselves() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let folder = temp.path().join("photos");
    make_dir(&folder);

    // Directories go through this predicate too (both front ends compress a
    // folder as readily as a file) and nothing in `same_file` branches on what
    // a path names, so one table covers both kinds.
    for path in [&file, &folder] {
        assert_same(path, path, "a path is the same file as itself");
    }
}

// ---------------------------------------------------------------- spellings --

#[test]
fn spellings_that_path_equality_already_folds_still_reach_one_file() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let folder = temp.path().join("photos");
    make_dir(&folder);

    // Honest scope: `std::path::Path` compares component by component, so a `.`
    // component and a trailing separator are gone before `same_file` sees them.
    // These two cases therefore survive a predicate reduced to `a == b`; what
    // they kill is the other cheap shortcut, a predicate comparing the caller's
    // raw strings, for which every spelling below differs. A native folder
    // picker may or may not hand back the trailing separator, which is how such
    // a predicate would meet these in the wild. The spelling with real teeth is
    // `..`, in the next test.
    let dotted = temp.path().join(".").join("notes.txt");
    let trailing = {
        let mut spelled = folder.clone().into_os_string();
        spelled.push(std::path::MAIN_SEPARATOR.to_string());
        PathBuf::from(spelled)
    };

    assert_same(&dotted, &file, "a `.` component does not change the file");
    assert_same(
        &trailing,
        &folder,
        "a trailing separator does not change the folder",
    );
}

#[test]
fn a_parent_component_is_resolved_before_the_comparison() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    make_dir(&temp.path().join("sub"));

    // `dir/sub/../notes.txt` is how a concatenated or hand-typed output path
    // sneaks back onto its own source, and unlike `.` it is a spelling `Path`
    // must not fold on its own, because `sub` could be a symlink. Resolving it
    // is the predicate's own work, so dropping the `canonicalize` fails here.
    let round_trip = temp.path().join("sub").join("..").join("notes.txt");
    assert_ne!(
        round_trip.as_path(),
        file.as_path(),
        "fixture is pointless unless the two spellings differ lexically"
    );
    assert_same(
        &round_trip,
        &file,
        "`sub/../name` is `name` and must be recognised as such",
    );
}

#[test]
fn a_case_differing_spelling_follows_the_filesystem_not_the_operating_system() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let shouted = temp.path().join("NOTES.TXT");

    // Probed at runtime instead of gated by `cfg`, because case folding belongs
    // to the volume rather than to the OS: APFS and NTFS fold by default, ext4
    // does not, and the toolkit ships on all three.
    if shouted.exists() {
        // One file, two spellings: a user who picks `notes.txt` and types
        // `NOTES.TXT` as the output would otherwise watch the source be
        // truncated. Resolution catches it rather than the identity layers,
        // since the OS reports the name as stored, so a predicate comparing the
        // caller's strings would answer "different" here.
        assert_same(
            &file,
            &shouted,
            "a case-folding volume reaches one file through both spellings",
        );
    } else {
        // Case-sensitive volume: the shouted spelling names nothing, so this is
        // the missing-path case again and has no teeth of its own.
        assert_not_same(
            &file,
            &shouted,
            "a case-sensitive volume has no file by that name",
        );
    }
}

// ----------------------------------------------------------------- symlinks --

#[cfg(unix)]
#[test]
fn a_path_through_a_symlinked_ancestor_is_the_same_file() {
    let temp = TempDir::new().unwrap();
    let real = temp.path().join("real");
    let file = real.join("notes.txt");
    write(&file, b"hello");
    let link = temp.path().join("link");
    symlink(&real, &link);

    // The everyday shape: two paths that differ as strings and name one file
    // because a directory along the way is a link. The temp dir is itself one
    // on macOS (`/var` to `/private/var`) but not on the Linux runners, so the
    // fixture is built by hand and the `assert_ne!` proves it is not degenerate
    // on either.
    let through_link = link.join("notes.txt");
    assert_ne!(
        through_link,
        through_link.canonicalize().unwrap(),
        "fixture is pointless unless the spelling differs from its resolved form"
    );
    assert_same(
        &through_link,
        &file,
        "a symlinked ancestor must be resolved away",
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_is_the_same_file_as_its_target_for_files_and_directories() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let folder = temp.path().join("photos");
    make_dir(&folder);
    let to_file = temp.path().join("shortcut.txt");
    let to_folder = temp.path().join("pictures");
    symlink(&file, &to_file);
    symlink(&folder, &to_folder);

    // Writing an archive through a symlink truncates the target, so the link
    // has to count as the target. Both kinds are here because the predicate has
    // no file-type branch and the directory case must not be able to rot while
    // the file case passes.
    for (link, target) in [(&to_file, &file), (&to_folder, &folder)] {
        assert_same(link, target, "a symlink is its target");
    }
}

#[cfg(unix)]
#[test]
fn a_chain_of_symlinks_is_the_same_file_as_the_file_at_the_end() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let first = temp.path().join("one.txt");
    let second = temp.path().join("two.txt");
    symlink(&file, &first);
    symlink(&first, &second);

    // Resolution must follow the whole chain, not one hop: an implementation
    // built on a single `read_link` passes the test above and fails this one.
    assert_same(
        &second,
        &file,
        "a chain of symlinks resolves to the file it ends at",
    );
}

#[cfg(unix)]
#[test]
fn a_broken_symlink_matches_nothing_until_its_target_exists() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let gone = temp.path().join("gone.txt");
    let dangling = temp.path().join("dangling.txt");
    symlink(&gone, &dangling);

    // Nothing exists behind a dangling link, so writing through it creates a
    // file instead of truncating one and the guard must not stand in the way.
    assert_not_same(
        &file,
        &dangling,
        "a broken symlink resolves to nothing and cannot be an existing file",
    );

    // The half that pins the cause: it is the brokenness and not the link-ness
    // that produced the answer above. Create the target, which is exactly what
    // writing through the link would do, and the same two paths become one
    // file. A predicate that answered "different" for any symlink would pass
    // the first assertion and fail this one.
    write(&gone, b"created later");
    assert_same(
        &gone,
        &dangling,
        "once its target exists the link is that file",
    );
}

// ---------------------------------------------------------------- hardlinks --

#[test]
fn a_hardlink_beside_its_file_is_the_same_file() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let link = temp.path().join("also-notes.txt");
    hard_link_apart(&file, &link);

    // The reason the predicate asks the filesystem for identity instead of
    // stopping at the resolved path. Deleting the `is_same_file` layer turns
    // this red on both Unix and Windows.
    assert_same(&file, &link, "two names for one inode are one file");
}

#[test]
fn a_hardlink_in_another_directory_under_another_name_is_the_same_file() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("source").join("notes.txt");
    write(&file, b"hello");
    let link = temp.path().join("output").join("archive.zip");
    hard_link_apart(&file, &link);

    // Reproduction one, exactly: different directory, different stem, different
    // extension, and nothing about either spelling hints they are one file.
    // This is the pair that `--force` used to overwrite, destroying the source.
    assert_same(
        &file,
        &link,
        "a hardlink is the same file wherever it lives and whatever it is called",
    );
}

#[cfg(unix)]
#[test]
fn a_write_only_file_reached_through_two_hardlinks_is_the_same_file() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let link = temp.path().join("archive.zip");
    hard_link_apart(&file, &link);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o222))
        .expect("make the fixture write-only");

    // The case the third layer exists for, and the one a naive implementation
    // gets wrong in the dangerous direction. Mode 0o222 refuses `open` for
    // reading, so the handle-based identity check cannot answer, yet the file is
    // still perfectly truncatable: answering "different" here would hand
    // `--force` the one file it must never be given. `stat` needs no handle and
    // answers anyway.
    //
    // Running as root (some CI containers do) opens it regardless, in which case
    // layer two answers and this degrades into another hardlink case. The
    // expected result is identical either way, and the flag below names which
    // world the run happened in when it fails.
    let stat_layer_required = std::fs::File::open(&file).is_err();
    assert_same(
        &file,
        &link,
        &format!(
            "a write-only file is one file through both of its names \
             (unreadable, so the stat layer was required: {stat_layer_required})"
        ),
    );
}

// --------------------------------------------------------- distinct things --

#[test]
fn identical_content_does_not_make_two_files_one_file() {
    let temp = TempDir::new().unwrap();
    let same_bytes = (temp.path().join("a.txt"), temp.path().join("b.txt"));
    write(&same_bytes.0, b"identical bytes");
    write(&same_bytes.1, b"identical bytes");
    let empty = (temp.path().join("c.txt"), temp.path().join("d.txt"));
    write(&empty.0, b"");
    write(&empty.1, b"");

    // The predicate is about identity, never equality. Compressing a file into
    // an archive whose bytes happen to match another file has to be allowed, and
    // two empty files are the degenerate version that would fool any
    // size-or-content shortcut.
    assert_not_same(
        &same_bytes.0,
        &same_bytes.1,
        "equal content is not identity",
    );
    assert_not_same(&empty.0, &empty.1, "two empty files are still two files");
}

#[test]
fn the_same_name_in_two_directories_is_not_the_same_file() {
    let temp = TempDir::new().unwrap();
    let one = temp.path().join("one").join("notes.txt");
    let two = temp.path().join("two").join("notes.txt");
    write(&one, b"hello");
    write(&two, b"hello");

    // A shared file name is what a comparison of `file_name()` would fall for.
    assert_not_same(&one, &two, "a shared file name does not make one file");
}

#[test]
fn a_file_is_not_the_same_as_the_directory_that_holds_it() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    let file = folder.join("notes.txt");
    write(&file, b"hello");
    let unrelated = temp.path().join("elsewhere");
    make_dir(&unrelated);

    // Containment must not read as identity. `same_file` answers one question
    // only, and the containment question has its own predicate: this is the
    // seam between them, and collapsing the two would make `inside`'s
    // `candidate != dir` guard unreachable.
    assert_not_same(
        &folder,
        &file,
        "a directory is not the same file as its own child",
    );
    assert_not_same(&file, &unrelated, "a file is never a directory");
}

// ----------------------------------------------------------- missing paths --

#[test]
fn a_path_that_does_not_exist_is_never_the_same_file() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let missing = temp.path().join("notes.zip");
    assert!(!missing.exists(), "fixture must not exist yet");

    // Load bearing rather than a detail: this is the ordinary happy path, an
    // archive named after its source and written beside it. Every first
    // compression targets a path with nothing behind it, so a `true` here would
    // stop the toolkit producing a single archive. Both argument positions are
    // asserted because the callers pass (source, output) and only the output is
    // normally the missing one.
    assert_not_same(
        &file,
        &missing,
        "a path with nothing behind it cannot be an existing file",
    );
}

#[test]
fn paths_with_nothing_behind_them_never_match_even_when_spelled_alike() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("gone.txt");
    let also_missing = temp.path().join("gone-too.txt");

    // Textual equality is deliberately not enough: with nothing on disk there is
    // no file to be the same as, and the write must be let through. A predicate
    // short-circuiting on `a == b` before resolving anything fails the first
    // assertion.
    assert_not_same(
        &missing,
        &missing,
        "one spelling of a missing path is still not a file",
    );
    assert_not_same(
        &missing,
        &also_missing,
        "two paths that resolve to nothing do not match",
    );

    // An unset field in a front end sends an empty path; it must resolve to
    // nothing rather than to the current directory.
    let empty = PathBuf::new();
    assert_not_same(&missing, &empty, "an empty path resolves to nothing");
    assert_not_same(&empty, &empty, "two empty paths still resolve to nothing");
}

#[test]
fn the_predicate_agrees_with_the_os_about_a_parent_component_that_skips_a_missing_directory() {
    let temp = TempDir::new().unwrap();
    let file = temp.path().join("notes.txt");
    write(&file, b"hello");
    let through_missing = temp.path().join("gone").join("..").join("notes.txt");

    // Whether `gone/../notes.txt` names the file depends on the platform: Unix
    // resolves every component and refuses because `gone` is absent, while
    // Windows folds `..` lexically for non-verbatim paths and opens the file.
    // Hardcoding either answer would fail on the other OS, and the answer that
    // matters is not the platform's, it is whether the guard can be walked past:
    // if the OS will open this path, the same write can truncate the source, so
    // the predicate must say "same". If the OS refuses it, no write can happen
    // through it and "different" costs nothing.
    match std::fs::File::open(&through_missing) {
        Ok(_) => assert_same(
            &file,
            &through_missing,
            "the OS opens this path, so the guard must recognise it as the source",
        ),
        Err(error) => {
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::NotFound,
                "the fixture must be refused for the missing component, \
                 not for a permission or name-length problem"
            );
            assert_not_same(
                &file,
                &through_missing,
                "an unresolvable path is not a file, and the OS refuses it too",
            );
        }
    }
}

// ============================================================== inside ======

// ------------------------------------------------------------- by the path --

#[test]
fn a_file_directly_in_the_folder_is_inside_it() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    let file = folder.join("a.txt");
    write(&file, b"hello");

    assert_inside(
        &folder,
        &file,
        "a child of the folder would be read while archiving it",
    );

    // The arguments are not interchangeable, and the swap is the pin: `inside`
    // asks "would archiving the first read the second", so a folder is not
    // inside one of its own files. It also proves a non-directory first argument
    // is answered rather than panicking, since `read_dir` on a file fails.
    assert_not_inside(
        &file,
        &folder,
        "the folder holding a file is not inside that file",
    );
}

#[test]
fn a_file_nested_several_levels_down_is_inside_the_folder() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    let deep = folder.join("2026").join("august").join("raw").join("a.txt");
    write(&deep, b"hello");
    let subfolder = folder.join("2026").join("august");

    // Depth changes nothing for the path half: the archive contains the whole
    // tree, so anything under the root would be read.
    assert_inside(&folder, &deep, "a deeply nested file is still a member");
    assert_inside(
        &folder,
        &subfolder,
        "an intermediate directory is inside the folder too",
    );
}

#[test]
fn a_file_outside_the_folder_is_not_inside_it() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    write(&folder.join("a.txt"), b"hello");
    let sibling = temp.path().join("output").join("archive.zip");
    write(&sibling, b"archive");
    let above = temp.path().join("archive.zip");
    write(&above, b"archive");

    // The everyday legitimate case: writing the archive next to, or above, the
    // folder being compressed must stay allowed, or the tool cannot be used.
    assert_not_inside(&folder, &sibling, "a sibling directory is not inside");
    assert_not_inside(&folder, &above, "the parent directory is not inside");
}

#[test]
fn the_folder_itself_is_not_reported_as_inside_itself() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    write(&folder.join("a.txt"), b"hello");

    // Deliberate: `inside` answers about containment only, and identity is
    // `same_file`'s question, which both callers ask first (the CLI returns
    // `OutputIsSource` before it ever reaches the containment check). Reporting
    // the folder as inside itself would make the two guards overlap and would
    // put a second, differently worded error in the way of the same mistake.
    // Deleting the `candidate != dir` condition turns this red.
    assert_not_inside(
        &folder,
        &folder,
        "containment is not identity: the folder is not inside itself",
    );
}

#[test]
fn a_sibling_folder_whose_name_merely_starts_with_the_folders_name_is_not_inside_it() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    write(&folder.join("a.txt"), b"hello");
    let backup = temp.path().join("photos-backup");
    let in_backup = backup.join("a.txt");
    write(&in_backup, b"hello");

    // `photos-backup` starts with `photos` as a string but not as a sequence of
    // path components, and `Path::starts_with` is what tells them apart. A
    // predicate comparing the two as strings (or as `to_string_lossy()`) would
    // refuse to write the backup archive and there would be no way around it,
    // since neither guard can be bought past with `--force`. The file inside the
    // sibling is checked too, because that is the path a real output takes.
    assert_not_inside(&folder, &backup, "a name prefix is not a component prefix");
    assert_not_inside(
        &folder,
        &in_backup,
        "a file in the prefixed sibling is not inside the folder",
    );
}

#[test]
fn both_sides_are_resolved_before_containment_is_judged() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    let file = folder.join("a.txt");
    write(&file, b"hello");
    make_dir(&folder.join("sub"));

    // Neither side is compared as typed. A folder spelled through `..`, and a
    // candidate spelled the same way, are the shapes a concatenated output path
    // arrives in, and dropping either `canonicalize` makes one of these red.
    let spelled_folder = folder.join("sub").join("..");
    let spelled_file = folder.join("sub").join("..").join("a.txt");
    assert_inside(
        &spelled_folder,
        &file,
        "the folder is resolved before it is compared",
    );
    assert_inside(
        &folder,
        &spelled_file,
        "the candidate is resolved before it is compared",
    );
}

#[cfg(unix)]
#[test]
fn a_folder_reached_through_a_symlinked_ancestor_still_contains_its_files() {
    let temp = TempDir::new().unwrap();
    let real = temp.path().join("real");
    let file = real.join("photos").join("a.txt");
    write(&file, b"hello");
    let link = temp.path().join("link");
    symlink(&real, &link);

    // The source path a picker hands over may run through a linked ancestor
    // while the output path does not, or the other way round. Either way the
    // archive would read this file.
    let through_link = link.join("photos");
    assert_inside(
        &through_link,
        &file,
        "a symlinked ancestor on the folder side must be resolved away",
    );
    assert_inside(
        &real.join("photos"),
        &link.join("photos").join("a.txt"),
        "a symlinked ancestor on the candidate side must be resolved away",
    );
}

// ----------------------------------------------------- by shared identity --

#[test]
fn a_hardlink_outside_the_folder_to_a_file_inside_it_is_inside_it() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    let member = folder.join("a.txt");
    write(&member, b"hello");
    let output = temp.path().join("out").join("archive.zip");
    hard_link_apart(&member, &output);

    // Reproduction two, exactly. The output's path is nowhere near the folder,
    // so the path half of the predicate says nothing, yet the two names share an
    // inode: creating the archive truncates `a.txt`, which the walk then
    // archives in its truncated state, losing it from the archive as thoroughly
    // as from disk. Deleting the tree walk turns this red, and `--force` cannot
    // buy past it because both callers check containment before the force flag.
    assert_inside(
        &folder,
        &output,
        "a hardlink to a member is a member, wherever its path points",
    );

    // Identity is mutual, so the relation holds from the other folder too:
    // archiving `out` reads the same bytes, and writing that archive over
    // `photos/a.txt` is refused for the same reason.
    assert_inside(
        &temp.path().join("out"),
        &member,
        "the shared inode makes each folder's member the other's too",
    );
}

#[test]
fn the_walk_finds_a_hardlink_to_a_file_in_a_deeply_nested_subfolder() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    let buried = folder
        .join("2026")
        .join("august")
        .join("raw")
        .join("dsc_0001.txt");
    write(&buried, b"hello");
    let output = temp.path().join("out").join("archive.zip");
    hard_link_apart(&buried, &output);

    // The archive contains the whole tree, so a shared inode four levels down is
    // as fatal as one at the top. An implementation that only listed the
    // folder's immediate children would pass the test above and fail this one.
    assert_inside(
        &folder,
        &output,
        "the search for a shared inode must recurse into subfolders",
    );
}

#[test]
fn a_hardlink_to_a_file_in_another_folder_is_not_inside_this_one() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    write(&folder.join("a.txt"), b"hello");
    let elsewhere = temp.path().join("documents").join("b.txt");
    write(&elsewhere, b"hello");
    let output = temp.path().join("out").join("archive.zip");
    hard_link_apart(&elsewhere, &output);

    // The walk has to discriminate, not just find something: this output shares
    // an inode with a file that is not a member, so archiving `photos` never
    // reads it and the write is legitimate. A walk that answered true on any
    // match, or on any readable file, would block it forever.
    assert_not_inside(
        &folder,
        &output,
        "a hardlink to a non-member does not make the output a member",
    );
}

#[test]
fn an_empty_folder_holds_nothing_until_a_second_name_lands_in_it() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    make_dir(&folder);
    let output = temp.path().join("out").join("archive.zip");
    write(&output, b"previous archive");

    // Nothing to read means nothing to lose, and the walk must terminate rather
    // than fall over on an empty directory.
    assert_not_inside(
        &folder,
        &output,
        "an empty folder cannot contain the output",
    );

    // The other half is what stops the assertion above from passing for the
    // wrong reason: give the folder a second name for that very file and the
    // same call must flip. A predicate hardwired to `false` passes the first
    // assertion and fails this one.
    hard_link_apart(&output, &folder.join("linked.bin"));
    assert_inside(
        &folder,
        &output,
        "once the folder holds another name for the output, it is a member",
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_child_pointing_at_the_candidate_does_not_make_it_inside() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    make_dir(&folder);
    let outside = temp.path().join("documents").join("b.txt");
    write(&outside, b"hello");
    symlink(&outside, &folder.join("shortcut.txt"));

    // Must not match, and this is not a leniency: every backend skips symlinked
    // children (`walk_tree` never stores a link), so this file is never read
    // while archiving the folder and truncating it cannot corrupt the archive.
    // Refusing here would block a legitimate write with an error the user cannot
    // override. Dropping the `is_symlink` skip in the walk turns this red.
    assert_not_inside(
        &folder,
        &outside,
        "a symlinked child is never read, so its target is not a member",
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_folder_that_points_out_of_it_is_not_inside_it() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    make_dir(&folder);
    let outside = temp.path().join("documents").join("b.txt");
    write(&outside, b"hello");
    let shortcut = folder.join("shortcut.txt");
    symlink(&outside, &shortcut);

    // The mirror image of the previous test, and the reason the path half
    // resolves the candidate: this path *looks* like a member but writing to it
    // lands on `documents/b.txt`, which the archive never touches. Comparing the
    // candidate as typed would refuse this write on a lexical resemblance.
    assert_not_inside(
        &folder,
        &shortcut,
        "a path inside the folder that resolves out of it is not a member",
    );
}

// ----------------------------------------------------------- missing paths --

#[test]
fn a_path_that_does_not_exist_is_not_inside_anything() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("photos");
    write(&folder.join("a.txt"), b"hello");
    let not_yet = folder.join("archive.zip");
    let missing_folder = temp.path().join("gone");
    let existing = folder.join("a.txt");

    // Pins the contract the module documents: `inside` speaks about files that
    // exist. A name nothing occupies cannot be truncated, and both callers ask
    // only about an output that already exists (the CLI runs both guards inside
    // `if output.exists()`), so nothing today depends on a different answer.
    // Anyone adding a caller has to know that a not-yet-created path inside the
    // folder is not reported here.
    assert!(!not_yet.exists(), "fixture must not exist yet");
    assert_not_inside(
        &folder,
        &not_yet,
        "a path with nothing behind it is not a member yet",
    );

    // And a folder that does not exist contains nothing at all, rather than
    // producing an error or a walk of the whole filesystem.
    assert_not_inside(
        &missing_folder,
        &existing,
        "a folder that does not exist cannot contain anything",
    );
}
