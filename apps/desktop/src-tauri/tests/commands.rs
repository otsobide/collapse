//! Tests for the local half of the desktop command surface: `is_directory`,
//! `compress_path` with no server, and `extract_archive`.
//!
//! A `#[tauri::command]` is an ordinary function, so these drive the real
//! commands in-process and assert what lands on disk. Nothing here starts a
//! server; the remote branch of `compress_path` is `tests/remote.rs`'s job.
//!
//! Every command reports failure as a plain `String` (the backend errors are
//! stringified with `.to_string()`, and `Algorithm`'s own `FromStr` already
//! yields one), so the assertions match on the message, not on a variant.

use std::fs;
use std::path::{Path, PathBuf};

use collapse_desktop::commands::{compress_path, extract_archive, is_directory};
use tempfile::TempDir;

/// The three formats the UI offers. The wire spelling doubles as the archive
/// extension for all of them, which is why one array serves both roles.
const FORMATS: [&str; 3] = ["zip", "7z", "tar"];

/// Names a real user produces and a tidy-ASCII test suite never would: a
/// space, an accent, a `#`, a `%`, a leading dash (which a shell would read as
/// a flag) and a non-Latin script. They cross three boundaries here: the
/// arcname the command derives from the path, the entry name the backend
/// writes, and the file name extraction recreates.
const AWKWARD_NAMES: [&str; 6] = [
    "piñata report 2024 #1 (final)",
    "-dash",
    "100% done",
    "naïve café",
    "報告書",
    "a b  c",
];

fn compress_local(
    source: &Path,
    output: &Path,
    format: &str,
    level: u32,
) -> Result<String, String> {
    compress_local_with(source, output, format, level, false)
}

/// The same call with the caller reporting that the user agreed to replace
/// whatever is at `output`, which is what `App.vue` sends after the native save
/// dialog has asked.
fn compress_local_overwriting(
    source: &Path,
    output: &Path,
    format: &str,
    level: u32,
) -> Result<String, String> {
    compress_local_with(source, output, format, level, true)
}

