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

/// `RemoteError::BlankServer` rendered, which is the string `App.vue` puts in
/// its error banner. Spelled out whole so the tests below compare the sentence
/// a user reads, not a fragment of it: `apps/cli/tests/remote.rs` spells out
/// the same literal, and the two saying different things is the drift the
/// shared crate exists to prevent (issue #65).
const BLANK_ADDRESS: &str =
    "the server address is blank: it needs a URL, for example http://localhost:8000";

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
    compress_local_with(source, output, format, level, false, false)
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
    compress_local_with(source, output, format, level, true, false)
}

/// The same call with the Verify box ticked, which asks for the deeper of the
/// two checks: every entry decompressed rather than the listing alone.
fn compress_local_checking_contents(
    source: &Path,
    output: &Path,
    format: &str,
    level: u32,
) -> Result<String, String> {
    compress_local_with(source, output, format, level, false, true)
}

fn compress_local_with(
    source: &Path,
    output: &Path,
    format: &str,
    level: u32,
    overwrite: bool,
    verify: bool,
) -> Result<String, String> {
    compress_path(
        source.to_string_lossy().into_owned(),
        output.to_string_lossy().into_owned(),
        format.to_string(),
        level,
        None,
        overwrite,
        verify,
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

// ------------------------------------------------------------- verification --

/// Every file an archive holds, with its bytes, so two runs can be compared by
/// what they really contain rather than by the size of the file they produced.
fn contents_of(archive: &Path, into: &Path) -> Vec<(String, Vec<u8>)> {
    listing(extract_to(archive, into).expect("the archive extracts cleanly"))
        .into_iter()
        .map(|name| {
            let bytes = fs::read(into.join(&name)).expect("an extracted file is readable");
            (name, bytes)
        })
        .collect()
}

/// The Verify checkbox, both ways, for both source shapes and every format.
///
/// What this can prove from out here: ticking it is accepted rather than
/// refused, the archive it produces holds exactly what the unticked run's does,
/// and the staging the check runs on top of leaves nothing behind. What it
/// cannot prove is that `true` reaches core as the deeper depth: a compression
/// that succeeds looks identical at both depths by construction, since the
/// check only reads, and telling them apart needs an archive whose listing is
/// right and whose entry data is corrupt, which this command cannot be made to
/// produce. Core's own suite owns that half; here the parameter is held in
/// place by these three claims plus the frozen signature in tests/ipc.rs.
#[test]
fn checking_contents_is_accepted_and_yields_the_same_archive() {
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("notes.txt");
        fs::write(&file, prose(20_000)).unwrap();
        let tree = make_tree(dir.path());

        for (shape, source) in [("file", &file), ("tree", &tree)] {
            let mut by_depth = Vec::new();
            for (depth, checked) in [("listing", false), ("contents", true)] {
                // A folder of its own for the archive, so "what is beside it"
                // has exactly one right answer: nothing else writes here.
                let destination = dir.path().join(format!("{format}-{shape}-{depth}"));
                fs::create_dir(&destination).unwrap();
                let archive = destination.join(format!("out.{format}"));

                let call = if checked {
                    compress_local_checking_contents(source, &archive, format, 3)
                } else {
                    compress_local(source, &archive, format, 3)
                };
                call.unwrap_or_else(|e| panic!("{format} {shape}, {depth} check: {e}"));

                // The archive is built in a temporary beside the destination
                // and renamed in, so one that outlived the run shows up here.
                let mut beside: Vec<String> = fs::read_dir(&destination)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect();
                beside.sort();
                assert_eq!(
                    beside,
                    vec![format!("out.{format}")],
                    "{format} {shape}, {depth} check: something was left beside the archive"
                );

                by_depth.push(contents_of(
                    &archive,
                    &dir.path().join(format!("{format}-{shape}-{depth}-out")),
                ));
            }

            assert_eq!(
                by_depth[0], by_depth[1],
                "{format} {shape}: checking the contents changed what the archive holds"
            );
            assert!(
                !by_depth[0].is_empty(),
                "{format} {shape}: the comparison above passed on two empty archives"
            );
        }
    }
}

#[test]
fn the_data_loss_guards_hold_whatever_the_verify_box_says() {
    // Checking happens after an archive exists; the guards happen before
    // anything is read or written. Ticking the box must not reorder that, or
    // the app would destroy a source and then carefully verify the archive it
    // made out of it.
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"irreplaceable").unwrap();

        let err = compress_local_checking_contents(&source, &source, format, 3).unwrap_err();

        assert_eq!(
            err, "The output is the same file as the source.",
            "{format}"
        );
        assert_eq!(
            fs::read(&source).unwrap(),
            b"irreplaceable",
            "{format}: the source was modified"
        );
    }

    // The other irreversible one, with consent given as well, since that is
    // the combination the containment guard exists to refuse.
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("photos");
    fs::create_dir_all(&root).unwrap();
    let victim = root.join("a.txt");
    fs::write(&victim, b"irreplaceable member").unwrap();

    let err = compress_local_with(&root, &victim, "zip", 3, true, true).unwrap_err();

    assert_eq!(
        err,
        format!(
            "The output is inside the folder being compressed: {}. \
             It would be destroyed instead of archived. Choose a location outside it.",
            victim.display()
        )
    );
    assert_eq!(fs::read(&victim).unwrap(), b"irreplaceable member");
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

