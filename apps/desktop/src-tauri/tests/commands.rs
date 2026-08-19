//! Tests for the local half of the desktop command surface: `is_directory`,
//! `compress_path` with no server, and `extract_archive`.
//!
//! A `#[tauri::command]` is an ordinary function, so these drive the real
//! commands in-process and assert what lands on disk. Nothing here starts a
//! server; the remote branch of `compress_path` is `tests/remote.rs`'s job.
//!
//! Every command stringifies the underlying error with `.to_string()`, so the
//! assertions match on the message rather than on an error variant.

use std::fs;
use std::path::{Path, PathBuf};

use collapse_desktop::commands::{compress_path, extract_archive, is_directory};
use tempfile::TempDir;

/// The three formats the UI offers. The wire spelling doubles as the archive
/// extension for all of them, which is why one array serves both roles.
const FORMATS: [&str; 3] = ["zip", "7z", "tar"];

fn compress_local(source: &Path, output: &Path, format: &str, level: u32) -> Result<String, String> {
    compress_path(
        source.to_string_lossy().into_owned(),
        output.to_string_lossy().into_owned(),
        format.to_string(),
        level,
        None,
    )
}

fn extract_to(archive: &Path, output_dir: &Path) -> Result<Vec<String>, String> {
    extract_archive(
        archive.to_string_lossy().into_owned(),
        output_dir.to_string_lossy().into_owned(),
    )
}

/// Normalize and sort an extracted listing so the expectations read the same
/// on a platform whose path separator is not `/`.
fn listing(paths: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = paths.iter().map(|p| p.replace('\\', "/")).collect();
    out.sort();
    out
}

/// A small tree with a nested subdirectory and an empty folder, so the
/// round-trip tests can prove the shape survives and not just the bytes.
fn make_tree(parent: &Path) -> PathBuf {
    let root = parent.join("photos");
    fs::create_dir_all(root.join("sub").join("deep")).unwrap();
    fs::create_dir_all(root.join("empty")).unwrap();
    fs::write(root.join("a.txt"), b"top level").unwrap();
    fs::write(root.join("sub").join("b.txt"), b"one down").unwrap();
    fs::write(root.join("sub").join("deep").join("c.txt"), b"two down").unwrap();
    root
}

// -------------------------------------------------------------- is_directory --

#[test]
fn is_directory_distinguishes_directories_files_and_absences() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    fs::write(&file, b"hello").unwrap();

    assert!(is_directory(dir.path().to_string_lossy().into_owned()));
    assert!(!is_directory(file.to_string_lossy().into_owned()));
    // A path that does not exist must answer `false` rather than panic: the UI
    // calls this on whatever the dialog handed back, before anything reads it.
    assert!(!is_directory(
        dir.path().join("nope").to_string_lossy().into_owned()
    ));
}

#[test]
#[cfg(unix)]
fn is_directory_follows_symlinks_because_it_uses_is_dir() {
    let dir = TempDir::new().unwrap();
    let target_dir = dir.path().join("target_dir");
    let target_file = dir.path().join("target_file");
    fs::create_dir(&target_dir).unwrap();
    fs::write(&target_file, b"hello").unwrap();

    let link_to_dir = dir.path().join("link_to_dir");
    let link_to_file = dir.path().join("link_to_file");
    std::os::unix::fs::symlink(&target_dir, &link_to_dir).unwrap();
    std::os::unix::fs::symlink(&target_file, &link_to_file).unwrap();

    // `is_dir()` resolves the link, so the UI shows a link to a folder as a
    // folder. Pinned because the compression side deliberately does the
    // opposite (walk_tree never follows a link).
    assert!(is_directory(link_to_dir.to_string_lossy().into_owned()));
    assert!(!is_directory(link_to_file.to_string_lossy().into_owned()));
}

// ------------------------------------------------------- compress happy path --

#[test]
fn compressing_a_file_produces_a_non_empty_archive_at_the_requested_path() {
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"collapse desktop").unwrap();
        let output = dir.path().join(format!("out.{format}"));

        let returned = compress_local(&source, &output, format, 3).unwrap();

        // The command echoes back the very path it was asked to write, which
        // is what the UI shows the user afterwards.
        assert_eq!(returned, output.to_string_lossy(), "{format}");
        assert!(output.is_file(), "{format}: no archive was written");
        assert!(
            fs::metadata(&output).unwrap().len() > 0,
            "{format}: the archive is empty"
        );
    }
}