fn compress_local_with(
    source: &Path,
    output: &Path,
    format: &str,
    level: u32,
    overwrite: bool,
) -> Result<String, String> {
    compress_path(
        source.to_string_lossy().into_owned(),
        output.to_string_lossy().into_owned(),
        format.to_string(),
        level,
        None,
        overwrite,
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

/// Deterministic pseudo-random prose: words drawn from a small vocabulary by a
/// linear congruential generator, so the bytes are compressible (repeated
/// words) without being trivial (a repeated literal collapses to the same
/// handful of bytes at every level, which is exactly what hid the level
/// argument from this suite before).
fn prose(bytes: usize) -> Vec<u8> {
    const WORDS: [&str; 16] = [
        "collapse", "archive", "compress", "desktop", "level", "entry", "folder", "bytes",
        "window", "native", "dialog", "server", "remote", "listing", "extract", "tarball",
    ];
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut out = Vec::with_capacity(bytes + 16);
    while out.len() < bytes {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        out.extend_from_slice(WORDS[((seed >> 33) as usize) % WORDS.len()].as_bytes());
        out.push(b' ');
    }
    out.truncate(bytes);
    out
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
    // folder. That agrees with the compression side, which selects the
    // directory branch with the same following `is_dir()`: a symlinked *root*
    // is archived (see the test below), while symlinked *children* are skipped
    // by core's `walk_tree`.
    assert!(is_directory(link_to_dir.to_string_lossy().into_owned()));
    assert!(!is_directory(link_to_file.to_string_lossy().into_owned()));
}

// ------------------------------------------------------- compress happy path --

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
        let returned = compress_local(&source, &archive, format, 3).unwrap();

        // The command echoes back the path it was asked to write (what the UI
        // shows afterwards), never the source it read.
        assert_eq!(returned, archive.to_string_lossy(), "{format}");
        assert_ne!(returned, source.to_string_lossy(), "{format}");

        let out_dir = dir.path().join("extracted");
        let extracted = extract_to(&archive, &out_dir).unwrap();

        assert_eq!(
            listing(extracted.clone()),
            vec!["notes.txt".to_string()],
            "{format}"
        );
        // Read through the returned entry rather than a hardcoded path: that is
        // what makes "the listing is relative to the output dir" load bearing.
        assert_eq!(
            fs::read(out_dir.join(&extracted[0])).unwrap(),
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

        // Files only, relative to the output dir, forward-slashed: the two
        // parent directories and `photos/empty` are created on disk but never
        // listed.
        assert_eq!(
            listing(extracted.clone()),
            vec![
                "photos/a.txt".to_string(),
                "photos/sub/b.txt".to_string(),
                "photos/sub/deep/c.txt".to_string(),
            ],
            "{format}: wrong entry shape"
        );
        // Each entry, joined to the output dir, is a file that is really on
        // disk: the listing names what extraction wrote, not merely what the
        // archive claimed to hold. (Containment is pinned by the equality
        // above, which admits no absolute or `..`-bearing entry.)
        for entry in &extracted {
            assert!(
                out_dir.join(entry).is_file(),
                "{format}: {entry} does not resolve to a file under the output dir"
            );
        }
        assert_eq!(
            fs::read(out_dir.join("photos").join("a.txt")).unwrap(),
            b"top level"
        );
        assert_eq!(
            fs::read(out_dir.join("photos").join("sub").join("b.txt")).unwrap(),
            b"one down"
        );
        assert_eq!(
            fs::read(
                out_dir
                    .join("photos")
                    .join("sub")
                    .join("deep")
                    .join("c.txt")
            )
            .unwrap(),
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
fn a_file_whose_name_is_not_tidy_ascii_round_trips_under_that_name() {
    // The desktop is where such names are the norm, and the arcname is derived
    // from the path by the command itself, so a mangled name here would ship a
    // wrong entry name to every archive the app writes.
    for format in FORMATS {
        for name in AWKWARD_NAMES {
            let dir = TempDir::new().unwrap();
            let file_name = format!("{name}.txt");
            let source = dir.path().join(&file_name);
            let content = format!("content of {name}").into_bytes();
            fs::write(&source, &content).unwrap();
            let archive = dir.path().join(format!("out.{format}"));

            compress_local(&source, &archive, format, 3)
                .unwrap_or_else(|e| panic!("{format} {file_name:?}: {e}"));

            let out_dir = dir.path().join("extracted");
            let extracted = extract_to(&archive, &out_dir)
                .unwrap_or_else(|e| panic!("{format} {file_name:?}: {e}"));

            assert_eq!(
                listing(extracted),
                vec![file_name.clone()],
                "{format}: the entry name did not survive"
            );
            assert_eq!(
                fs::read(out_dir.join(&file_name)).unwrap(),
                content,
                "{format} {file_name:?}: the bytes did not survive"
            );
        }
    }
}

#[test]
fn a_directory_whose_name_is_not_tidy_ascii_round_trips_under_that_name() {
    // Same names one level up: here the name becomes the prefix core's
    // `walk_tree` puts in front of every entry, and the child name comes back
    // from `read_dir` rather than from the caller's string.
    for format in FORMATS {
        for name in AWKWARD_NAMES {
            let dir = TempDir::new().unwrap();
            let root = dir.path().join(name);
            fs::create_dir(&root).unwrap();
            fs::write(root.join("café.txt"), b"inside").unwrap();
            let archive = dir.path().join(format!("out.{format}"));

            compress_local(&root, &archive, format, 3)
                .unwrap_or_else(|e| panic!("{format} {name:?}: {e}"));

            let out_dir = dir.path().join("extracted");
            let extracted =
                extract_to(&archive, &out_dir).unwrap_or_else(|e| panic!("{format} {name:?}: {e}"));

            assert_eq!(
                listing(extracted),
                vec![format!("{name}/café.txt")],
                "{format}: the folder prefix did not survive"
            );
            assert_eq!(
                fs::read(out_dir.join(name).join("café.txt")).unwrap(),
                b"inside",
                "{format} {name:?}: the bytes did not survive"
            );
        }
    }
}

#[test]
fn a_zero_byte_file_round_trips_as_a_zero_byte_file() {
    // Pins the real behaviour: all three backends accept an empty source and
    // give the entry back with a length of zero, rather than skipping it or
    // refusing the archive.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("empty.txt");
        fs::write(&source, b"").unwrap();
        let archive = dir.path().join(format!("out.{format}"));

        compress_local(&source, &archive, format, 3).unwrap_or_else(|e| panic!("{format}: {e}"));

        let out_dir = dir.path().join("extracted");
        let extracted = extract_to(&archive, &out_dir).unwrap_or_else(|e| panic!("{format}: {e}"));

        assert_eq!(
            listing(extracted),
            vec!["empty.txt".to_string()],
            "{format}"
        );
        assert_eq!(
            fs::metadata(out_dir.join("empty.txt")).unwrap().len(),
            0,
            "{format}: the empty file came back with bytes in it"
        );
    }
}

#[test]
fn an_empty_directory_comes_back_as_a_directory_with_an_empty_listing() {
    // Pins the real behaviour for the one archive shape the UI can produce
    // whose entries are *all* directories: compression succeeds, extraction
    // succeeds, and the listing is empty because directories are never listed.
    // The UI therefore reports "0 files" for a folder that did materialize,
    // which is worth knowing before anyone treats an empty listing as failure.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("nothing");
        fs::create_dir(&root).unwrap();
        let archive = dir.path().join(format!("out.{format}"));

        compress_local(&root, &archive, format, 3).unwrap_or_else(|e| panic!("{format}: {e}"));

        let out_dir = dir.path().join("extracted");
        let extracted = extract_to(&archive, &out_dir).unwrap_or_else(|e| panic!("{format}: {e}"));

        assert!(
            extracted.is_empty(),
            "{format}: a directory-only archive listed {extracted:?}"
        );
        assert!(
            out_dir.join("nothing").is_dir(),
            "{format}: the empty folder was not recreated"
        );
    }
}

#[test]
fn every_level_from_one_to_five_is_accepted_and_keeps_the_content() {
    // Acceptance only: that the number changes the *output* is the next test's
    // job. Extracting is what proves the archive holds the source, where a
    // length check would not: every one of these formats writes a header (and
    // zip a central directory) before any entry, so an archive holding nothing
    // at all is still comfortably longer than zero bytes.
    for format in FORMATS {
        for level in 1..=5 {
            let dir = TempDir::new().unwrap();
            let source = dir.path().join("notes.txt");
            let content = b"levels are 1..=5 ".repeat(32);
            fs::write(&source, &content).unwrap();
            let output = dir.path().join(format!("out.{format}"));

            compress_local(&source, &output, format, level)
                .unwrap_or_else(|e| panic!("{format} level {level} was refused: {e}"));

            let out_dir = dir.path().join("extracted");
            let extracted = extract_to(&output, &out_dir)
                .unwrap_or_else(|e| panic!("{format} level {level}: {e}"));
            assert_eq!(
                listing(extracted),
                vec!["notes.txt".to_string()],
                "{format} level {level}"
            );
            assert_eq!(
                fs::read(out_dir.join("notes.txt")).unwrap(),
                content,
                "{format} level {level}: the content did not survive"
            );
        }
    }
}

#[test]
fn the_level_reaches_the_backend_because_one_and_five_compress_differently() {
    // Without this, `compress_path` could drop its `level` argument and hand a
    // hardcoded 3 to core with the whole suite still green. A megabyte of
    // pseudo-random prose is the fixture because the gap has to be far outside
    // any noise: measured here, zip 183447 bytes at level 1 against 118492 at
    // level 5, and 7z 157920 against 98969.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("prose.txt");
    let content = prose(1_000_000);
    fs::write(&source, &content).unwrap();

    for format in ["zip", "7z"] {
        let mut sizes = Vec::new();
        for level in [1u32, 5] {
            let archive = dir.path().join(format!("level{level}.{format}"));
            compress_local(&source, &archive, format, level)
                .unwrap_or_else(|e| panic!("{format} level {level}: {e}"));
            sizes.push(fs::metadata(&archive).unwrap().len());

            // A smaller archive must still be the same archive: a mutation that
            // "compressed better" by dropping bytes would pass on size alone.
            let out_dir = dir.path().join(format!("out{level}{format}"));
            let extracted = extract_to(&archive, &out_dir).unwrap();
            assert_eq!(
                listing(extracted),
                vec!["prose.txt".to_string()],
                "{format}"
            );
            assert_eq!(
                fs::read(out_dir.join("prose.txt")).unwrap(),
                content,
                "{format} level {level}: the round-trip changed the bytes"
            );
        }
        assert!(
            sizes[0] > sizes[1],
            "{format}: level 1 produced {} bytes and level 5 {}, so the level never reached the backend",
            sizes[0],
            sizes[1]
        );
    }

    // tar is the exception and it is not forced: it stores without compressing,
    // so the level is validated and then ignored, and the two archives come out
    // byte for byte identical. Asserting that is the honest pin.
    let mut tars = Vec::new();
    for level in [1u32, 5] {
        let archive = dir.path().join(format!("level{level}.tar"));
        compress_local(&source, &archive, "tar", level).unwrap();
        tars.push(fs::read(&archive).unwrap());
    }
    assert_eq!(
        tars[0], tars[1],
        "tar started reacting to the level: the comment above is now wrong"
    );
}

#[test]
#[cfg(unix)]
fn a_symlinked_directory_is_archived_under_the_links_own_name() {
    // Pins the real behaviour, which the two `is_dir()` calls make inevitable:
    // the command follows a symlinked *root* (so the link's target is archived)
    // and stores it under the link's own name, while core's `walk_tree` skips
    // any symlinked child it meets inside the tree. Nothing refuses this, so
    // the app happily archives a folder the user only pointed at indirectly.
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("photos");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a.txt"), b"top level").unwrap();
    let link = dir.path().join("pictures");
    std::os::unix::fs::symlink(&root, &link).unwrap();

    let archive = dir.path().join("out.zip");
    compress_local(&link, &archive, "zip", 3).expect("a symlinked directory is accepted");

    let out_dir = dir.path().join("extracted");
    let extracted = extract_to(&archive, &out_dir).unwrap();

    assert_eq!(
        listing(extracted),
        vec!["pictures/a.txt".to_string()],
        "the target's content is archived under the link's name"
    );
    assert_eq!(
        fs::read(out_dir.join("pictures").join("a.txt")).unwrap(),
        b"top level"
    );
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
        false,
    )
    .expect("an empty server string must compress locally");

    assert_eq!(returned, output.to_string_lossy());

    // The same archive the `None` path produces, proven by opening it: "did not
    // error" would also be satisfied by an empty archive of the wrong source.
    let out_dir = dir.path().join("extracted");
    assert_eq!(
        extract_to(&output, &out_dir).unwrap(),
        vec!["notes.txt".to_string()]
    );
    assert_eq!(
        fs::read(out_dir.join("notes.txt")).unwrap(),
        b"local please"
    );
}

#[test]
fn a_whitespace_only_server_string_takes_the_remote_branch() {
    // KNOWN DEFECT, pinned rather than endorsed. The dispatcher filters on
    // `!s.is_empty()`, not on a trim, so a server string of blanks is treated
    // as a real destination: the compression leaves the machine (or tries to)
    // instead of running locally. A stale or half-cleared localStorage entry in
    // the settings sheet is enough to produce one. If the filter ever learns to
    // trim, this test should be rewritten to assert the local result, not
    // deleted.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"should have stayed here").unwrap();
    let output = dir.path().join("out.zip");

    let error = compress_path(
        source.to_string_lossy().into_owned(),
        output.to_string_lossy().into_owned(),
        "zip".to_string(),
        3,
        Some("   ".to_string()),
        false,
    )
    .expect_err("blanks are not a reachable server");

    // The wording is the remote client's, which is the proof the branch was
    // taken: no local error can mention a server.
    assert!(
        error.starts_with("cannot reach the server at"),
        "the remote branch must be what failed: {error}"
    );
    assert!(
        !output.exists(),
        "nothing was compressed locally, so no archive may exist"
    );
    assert_eq!(fs::read(&source).unwrap(), b"should have stayed here");
}

// ----------------------------------------------------------- compress guards --

#[test]
fn a_missing_source_is_refused_before_any_output_is_created() {
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope.txt");
        let output = dir.path().join(format!("out.{format}"));

        let err = compress_local(&missing, &output, format, 3).unwrap_err();

        assert_eq!(
            err,
            format!("Not found: {}", missing.to_string_lossy()),
            "{format}"
        );
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
    let dev_null = Path::new("/dev/null");
    assert!(
        dev_null.exists() && !dev_null.is_file() && !dev_null.is_dir(),
        "the fixture must be an existing non-file non-directory, or this test \
         is exercising the existence check instead"
    );
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("out.zip");

    let err = compress_local(dev_null, &output, "zip", 3).unwrap_err();

    assert_eq!(err, "Unsupported source (not a regular file or directory).");
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

            assert_eq!(
                err,
                format!("Invalid compression level: {level}. Must be between 1 and 5."),
                "{format} level {level}"
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

        assert_eq!(
            err, "Invalid compression level: 0. Must be between 1 and 5.",
            "{format}"
        );
        assert!(!output.exists(), "{format}: a stray output was left behind");
    }
}

#[test]
fn an_output_inside_a_missing_directory_fails_instead_of_panicking() {
    // The command does not create the parent directory of its output, so the
    // failure comes from the backend. The two spellings below are the real
    // messages a user would see; they differ because zip and tar reach it
    // through `File::create` (surfacing as `CompressionError::Io`) while the
    // 7z writer stringifies its own error into `Failed`.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"hello").unwrap();
        let missing_dir = dir.path().join("does_not_exist");
        let output = missing_dir.join(format!("out.{format}"));

        let err = compress_local(&source, &output, format, 3).unwrap_err();

        if format == "7z" {
            assert!(err.starts_with("Compression failed:"), "{format}: {err}");
            assert!(
                err.contains("NotFound") && err.contains("out.7z"),
                "the message must name the path it could not write: {err}"
            );
        } else {
            assert!(err.starts_with("IO error:"), "{format}: {err}");
            #[cfg(unix)]
            assert_eq!(
                err, "IO error: No such file or directory (os error 2)",
                "{format}"
            );
        }
        assert!(!output.exists(), "{format}: an archive appeared anyway");
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

        assert_eq!(
            err, "The output is the same file as the source.",
            "{format}"
        );
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
    assert_eq!(
        fs::read(&source).unwrap(),
        content,
        "the source was modified"
    );
    assert_eq!(
        fs::read_link(&alias).unwrap(),
        source,
        "the symlink itself was replaced"
    );
}

#[test]
fn a_hardlink_to_the_source_is_refused_even_though_the_paths_differ() {
    // Two names for one file: the canonical paths are genuinely different, so
    // only the filesystem-identity comparison catches this one. Not
    // `cfg(unix)`: `hard_link` needs no privilege on Windows either, and this
    // is the exact command-level reproduction that used to destroy the source
    // there (the identity comparison was Unix-only, so the guard saw two
    // different paths and let the archive through). Only the link-count
    // assertion at the end is Unix-only, for want of a stable API.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    let content = b"irreplaceable".to_vec();
    fs::write(&source, &content).unwrap();
    let hardlink = dir.path().join("also_notes.zip");
    fs::hard_link(&source, &hardlink).unwrap();

    // Without this the fixture could degenerate into two spellings of one path,
    // and the test would no longer reach the inode branch it is named after.
    assert_ne!(
        source.canonicalize().unwrap(),
        hardlink.canonicalize().unwrap(),
        "the fixture is pointless unless the two names canonicalize apart"
    );

    let err = compress_local(&source, &hardlink, "zip", 3).unwrap_err();

    assert_eq!(err, "The output is the same file as the source.");
    assert_eq!(
        fs::read(&source).unwrap(),
        content,
        "the source was modified"
    );
    // Reading the other name is the portable half of the link-count check
    // below: one file under two names, so an archive written through either
    // shows up in both.
    assert_eq!(
        fs::read(&hardlink).unwrap(),
        content,
        "the output name holds an archive, so the source was written over"
    );
    // A backend that unlinked the output path and created a fresh archive there
    // would leave the source readable but drop the link count to 1. Unix only:
    // Windows keeps the same count, but `std` exposes it behind the unstable
    // `windows_by_handle` feature, so a stable Windows build cannot read it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(&source).unwrap().nlink(),
            2,
            "the hardlink was replaced instead of being refused"
        );
    }
}