/// Drive `compress_path` with a blank `server` and assert the single answer
/// both spellings of blank now get: the address is named as the mistake,
/// nothing is written, and the source is untouched.
///
/// The wording belongs to `collapse-remote`, which is where the decision now
/// lives, and `apps/cli/tests/remote.rs` asserts the very same message. That
/// is the point of the fix: the two front-ends used to answer this
/// differently (issue #65), and the shared crate is what stops them drifting
/// again. "This computer" is `None`, covered by every local test above.
fn expect_blank_server_refusal(blank: &str) {
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"should have stayed here").unwrap();
    let output = dir.path().join("out.zip");

    let error = compress_path(
        source.to_string_lossy().into_owned(),
        output.to_string_lossy().into_owned(),
        "zip".to_string(),
        3,
        Some(blank.to_string()),
        false,
        false,
    )
    .expect_err("a blank address is not a server");

    // The whole message rather than fragments of it. The command stringifies
    // whatever `collapse-remote` returned, so equality is what pins that this
    // app adds nothing of its own to the sentence the CLI also prints; three
    // `contains` checks would all still pass if it started prefixing them.
    // ("cannot reach the server at    " named a server with no name and blamed
    // the network for what is a bad setting.)
    assert_eq!(error, BLANK_ADDRESS, "{blank:?}");

    // An archive on disk is what a silent local fallback looks like, and its
    // absence is the only thing that tells the two readings apart.
    assert!(
        !output.exists(),
        "{blank:?} was compressed locally instead of being reported"
    );
    assert_eq!(fs::read(&source).unwrap(), b"should have stayed here");
}

/// Was `an_empty_server_string_is_treated_as_local`, which pinned the
/// dispatcher's `!s.is_empty()` filter turning `""` into a local compression.
/// Nothing sends `""`: `App.vue` sends `null` for this computer, so an empty
/// string means a stale stored value or a caller's bug, and compressing
/// locally hid it. It is now refused, the same as on the CLI.
#[test]
fn an_empty_server_string_is_refused_not_compressed_locally() {
    expect_blank_server_refusal("");
}

/// Was `a_whitespace_only_server_string_takes_the_remote_branch`, the KNOWN
/// DEFECT this replaces: the old filter tested emptiness rather than trimming,
/// so a run of blanks was a real destination and the compression tried to
/// leave the machine. It is refused before any request now, with the same
/// message `""` gets, so the two spellings can no longer mean two things.
#[test]
fn a_whitespace_only_server_string_is_refused_not_sent() {
    for blank in ["   ", "\t", "\n"] {
        expect_blank_server_refusal(blank);
    }
}