#[test]
fn compressing_a_file_round_trips_byte_identically_under_its_base_name() {
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        // Nested, so a regression that stored the full path instead of the
        // base name would be visible in the extracted listing.
        let nested = dir.path().join("deep").join("folder");
        fs::create_dir_all(&nested).unwrap();
        let source = nested.join("notes.txt");
        let content = b"collapse desktop round trip ".repeat(64);
        fs::write(&source, &content).unwrap();

        let archive = dir.path().join(format!("out.{format}"));
        compress_local(&source, &archive, format, 3).unwrap();

        let out_dir = dir.path().join("extracted");
        let extracted = extract_to(&archive, &out_dir).unwrap();

        assert_eq!(listing(extracted), vec!["notes.txt".to_string()], "{format}");
        assert_eq!(
            fs::read(out_dir.join("notes.txt")).unwrap(),
            content,
            "{format}: the round-trip changed the bytes"
        );
    }
}

#[test]
fn compressing_a_directory_round_trips_the_whole_tree_prefixed_with_its_own_name() {
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let root = make_tree(dir.path());
        let archive = dir.path().join(format!("tree.{format}"));

        compress_local(&root, &archive, format, 3).unwrap();
        assert!(archive.is_file(), "{format}");

        let out_dir = dir.path().join("extracted");
        let extracted = extract_to(&archive, &out_dir).unwrap();

        assert_eq!(
            listing(extracted),
            vec![
                "photos/a.txt".to_string(),
                "photos/sub/b.txt".to_string(),
                "photos/sub/deep/c.txt".to_string(),
            ],
            "{format}: wrong entry shape"
        );
        assert_eq!(fs::read(out_dir.join("photos").join("a.txt")).unwrap(), b"top level");
        assert_eq!(
            fs::read(out_dir.join("photos").join("sub").join("b.txt")).unwrap(),
            b"one down"
        );
        assert_eq!(
            fs::read(out_dir.join("photos").join("sub").join("deep").join("c.txt")).unwrap(),
            b"two down"
        );
        // Directory entries are excluded from the listing but must still be
        // materialized, or an empty folder would silently vanish.
        assert!(
            out_dir.join("photos").join("empty").is_dir(),
            "{format}: the empty folder did not survive"
        );
    }
}

#[test]
fn every_level_from_one_to_five_is_accepted() {
    for format in FORMATS {
        for level in 1..=5 {
            let dir = TempDir::new().unwrap();
            let source = dir.path().join("notes.txt");
            fs::write(&source, b"levels are 1..=5 ".repeat(32)).unwrap();
            let output = dir.path().join(format!("out.{format}"));

            compress_local(&source, &output, format, level)
                .unwrap_or_else(|e| panic!("{format} level {level} was refused: {e}"));
            assert!(
                fs::metadata(&output).unwrap().len() > 0,
                "{format} level {level}: empty archive"
            );
        }
    }
}

#[test]
fn an_empty_server_string_is_treated_as_local() {
    // The dispatcher filters the empty string out before choosing the remote
    // branch. If that filter went missing this would try to reach a server at
    // "" and fail, with nothing listening anywhere.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"local please").unwrap();
    let output = dir.path().join("out.zip");

    let returned = compress_path(
        source.to_string_lossy().into_owned(),
        output.to_string_lossy().into_owned(),
        "zip".to_string(),
        3,
        Some(String::new()),
    )
    .expect("an empty server string must compress locally");

    assert_eq!(returned, output.to_string_lossy());
    assert!(fs::metadata(&output).unwrap().len() > 0);
}

// ----------------------------------------------------------- compress guards --

#[test]
fn a_missing_source_is_refused_before_any_output_is_created() {
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope.txt");
        let output = dir.path().join(format!("out.{format}"));

        let err = compress_local(&missing, &output, format, 3).unwrap_err();

        assert_eq!(err, format!("Not found: {}", missing.to_string_lossy()), "{format}");
        // core's `compress_zip` creates its output before opening the source,
        // so without the command's own existence check a missing source would
        // leave a zero-byte .zip behind. This pins that the check runs first.
        assert!(
            !output.exists(),
            "{format}: a stray output was left behind for a missing source"
        );
    }
}

#[test]
#[cfg(unix)]
fn a_source_that_is_neither_file_nor_directory_is_refused() {
    // /dev/null exists, so the existence check passes, but it is a character
    // device: the second guard is the only thing standing between it and a
    // backend that would try to read it as a file.
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("out.zip");

    let err = compress_local(Path::new("/dev/null"), &output, "zip", 3).unwrap_err();

    assert!(err.contains("Unsupported source"), "{err}");
    assert!(!output.exists(), "a stray output was left behind");
}