#[test]
fn the_first_failing_guard_is_the_one_reported() {
    // Order is source exists, source kind, format parse, same file, output
    // exists. None of these orderings is data-loss relevant on its own (all
    // five return before any backend runs, so nothing is written whichever
    // fires); what they decide is
    // which message the user gets, and a "Unknown algorithm" shown for a path
    // that is simply not there sends people hunting the wrong problem. The
    // ordering that *is* data-loss relevant, same file before any write or
    // upload, is pinned with effect checks by
    // `compressing_a_file_onto_itself_is_refused_and_leaves_it_untouched` and by
    // `an_output_equal_to_the_source_is_refused_before_any_network_io` in
    // tests/remote.rs. Those two also pin the last pair by construction: an
    // output equal to the source obviously exists, so asserting the exact
    // same-file message is what proves the coarser "already exists" refusal
    // does not fire first and bury the useful one.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"irreplaceable").unwrap();

    // Existence before the format parse.
    let missing = dir.path().join("nope.txt");
    let err = compress_local(&missing, &dir.path().join("out.rar"), "rar", 3).unwrap_err();
    assert_eq!(err, format!("Not found: {}", missing.to_string_lossy()));

    // Source kind before the format parse.
    #[cfg(unix)]
    {
        let err = compress_local(
            Path::new("/dev/null"),
            &dir.path().join("out.rar"),
            "rar",
            3,
        )
        .unwrap_err();
        assert_eq!(err, "Unsupported source (not a regular file or directory).");
    }

    // Format parse before the same-file check.
    let err = compress_local(&source, &source, "rar", 3).unwrap_err();
    assert_eq!(err, "Unknown algorithm: rar");
    assert_eq!(fs::read(&source).unwrap(), b"irreplaceable");
}