/// The dispatch's other arm, and the one the old filter cost the most. With
/// `Some("")` read as "compress locally", pointing the app at a folder built
/// a complete archive of the tree on the very machine the user had told it
/// not to use, and nothing said so. A file cannot show that: it takes a
/// different route into `collapse-remote` (its own bytes, where a folder
/// travels as a tar envelope), so both arms have to be pinned.
#[test]
fn a_blank_server_string_is_refused_for_a_directory_too() {
    for blank in ["", "   "] {
        let dir = TempDir::new().unwrap();
        let tree = make_tree(dir.path());
        let output = dir.path().join("out.zip");

        let error = compress_path(
            tree.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
            "zip".to_string(),
            3,
            Some(blank.to_string()),
            false,
            false,
        )
        .expect_err("a blank address is not a server");

        assert_eq!(error, BLANK_ADDRESS, "{blank:?}");
        assert!(
            !output.exists(),
            "{blank:?} archived the whole tree locally instead of reporting the address"
        );
        // And the tree it would have archived is still there to archive.
        assert_eq!(fs::read(tree.join("a.txt")).unwrap(), b"top level");
    }
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
    // failure comes from the backend, and the message below is the real one a
    // user would see. All three formats now spell it the same way: 7z used to
    // answer with a `Debug` dump of its dependency's error, absolute output
    // path included, until core learned to map that error (issue #66).
    for format in FORMATS {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        fs::write(&source, b"hello").unwrap();
        let missing_dir = dir.path().join("does_not_exist");
        let output = missing_dir.join(format!("out.{format}"));

        let err = compress_local(&source, &output, format, 3).unwrap_err();

        assert!(err.starts_with("IO error:"), "{format}: {err}");
        #[cfg(unix)]
        assert_eq!(
            err, "IO error: No such file or directory (os error 2)",
            "{format}"
        );
        assert!(
            !err.contains(&format!("out.{format}")),
            "{format}: the message names the path the user picked: {err}"
        );
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
fn replacing_an_output_no_longer_writes_through_a_hardlink_to_it() {
    // This was a KNOWN LIMITATION and is now the opposite assertion. The
    // archive used to be written straight to the output path, so replacing an
    // output that happened to be a hardlink wrote through the shared inode and
    // took the other name down with it: someone else's copy of an old archive
    // silently became this one. Core now writes to a temporary beside the
    // destination and renames it in, and a rename replaces the *name*, so the
    // other name keeps the file it always had.
    //
    // Not `cfg(unix)`: NTFS hardlinks share their data the same way, so this is
    // a property of every platform the app ships to.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"hello").unwrap();
    let output = dir.path().join("out.zip");
    let previous = b"an older archive the user agreed to replace";
    fs::write(&output, previous).unwrap();
    let bystander = dir.path().join("someone-elses-copy.zip");
    fs::hard_link(&output, &bystander).unwrap();

    compress_local_overwriting(&source, &output, "zip", 3).expect("the user agreed");

    assert_eq!(
        fs::read(&bystander).unwrap(),
        previous,
        "the write went through the shared inode and destroyed a file nobody \
         named in the dialog"
    );
    // And the file the user did name really was replaced, so this is not a
    // refusal dressed up as a fix.
    let out_dir = dir.path().join("extracted");
    assert_eq!(
        listing(extract_to(&output, &out_dir).unwrap()),
        vec!["notes.txt".to_string()]
    );
    assert_eq!(fs::read(out_dir.join("notes.txt")).unwrap(), b"hello");
    // The two names are now two files, which is what a rename into place means
    // and what a write-through would have avoided.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(&bystander).unwrap().nlink(),
            1,
            "the archive still shares an inode with the bystander"
        );
        assert_ne!(
            fs::metadata(&output).unwrap().ino(),
            fs::metadata(&bystander).unwrap().ino(),
            "both names still point at one file"
        );
    }
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
            let err = compress_local_with(&root, &victim, format, 3, consented, false).unwrap_err();
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
        let err = compress_local_with(&root, &alias, "zip", 3, consented, false).unwrap_err();
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
fn an_uppercase_extension_extracts_like_any_other() {
    // The extension is a file name, not a wire value, and Windows and macOS
    // fold case in the filesystem, so a perfectly good zip called `.ZIP` used
    // to be refused as an unknown format for the spelling of its name alone.
    // This is where a user met that: whatever the open dialog handed back.
    let dir = TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    fs::write(&source, b"hello").unwrap();
    let lower = dir.path().join("quiet.zip");
    compress_local(&source, &lower, "zip", 3).unwrap();

    // A distinct base name, so this still works on a case-insensitive volume.
    for shouted in ["LOUD.ZIP", "Mixed.Zip", "SEVEN.7Z", "BALL.TAR"] {
        let renamed = dir.path().join(shouted);
        fs::copy(&lower, &renamed).unwrap();
        let out = dir.path().join(format!("out_{shouted}"));

        // .7Z and .TAR are the wrong format for these bytes, so they must fail
        // on the CONTENT, not on the name: the point is that the extension was
        // understood and the archive opened, which is the opposite of what a
        // rejected name looks like.
        let outcome = extract_to(&renamed, &out);
        match shouted {
            "LOUD.ZIP" | "Mixed.Zip" => assert_eq!(
                outcome.unwrap(),
                vec!["notes.txt".to_string()],
                "{shouted} is a zip and should read as one"
            ),
            _ => {
                let err = outcome.expect_err("zip bytes are not a 7z or a tar");
                assert!(
                    !err.contains("Unknown archive extension"),
                    "{shouted}: the name was understood, so the complaint must be \
                     about the bytes, got {err}"
                );
            }
        }
    }

    // An extension that is not one of ours is still refused, whatever its case:
    // this made the match lenient about spelling, not about formats.
    let foreign = dir.path().join("photos.RAR");
    fs::copy(&lower, &foreign).unwrap();
    assert_eq!(
        extract_to(&foreign, &dir.path().join("out_rar")).unwrap_err(),
        "Compression failed: Unknown archive extension: .RAR"
    );
}

#[test]
fn a_truncated_archive_is_reported_legibly_instead_of_panicking() {
    // Half a download, a full disk, a copy from a dying drive: the UI has to
    // show something a person can act on, and the process must survive. The
    // three messages below are the real ones, and they are pinned by their
    // recognizable half so a zip/7z/tar version bump does not rewrite the test
    // while the extension error (meaning the dispatch went wrong before the
    // archive was ever read) still fails it.
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

        assert!(
            !err.contains("Unknown archive extension"),
            "{format}: the extension dispatch failed before the archive was even read: {err}"
        );
        // No backend may answer with a struct dump, whatever its variant.
        assert!(
            !err.contains("Error {") && !err.contains("kind:"),
            "{format}: this is a Debug dump, not a sentence: {err}"
        );
        match format {
            // "Compression failed: invalid Zip archive: Could not find EOCD"
            "zip" => {
                assert!(err.starts_with("Compression failed:"), "{format}: {err}");
                assert!(err.contains("Zip"), "{err}");
                assert!(
                    !out_dir.exists(),
                    "zip refuses the archive before creating the output directory"
                );
            }
            // The short read reaches core as its dependency's `Io` variant, so
            // core unwraps it to `CompressionError::Io` (issue #66). It used to
            // read `Compression failed: Io(Error { kind: UnexpectedEof,
            // message: "failed to fill whole buffer" }, "")`.
            "7z" => {
                assert_eq!(err, "IO error: failed to fill whole buffer");
                let leftovers: Vec<PathBuf> = fs::read_dir(&out_dir)
                    .map(|entries| entries.map(|e| e.unwrap().path()).collect())
                    .unwrap_or_default();
                assert!(leftovers.is_empty(), "7z left {leftovers:?} behind");
            }
            // "Compression failed: failed to unpack `<output dir>/notes.txt`"
            _ => {
                assert!(err.starts_with("Compression failed:"), "{format}: {err}");
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