#[test]
fn an_unknown_format_is_refused_and_writes_nothing() {
    // "ZIP" is in the list on purpose: the parse is exact and case-sensitive,
    // so the UI must send the lowercase spelling.
    for format in ["rar", "", "ZIP", "7Z"] {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"hello").unwrap();
        let output = dir.path().join("out.zip");

        let err = compress_local(&source, &output, format, 3).unwrap_err();

        assert_eq!(err, format!("Unknown algorithm: {format}"));
        assert!(!output.exists(), "{format}: a stray output was left behind");
    }
}

#[test]
fn a_level_out_of_range_is_refused_for_every_format_and_never_panics() {
    // This is the one that matters most after the data-loss guard: core's zip
    // and 7z backends index a five-element preset array with `level - 1`, so a
    // level of 0 or 6 reaching them is a panic (and 0 underflows the index).
    // Going through the validating dispatcher is what keeps them unreachable.
    for format in FORMATS {
        for level in [0, 6, 99] {
            let dir = TempDir::new().unwrap();
            let source = dir.path().join("notes.txt");
            fs::write(&source, b"hello").unwrap();
            let output = dir.path().join(format!("out.{format}"));

            let err = compress_local(&source, &output, format, level).unwrap_err();

            assert!(
                err.contains("Invalid compression level"),
                "{format} level {level}: {err}"
            );
            assert!(
                !output.exists(),
                "{format} level {level}: a stray output was left behind"
            );
        }
    }
}

#[test]
fn a_level_out_of_range_is_refused_for_a_directory_too() {
    // `compress_dir` validates the level on its own; the directory branch of
    // the command must not bypass it.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let root = make_tree(dir.path());
        let output = dir.path().join(format!("tree.{format}"));

        let err = compress_local(&root, &output, format, 0).unwrap_err();

        assert!(err.contains("Invalid compression level"), "{format}: {err}");
        assert!(!output.exists(), "{format}: a stray output was left behind");
    }
}

#[test]
fn an_output_inside_a_missing_directory_fails_instead_of_panicking() {
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"hello").unwrap();
        let missing_dir = dir.path().join("does_not_exist");
        let output = missing_dir.join(format!("out.{format}"));

        let err = compress_local(&source, &output, format, 3).unwrap_err();

        assert!(!err.is_empty(), "{format}: an empty error message");
        assert!(
            !missing_dir.exists(),
            "{format}: the command created the missing parent directory"
        );
        if format != "7z" {
            // zip and tar hit it through `File::create`, so it surfaces as
            // CompressionError::Io; 7z's writer stringifies its own error.
            assert!(err.contains("IO error"), "{format}: {err}");
        }
    }
}

// ------------------------------------------------------- safety / data loss --

#[test]
fn compressing_a_file_onto_itself_is_refused_and_leaves_it_untouched() {
    // The single most destructive mistake this command can make: every backend
    // truncates the output before (or while) reading the source, so writing an
    // archive over its own source destroys it with no way back.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        let content = b"irreplaceable".to_vec();
        fs::write(&source, &content).unwrap();

        let err = compress_local(&source, &source, format, 3).unwrap_err();

        assert_eq!(err, "The output is the same file as the source.", "{format}");
        assert_eq!(
            fs::read(&source).unwrap(),
            content,
            "{format}: the source was modified"
        );
    }
}

#[test]
fn a_different_spelling_of_the_same_path_is_still_the_same_file() {
    // `same_file` canonicalizes, so `.` and `..` detours must not slip past.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    let content = b"irreplaceable".to_vec();
    fs::write(&source, &content).unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();

    for spelling in [
        dir.path().join(".").join("notes.txt"),
        sub.join("..").join("notes.txt"),
    ] {
        let err = compress_local(&source, &spelling, "zip", 3).unwrap_err();

        assert_eq!(
            err,
            "The output is the same file as the source.",
            "{}",
            spelling.display()
        );
        assert_eq!(
            fs::read(&source).unwrap(),
            content,
            "{}: the source was modified",
            spelling.display()
        );
    }
}