#[test]
fn an_existing_output_is_refused_and_left_untouched() {
    // The desktop used to overwrite whatever sat at the output path, unlike the
    // CLI, which refuses without --force. It now refuses too, and deleting is
    // left to whoever owns the file: this command cannot tell an archive nobody
    // wants from the only copy of something.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"hello").unwrap();
    let output = dir.path().join("out.zip");
    fs::write(&output, b"an older archive nobody asked to replace").unwrap();

    let err = compress_local(&source, &output, "zip", 3).unwrap_err();

    assert_eq!(
        err,
        format!(
            "The output already exists: {}. Delete it first, or choose another name.",
            output.display()
        )
    );
    assert_eq!(
        fs::read(&output).unwrap(),
        b"an older archive nobody asked to replace",
        "the refusal must leave the existing file exactly as it was"
    );

    // Removing it is all the caller has to do, which is what makes the refusal
    // a speed bump rather than a dead end.
    fs::remove_file(&output).unwrap();
    compress_local(&source, &output, "zip", 3).expect("the path is free now");
    assert_eq!(&fs::read(&output).unwrap()[..2], b"PK");
}

#[test]
fn an_existing_output_is_replaced_when_the_caller_says_the_user_agreed() {
    // `overwrite` is what the UI sends after the native save dialog has asked
    // "replace?", so refusing there would contradict a prompt the user just
    // answered. The refusal is for callers that never asked.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"hello").unwrap();
    let output = dir.path().join("out.zip");
    fs::write(&output, b"an older archive the user agreed to replace").unwrap();

    compress_local_overwriting(&source, &output, "zip", 3).expect("the user agreed");

    // Opened rather than merely "changed": an empty file would also differ.
    let out_dir = dir.path().join("extracted");
    assert_eq!(
        listing(extract_to(&output, &out_dir).unwrap()),
        vec!["notes.txt".to_string()]
    );
    assert_eq!(fs::read(out_dir.join("notes.txt")).unwrap(), b"hello");
}

#[test]
fn replacing_an_output_writes_through_a_hardlink_to_it() {
    // KNOWN LIMITATION, pinned rather than endorsed. The archive is not written
    // to a temporary file and renamed into place, so replacing an output that
    // happens to be a hardlink writes through the shared inode and takes the
    // other name down with it. Unlinking first would fix this and cost more
    // than it is worth: nothing is written until the archive is fully in hand,
    // which is what lets a failed run leave the previous archive untouched (see
    // the truncated-download tests in tests/remote.rs). Writing to a temporary
    // file and renaming would buy both, and is the real fix if this ever bites.
    //
    // Not `cfg(unix)`: nothing here is Unix-only, and NTFS hardlinks share
    // their data the same way, so the limitation is shipped on every platform
    // and is pinned on every platform.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"hello").unwrap();
    let output = dir.path().join("out.zip");
    fs::write(&output, b"an older archive the user agreed to replace").unwrap();
    let bystander = dir.path().join("someone-elses-copy.zip");
    fs::hard_link(&output, &bystander).unwrap();

    compress_local_overwriting(&source, &output, "zip", 3).expect("the user agreed");

    assert_ne!(
        fs::read(&bystander).unwrap(),
        b"an older archive the user agreed to replace",
        "the hardlink kept its content, so the write stopped going through the \
         inode: update this test"
    );
}