#[test]
#[cfg(unix)]
fn a_symlink_at_the_output_path_pointing_at_the_source_is_refused() {
    // Writing through the link would truncate the target, so the resolved-path
    // comparison has to see through it.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    let content = b"irreplaceable".to_vec();
    fs::write(&source, &content).unwrap();
    let alias = dir.path().join("alias.zip");
    std::os::unix::fs::symlink(&source, &alias).unwrap();

    let err = compress_local(&source, &alias, "zip", 3).unwrap_err();

    assert_eq!(err, "The output is the same file as the source.");
    assert_eq!(fs::read(&source).unwrap(), content, "the source was modified");
    assert_eq!(
        fs::read_link(&alias).unwrap(),
        source,
        "the symlink itself was replaced"
    );
}

#[test]
#[cfg(unix)]
fn a_hardlink_to_the_source_is_refused_even_though_the_paths_differ() {
    // Two names for one inode: the canonical paths are genuinely different, so
    // only the inode/device comparison catches this one.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    let content = b"irreplaceable".to_vec();
    fs::write(&source, &content).unwrap();
    let hardlink = dir.path().join("also_notes.zip");
    fs::hard_link(&source, &hardlink).unwrap();

    let err = compress_local(&source, &hardlink, "zip", 3).unwrap_err();

    assert_eq!(err, "The output is the same file as the source.");
    assert_eq!(fs::read(&source).unwrap(), content, "the source was modified");
    assert_eq!(
        fs::read(&hardlink).unwrap(),
        content,
        "the shared inode was modified"
    );
}

#[test]
fn the_earlier_guards_win_when_several_apply_at_once() {
    // The documented order is: exists, source kind, format parse, same file.
    // Reordering it is what would turn a typo into data loss, so pin it.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    let content = b"irreplaceable".to_vec();
    fs::write(&source, &content).unwrap();

    // A bad format on top of a same-file output reports the format.
    let err = compress_local(&source, &source, "rar", 3).unwrap_err();
    assert_eq!(err, "Unknown algorithm: rar");
    assert_eq!(fs::read(&source).unwrap(), content);

    // A missing source reports the absence, not the same-file coincidence.
    let missing = dir.path().join("nope.txt");
    let err = compress_local(&missing, &missing, "zip", 3).unwrap_err();
    assert_eq!(err, format!("Not found: {}", missing.to_string_lossy()));
    assert!(!missing.exists());
}

#[test]
fn an_existing_unrelated_output_is_overwritten_without_warning() {
    // Pins the real behaviour: the only guard here is "not the source itself".
    // There is no clobber check and no `--force` equivalent, unlike the CLI, so
    // the native save dialog's own "replace?" prompt is the sole protection an
    // unrelated file gets. Any future caller that skips a dialog gets none.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"hello").unwrap();
    let output = dir.path().join("out.zip");
    fs::write(&output, b"an older archive nobody asked to replace").unwrap();

    compress_local(&source, &output, "zip", 3).unwrap();

    let bytes = fs::read(&output).unwrap();
    assert_ne!(
        bytes, b"an older archive nobody asked to replace",
        "the old file survived, so a clobber guard appeared: update this test"
    );
    assert_eq!(&bytes[..2], b"PK", "the output is not the new zip");
}

#[test]
fn an_output_written_inside_the_source_tree_destroys_the_file_it_lands_on() {
    // KNOWN DEFECT, pinned rather than endorsed. `same_file` only compares the
    // source against the output, so an output *inside* the directory being
    // archived slips through: the backend truncates it to create the archive
    // and then archives the truncated file, storing its own header bytes under
    // that entry. The original content is unrecoverable, from the archive as
    // much as from disk. This is the same hazard the server backend hit when
    // its staging layout was flat. If a containment guard is ever added, this
    // test should be rewritten to assert the refusal, not deleted.
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("photos");
    fs::create_dir_all(&root).unwrap();
    let victim = root.join("a.txt");
    fs::write(&victim, b"irreplaceable member").unwrap();
    fs::write(root.join("b.txt"), b"other member").unwrap();

    // The archive is asked to land on one of the files it is archiving.
    compress_local(&root, &victim, "zip", 3).unwrap();

    assert_ne!(
        fs::read(&victim).unwrap(),
        b"irreplaceable member",
        "the source member survived, so a guard appeared: update this test"
    );

    // Rename so the extension dispatch can read it back, then look at what the
    // archive actually stored for the file it overwrote.
    let archive = dir.path().join("recovered.zip");
    fs::rename(&victim, &archive).unwrap();
    let out_dir = dir.path().join("extracted");
    let extracted = extract_to(&archive, &out_dir).unwrap();

    assert_eq!(
        listing(extracted),
        vec!["photos/a.txt".to_string(), "photos/b.txt".to_string()]
    );
    // The untouched sibling round-trips fine, which is what isolates the damage
    // to the entry the output landed on.
    assert_eq!(
        fs::read(out_dir.join("photos").join("b.txt")).unwrap(),
        b"other member"
    );
    assert_ne!(
        fs::read(out_dir.join("photos").join("a.txt")).unwrap(),
        b"irreplaceable member",
        "the archived copy is intact, so the truncation window closed: update this test"
    );
}