#[test]
fn an_output_landing_on_a_member_of_the_source_tree_is_refused_even_with_consent() {
    // The data loss this whole guard exists for. The backends list the tree
    // BEFORE creating the archive, so an output landing on an existing member
    // used to be truncated to hold the archive's own header bytes: the original
    // was then unrecoverable from the archive as much as from disk, and the
    // archive was corrupt too. Verified through the CLI as well, where only the
    // clobber check stood in the way.
    //
    // `overwrite` cannot unlock it. Answering "replace this file?" is not
    // agreeing to lose the file AND get a broken archive, which is the only
    // outcome available here, so consent is not the missing ingredient.
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("photos");
    fs::create_dir_all(&root).unwrap();
    let victim = root.join("a.txt");
    fs::write(&victim, b"irreplaceable member").unwrap();
    fs::write(root.join("b.txt"), b"other member").unwrap();

    let expected = format!(
        "The output is inside the folder being compressed: {}. \
         It would be destroyed instead of archived. Choose a location outside it.",
        victim.display()
    );

    for format in FORMATS {
        for consented in [false, true] {
            let err = compress_local_with(&root, &victim, format, 3, consented).unwrap_err();
            assert_eq!(err, expected, "{format}, overwrite={consented}");
            assert_eq!(
                fs::read(&victim).unwrap(),
                b"irreplaceable member",
                "{format}, overwrite={consented}: the member must survive byte for byte"
            );
        }
    }

    // The sibling is untouched as well, so nothing was half written before the
    // refusal.
    assert_eq!(fs::read(root.join("b.txt")).unwrap(), b"other member");
}

#[test]
fn an_output_deeper_inside_the_source_tree_is_refused_too() {
    // Containment is not just "the same directory": a nested member is the
    // same hazard, and comparing parents rather than resolved prefixes would
    // miss it.
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("photos");
    fs::create_dir_all(root.join("sub").join("deep")).unwrap();
    let victim = root.join("sub").join("deep").join("c.txt");
    fs::write(&victim, b"buried but still doomed").unwrap();

    let err = compress_local_overwriting(&root, &victim, "zip", 3).unwrap_err();

    assert!(
        err.starts_with("The output is inside the folder being compressed:"),
        "{err}"
    );
    assert_eq!(fs::read(&victim).unwrap(), b"buried but still doomed");
}

#[test]
fn an_output_hardlinked_to_a_member_of_the_source_tree_is_refused_even_with_consent() {
    // The same data loss as the test above, reached the way a path comparison
    // cannot see: the output sits in a sibling folder, so nothing about where
    // it is looks wrong, but it is another name for a file inside the tree.
    // Truncating it truncates the member, which the backend then archives in
    // its truncated state, so the bytes are gone from the archive as much as
    // from disk. This is the second of the two reproductions that motivated
    // the fix (`collapse compress photos -o out/archive.zip --force`, where
    // `out/archive.zip` was a hardlink of `photos/a.txt`, destroyed `a.txt`).
    //
    // `overwrite` cannot unlock it, for the reason the containment guard gives
    // in general: agreeing to replace `out/archive.zip` is not agreeing to
    // lose a photo and get a corrupt archive in exchange.
    //
    // One format is enough here (the sibling test above covers all three):
    // every guard runs before an `Algorithm` is ever handed to core, so the
    // format cannot change the answer, only how the data would have been lost.
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("photos");
    fs::create_dir_all(&root).unwrap();
    let member = root.join("a.txt");
    fs::write(&member, b"irreplaceable member").unwrap();
    fs::write(root.join("b.txt"), b"other member").unwrap();
    let elsewhere = dir.path().join("out");
    fs::create_dir(&elsewhere).unwrap();
    let alias = elsewhere.join("archive.zip");
    // `hard_link` panics rather than falling back to a copy, so a fixture that
    // reached this line really is one file under two names. It needs no
    // privilege on Windows, which is why this test is not `cfg(unix)`.
    fs::hard_link(&member, &alias).unwrap();

    // The two halves that keep the fixture honest: the output is genuinely
    // outside the folder (so the path-prefix check cannot be what refuses it)
    // and the two names genuinely resolve apart (so the plain same-file check
    // cannot be either). Without both, this test would quietly stop exercising
    // the tree walk it exists for.
    assert!(
        !alias
            .canonicalize()
            .unwrap()
            .starts_with(root.canonicalize().unwrap()),
        "the output must sit outside the folder being compressed"
    );
    assert_ne!(
        member.canonicalize().unwrap(),
        alias.canonicalize().unwrap(),
        "the fixture is pointless unless the two names canonicalize apart"
    );

    let expected = format!(
        "The output is inside the folder being compressed: {}. \
         It would be destroyed instead of archived. Choose a location outside it.",
        alias.display()
    );

    for consented in [false, true] {
        let err = compress_local_with(&root, &alias, "zip", 3, consented).unwrap_err();
        assert_eq!(err, expected, "overwrite={consented}");
        assert_eq!(
            fs::read(&member).unwrap(),
            b"irreplaceable member",
            "overwrite={consented}: the member must survive byte for byte"
        );
        // Read back through the output name too: it is the same file, so an
        // archive written there is the member being destroyed, and this is
        // what fails first if the guard is ever reduced to a path comparison.
        assert_eq!(
            fs::read(&alias).unwrap(),
            b"irreplaceable member",
            "overwrite={consented}: an archive was written through the other name"
        );
    }

    // Nothing was half written before the refusal.
    assert_eq!(fs::read(root.join("b.txt")).unwrap(), b"other member");
}

#[test]
fn an_output_outside_the_source_tree_is_replaced_when_the_caller_says_the_user_agreed() {
    // The counterweight to the test above, and what stops the guard being
    // "refuse whenever the output already exists". Writing an archive of a
    // folder over an unrelated older file is the ordinary case of re-running a
    // backup, and it has to keep working.
    //
    // The stale file is given the exact bytes of a member on purpose: a tree
    // walk that compared content or size instead of file identity would call
    // it part of the folder and refuse this, and nothing else in the suite
    // would notice.
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("photos");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"irreplaceable member").unwrap();
    let elsewhere = dir.path().join("out");
    fs::create_dir(&elsewhere).unwrap();
    let output = elsewhere.join("archive.zip");
    fs::write(&output, b"irreplaceable member").unwrap();

    compress_local_overwriting(&root, &output, "zip", 3)
        .expect("an unrelated file outside the folder may be replaced");

    // Opened rather than merely "changed": the point is that the real archive
    // of the real tree is what landed there.
    let out_dir = dir.path().join("extracted");
    assert_eq!(
        listing(extract_to(&output, &out_dir).unwrap()),
        vec!["photos/a.txt".to_string()]
    );
    assert_eq!(
        fs::read(out_dir.join("photos").join("a.txt")).unwrap(),
        b"irreplaceable member"
    );
    assert_eq!(
        fs::read(root.join("a.txt")).unwrap(),
        b"irreplaceable member",
        "the source tree must come through untouched"
    );
}

#[test]
fn an_output_inside_the_source_tree_under_a_free_name_is_still_allowed() {
    // Deliberately NOT refused. The guard is about replacing a file, not about
    // containment: walk_tree snapshots the entries before the archive is
    // created, so an archive written under a name nothing occupies cannot
    // destroy anything and cannot contain itself. It just leaves the archive
    // inside the folder it describes, which is odd but is the caller's choice.
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("photos");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"first").unwrap();

    let inside = root.join("photos.zip");
    compress_local(&root, &inside, "zip", 3).expect("a free name inside the tree is allowed");

    let out_dir = dir.path().join("extracted");
    let extracted = extract_to(&inside, &out_dir).unwrap();
    assert_eq!(
        listing(extracted),
        vec!["photos/a.txt".to_string()],
        "the archive lists the tree as it was before the archive existed"
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
        assert_eq!(
            err,
            format!("Compression failed: Unknown archive extension: .{ext}")
        );
    }
}