// ------------------------------------------------------------- extraction --

#[test]
fn extracting_a_missing_archive_reports_it_by_path() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("nope.zip");
    let out_dir = dir.path().join("out");

    let err = extract_to(&missing, &out_dir).unwrap_err();

    assert_eq!(err, format!("Not found: {}", missing.to_string_lossy()));
    assert!(!out_dir.exists(), "the output directory was created anyway");
}

#[test]
fn extracting_an_unknown_extension_is_refused() {
    let dir = TempDir::new().unwrap();
    let out_dir = dir.path().join("out");

    for (name, ext) in [("archive.rar", "rar"), ("archive", "")] {
        let bogus = dir.path().join(name);
        fs::write(&bogus, b"not an archive").unwrap();

        let err = extract_to(&bogus, &out_dir).unwrap_err();

        // The format is chosen from the extension alone: there is no magic-byte
        // sniffing, so an unknown suffix never reaches a backend.
        assert_eq!(err, format!("Compression failed: Unknown archive extension: .{ext}"));
    }
}

#[test]
fn an_uppercase_extension_is_rejected_even_for_a_valid_archive() {
    // KNOWN LIMITATION, pinned rather than endorsed: collapse-core matches the
    // extension against literal lowercase strings without lowercasing the
    // input, so a perfectly good zip named `.ZIP` is unreadable. If the match
    // is ever made case-insensitive, this test should be updated, not deleted.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"hello").unwrap();
    let lower = dir.path().join("quiet.zip");
    compress_local(&source, &lower, "zip", 3).unwrap();

    // A distinct base name, so this still works on a case-insensitive volume.
    let upper = dir.path().join("LOUD.ZIP");
    fs::copy(&lower, &upper).unwrap();

    let err = extract_to(&upper, &dir.path().join("out")).unwrap_err();

    assert_eq!(err, "Compression failed: Unknown archive extension: .ZIP");
    // The same bytes under a lowercase name extract fine, which is what makes
    // the failure above a naming quirk rather than a corrupt archive.
    assert_eq!(
        extract_to(&lower, &dir.path().join("out_lower")).unwrap(),
        vec!["notes.txt".to_string()]
    );
}

#[test]
fn the_returned_listing_is_relative_to_the_output_dir_and_excludes_directories() {
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let root = make_tree(dir.path());
        let archive = dir.path().join(format!("tree.{format}"));
        compress_local(&root, &archive, format, 3).unwrap();

        let out_dir = dir.path().join("out");
        let extracted = extract_to(&archive, &out_dir).unwrap();

        // Relative, forward-slashed, files only: `photos/empty` and the two
        // parent directories are created on disk but never listed.
        assert_eq!(
            listing(extracted.clone()),
            vec![
                "photos/a.txt".to_string(),
                "photos/sub/b.txt".to_string(),
                "photos/sub/deep/c.txt".to_string(),
            ],
            "{format}"
        );
        for entry in &extracted {
            let relative = Path::new(entry);
            assert!(
                relative.is_relative(),
                "{format}: {entry} is not relative to the output dir"
            );
            assert!(
                out_dir.join(relative).is_file(),
                "{format}: {entry} does not resolve under the output dir"
            );
        }
    }
}

#[test]
fn extracting_twice_into_the_same_directory_overwrites_instead_of_failing() {
    // Pins the real behaviour: there is no clobber detection anywhere in the
    // extract path, so a second run silently replaces whatever is there. The
    // UI has to warn about it, because this layer will not.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"archived content").unwrap();
        let archive = dir.path().join(format!("out.{format}"));
        compress_local(&source, &archive, format, 3).unwrap();

        let out_dir = dir.path().join("out");
        let first = extract_to(&archive, &out_dir).unwrap();

        // Tamper with the extracted copy, then extract again over it.
        fs::write(out_dir.join("notes.txt"), b"locally edited, much longer").unwrap();
        let second = extract_to(&archive, &out_dir).unwrap();

        assert_eq!(first, second, "{format}: the second run reported differently");
        assert_eq!(
            fs::read(out_dir.join("notes.txt")).unwrap(),
            b"archived content",
            "{format}: the local edit was not overwritten"
        );
    }
}