#[test]
fn an_uppercase_extension_extracts_a_valid_archive() {
    // Was a known limitation: extension match used literal lowercase strings.
    // The match is now case-insensitive; this test records that the archive
    // extracts rather than being deleted when the behaviour changed.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"hello").unwrap();
    let lower = dir.path().join("quiet.zip");
    compress_local(&source, &lower, "zip", 3).unwrap();

    // A distinct base name, so this still works on a case-insensitive volume.
    let upper = dir.path().join("LOUD.ZIP");
    fs::copy(&lower, &upper).unwrap();

    assert_eq!(
        extract_to(&upper, &dir.path().join("out")).unwrap(),
        vec!["notes.txt".to_string()]
    );
    assert_eq!(
        extract_to(&lower, &dir.path().join("out_lower")).unwrap(),
        vec!["notes.txt".to_string()]
    );
}

#[test]
fn a_truncated_archive_is_reported_legibly_instead_of_panicking() {
    // Half a download, a full disk, a copy from a dying drive: the UI has to
    // show something a person can act on, and the process must survive. The
    // three messages below are the real ones, and they are pinned by their
    // recognizable half so a zip/7z/tar version bump does not rewrite the test
    // while a change of *variant* (an `IO error:` prefix, or the extension
    // error, meaning the dispatch went wrong) still fails it.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"repeatable filler content ".repeat(2000)).unwrap();
        let archive = dir.path().join(format!("out.{format}"));
        compress_local(&source, &archive, format, 3).unwrap();

        let whole = fs::read(&archive).unwrap();
        fs::write(&archive, &whole[..whole.len() / 2]).unwrap();

        let out_dir = dir.path().join("extracted");
        let err = extract_to(&archive, &out_dir).unwrap_err();

        assert!(err.starts_with("Compression failed:"), "{format}: {err}");
        assert!(
            !err.contains("Unknown archive extension"),
            "{format}: the extension dispatch failed before the archive was even read: {err}"
        );
        match format {
            // "Compression failed: invalid Zip archive: Could not find EOCD"
            "zip" => {
                assert!(err.contains("Zip"), "{err}");
                assert!(
                    !out_dir.exists(),
                    "zip refuses the archive before creating the output directory"
                );
            }
            // "Compression failed: Io(Error { kind: UnexpectedEof, message:
            //  \"failed to fill whole buffer\" }, \"\")"
            "7z" => {
                assert!(err.contains("UnexpectedEof"), "{err}");
                let leftovers: Vec<PathBuf> = fs::read_dir(&out_dir)
                    .map(|entries| entries.map(|e| e.unwrap().path()).collect())
                    .unwrap_or_default();
                assert!(leftovers.is_empty(), "7z left {leftovers:?} behind");
            }
            // "Compression failed: failed to unpack `<output dir>/notes.txt`"
            _ => {
                assert!(err.contains("failed to unpack"), "{err}");
                assert!(
                    err.contains("notes.txt"),
                    "the message names the entry: {err}"
                );
                // KNOWN DEFECT, pinned rather than endorsed: tar streams entries
                // straight to disk, so a failure part-way leaves a truncated
                // file where a complete one is expected, and the caller gets an
                // Err with no list of what was written. Nothing cleans it up.
                let partial = out_dir.join("notes.txt");
                assert!(
                    partial.is_file(),
                    "tar wrote nothing at all: update this test"
                );
                assert!(
                    fs::metadata(&partial).unwrap().len() < 52_000,
                    "the partial file is somehow complete: update this test"
                );
            }
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

        assert_eq!(
            first, second,
            "{format}: the second run reported differently"
        );
        assert_eq!(
            fs::read(out_dir.join("notes.txt")).unwrap(),
            b"archived content",
            "{format}: the local edit was not overwritten"
        );
    }
}

#[test]
fn extracting_into_a_directory_that_already_holds_files_leaves_them_alone() {
    // The dialog lets a user extract into any folder, most often one that is
    // already full. Extraction adds to it: it never empties the destination
    // first, and the returned listing names only what this archive wrote.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"archived content").unwrap();
        let archive = dir.path().join(format!("out.{format}"));
        compress_local(&source, &archive, format, 3).unwrap();

        let out_dir = dir.path().join("busy");
        fs::create_dir(&out_dir).unwrap();
        fs::write(out_dir.join("unrelated.txt"), b"keep me").unwrap();
        fs::create_dir(out_dir.join("unrelated_dir")).unwrap();

        let extracted = extract_to(&archive, &out_dir).unwrap();

        assert_eq!(
            listing(extracted),
            vec!["notes.txt".to_string()],
            "{format}"
        );
        assert_eq!(
            fs::read(out_dir.join("unrelated.txt")).unwrap(),
            b"keep me",
            "{format}: an unrelated file was clobbered"
        );
        assert!(
            out_dir.join("unrelated_dir").is_dir(),
            "{format}: an unrelated directory was removed"
        );
    }
}
