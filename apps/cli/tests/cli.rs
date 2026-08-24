//! Tests for collapse-cli, driving the real clap parser in-process via
//! `Cli::try_parse_from` and asserting the filesystem effects of `run`.

use clap::Parser;
use collapse_cli::{run, Cli, CliError, Command, Outcome};
use collapse_core::compression::verify_archive;
use collapse_core::{Algorithm, Verify};

/// Parse an argv-style slice through the real CLI definition.
fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
    Cli::try_parse_from(args)
}

fn run_ok(args: &[&str]) -> Outcome {
    run(parse(args).expect("args should parse")).expect("command should succeed")
}

fn run_err(args: &[&str]) -> CliError {
    run(parse(args).expect("args should parse")).expect_err("command should fail")
}

fn compressed_output(outcome: Outcome) -> std::path::PathBuf {
    match outcome {
        Outcome::Compressed { output, .. } => output,
        other => panic!("expected compressed, got {other:?}"),
    }
}

/// The depth `run` says it checked the archive at.
///
/// `run_compress` binds that depth once and uses the same binding for the call
/// into the engine and for the `Outcome`, so this is what the engine was
/// given, not a second opinion about it.
fn checked_depth(outcome: Outcome) -> Option<Verify> {
    match outcome {
        Outcome::Compressed { checked, .. } => checked,
        other => panic!("expected compressed, got {other:?}"),
    }
}

/// Normalize and sort an extracted listing so the expectations read the same
/// on a platform whose path separator is not `/`.
///
/// Archive entry names are always forward-slashed (core builds them that way
/// when it walks the tree), but the listing `extract` hands back is rebuilt
/// from `Path` components, so it arrives as `photos\a.txt` on Windows.
/// Without this every nested expectation below would be a Unix-only
/// assertion. The normalized name still reads the file, because `Path::join`
/// accepts a forward slash on Windows too.
///
/// Same shape as `listing` in `apps/desktop/src-tauri/tests/commands.rs`.
fn listing(paths: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = paths.iter().map(|p| p.replace('\\', "/")).collect();
    out.sort();
    out
}

// ------------------------------------------------------------------ parsing --

#[test]
fn compress_parses_defaults() {
    let cli = parse(&["collapse", "compress", "file.txt"]).unwrap();
    match cli.command {
        Command::Compress {
            path,
            level,
            output,
            force,
            verify,
            format,
            server,
        } => {
            assert_eq!(path.to_str().unwrap(), "file.txt");
            assert_eq!(level, 3);
            assert!(output.is_none());
            assert!(format.is_none());
            assert!(!force);
            // Off unless asked for: the deeper check costs about twice the
            // work, so it can only ever be opt-in.
            assert!(!verify);
            assert!(server.is_none());
        }
        _ => panic!("expected compress"),
    }
}

#[test]
fn level_out_of_range_is_rejected() {
    assert!(parse(&["collapse", "compress", "f", "-l", "0"]).is_err());
    assert!(parse(&["collapse", "compress", "f", "-l", "6"]).is_err());
    assert!(parse(&["collapse", "compress", "f", "-l", "3"]).is_ok());
}

#[test]
fn unknown_format_is_rejected() {
    assert!(parse(&["collapse", "compress", "f", "-f", "rar"]).is_err());
    for fmt in ["zip", "7z", "tar"] {
        assert!(
            parse(&["collapse", "compress", "f", "-f", fmt]).is_ok(),
            "{fmt}"
        );
    }
}

#[test]
fn extract_output_defaults_to_current_dir() {
    let cli = parse(&["collapse", "extract", "a.zip"]).unwrap();
    match cli.command {
        Command::Extract { output, .. } => assert_eq!(output.to_str().unwrap(), "."),
        _ => panic!("expected extract"),
    }
}

// -------------------------------------------------------- compress round-trip --

#[test]
fn compress_file_round_trips_for_every_format() {
    for (fmt, ext) in [("zip", "zip"), ("7z", "7z"), ("tar", "tar")] {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("notes.txt");
        std::fs::write(&src, b"hello cli").unwrap();
        let archive = dir.path().join(format!("out.{ext}"));

        let output = compressed_output(run_ok(&[
            "collapse",
            "compress",
            src.to_str().unwrap(),
            "-f",
            fmt,
            "-l",
            "2",
            "-o",
            archive.to_str().unwrap(),
        ]));
        assert_eq!(output, archive);
        assert!(archive.exists(), "{fmt}: archive not created");

        let out = dir.path().join("out");
        let files = collapse_core::extract(&archive, &out).unwrap();
        assert_eq!(listing(files), vec!["notes.txt"], "{fmt}");
        assert_eq!(std::fs::read(out.join("notes.txt")).unwrap(), b"hello cli");
    }
}

/// The `c`/`e` aliases must actually compress and extract through `run`.
#[test]
fn compress_and_extract_aliases_round_trip() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("data.txt");
    std::fs::write(&src, b"aliased").unwrap();
    let archive = dir.path().join("data.zip");

    run_ok(&[
        "collapse",
        "c",
        src.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
    ]);
    assert!(archive.exists());

    let out = dir.path().join("out");
    let outcome = run_ok(&[
        "collapse",
        "e",
        archive.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    match outcome {
        Outcome::Extracted { files, .. } => assert_eq!(listing(files), vec!["data.txt"]),
        _ => panic!("expected extracted"),
    }
    assert_eq!(std::fs::read(out.join("data.txt")).unwrap(), b"aliased");
}

#[test]
fn compress_directory_archives_the_tree() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    std::fs::write(root.join("sub/b.txt"), b"b").unwrap();
    let archive = dir.path().join("photos.zip");

    run_ok(&[
        "collapse",
        "compress",
        root.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
    ]);

    let out = dir.path().join("out");
    let files = listing(collapse_core::extract(&archive, &out).unwrap());
    assert_eq!(files, vec!["photos/a.txt", "photos/sub/b.txt"]);
}

// ------------------------------------------------------------- default output --

// The default output is derived from the *canonicalized* source
// (`run_compress` canonicalizes first so `.`/`..` resolve), so the path these
// two tests get back is whatever `canonicalize` produced: `/private/var/...`
// on macOS, and a verbatim `\\?\C:\Users\...` on Windows. Only the file name
// is asserted, never the full text, because the full text is platform shaped.
//
// KNOWN DEFECT, not fixed here (production code is out of scope for this
// pass): that same value is what `main.rs` prints, so a Windows user is told
// `Created \\?\C:\Users\me\notes.txt.7z` instead of the path they typed. It is
// cosmetic (the archive lands in the right place) but it is Windows only, and
// it is why an expectation on the whole path would be wrong to add. Tests that
// pass `-o` are unaffected: that path is returned exactly as given.

#[test]
fn default_output_for_file_is_beside_source() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"x").unwrap();

    let output = compressed_output(run_ok(&[
        "collapse",
        "compress",
        src.to_str().unwrap(),
        "-f",
        "7z",
    ]));
    assert_eq!(output.file_name().unwrap(), "notes.txt.7z");
    assert!(output.exists());
}

#[test]
fn default_output_for_directory_is_dirname_archive_beside_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();

    let output = compressed_output(run_ok(&["collapse", "compress", root.to_str().unwrap()]));
    assert_eq!(output.file_name().unwrap(), "photos.zip");
    assert!(output.exists());
    let out = dir.path().join("out");
    assert_eq!(
        listing(collapse_core::extract(&output, &out).unwrap()),
        vec!["photos/a.txt"]
    );
}

// ---------------------------------------------------------- format inference --

#[test]
fn format_is_inferred_from_output_extension() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"body").unwrap();
    // No -f, but -o names a .7z → the archive must really be 7z.
    let archive = dir.path().join("backup.7z");
    run_ok(&[
        "collapse",
        "compress",
        src.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
    ]);

    let out = dir.path().join("out");
    assert_eq!(
        listing(collapse_core::extract(&archive, &out).unwrap()),
        vec!["notes.txt"]
    );
    assert_eq!(std::fs::read(out.join("notes.txt")).unwrap(), b"body");
}

// ------------------------------------------------------------------- tar level --

#[test]
fn tar_output_is_independent_of_level() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"same bytes regardless of level").unwrap();

    let a1 = dir.path().join("l1.tar");
    let a5 = dir.path().join("l5.tar");
    run_ok(&[
        "collapse",
        "compress",
        src.to_str().unwrap(),
        "-f",
        "tar",
        "-l",
        "1",
        "-o",
        a1.to_str().unwrap(),
    ]);
    run_ok(&[
        "collapse",
        "compress",
        src.to_str().unwrap(),
        "-f",
        "tar",
        "-l",
        "5",
        "-o",
        a5.to_str().unwrap(),
    ]);
    assert_eq!(std::fs::read(&a1).unwrap(), std::fs::read(&a5).unwrap());
}

// -------------------------------------------------------------------- verify --

/// Bytes deflate and LZMA2 cannot shrink, so the archive's data region is about
/// as long as the input and a byte flipped in the middle of the file is
/// certainly inside it rather than in a header or the listing. Same
/// construction, and same reason, as `incompressible` in core's `verify.rs`.
fn incompressible(len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| ((i as u64).wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

fn flip_byte(path: &std::path::Path, offset: usize) {
    let mut bytes = std::fs::read(path).unwrap();
    bytes[offset] ^= 0xFF;
    std::fs::write(path, &bytes).unwrap();
}

#[test]
fn verify_flag_parses() {
    let cli = parse(&["collapse", "compress", "f.txt", "--verify"]).unwrap();
    match cli.command {
        Command::Compress { verify, .. } => assert!(verify),
        _ => panic!("expected compress"),
    }
}

/// Which check `--verify` asks the engine for, established by running that
/// check rather than by reading the CLI's source: the depth `run` reports is
/// the one it handed the engine, so pointing it at a damaged archive says what
/// the flag bought.
///
/// The CLI cannot be made to write a corrupt archive (its compressors checksum
/// exactly the bytes they wrote), so the damage is done afterwards, to the
/// archive the CLI itself produced.
///
/// Falsifiable in both directions: map `--verify` to the listing check and the
/// second half stops failing; make the listing check read entry data too and
/// the first half stops passing. It leans on `run_compress` binding the depth
/// once for both the call and the report, which is why that binding is single
/// and says so.
#[test]
fn verify_asks_for_the_check_that_catches_a_corrupt_entry() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.bin");
    std::fs::write(&src, incompressible(8192)).unwrap();

    let compress_to = |archive: &std::path::Path, flag: Option<&str>| -> Verify {
        let mut args = vec![
            "collapse",
            "compress",
            src.to_str().unwrap(),
            "-o",
            archive.to_str().unwrap(),
        ];
        args.extend(flag);
        checked_depth(run_ok(&args)).expect("a local compression checks the archive it wrote")
    };

    let plain = dir.path().join("plain.zip");
    let deep = dir.path().join("deep.zip");
    let shallow_depth = compress_to(&plain, None);
    let deep_depth = compress_to(&deep, Some("--verify"));

    // Both archives are sound as written, so the difference between the two
    // depths is invisible until one of them is damaged. Halfway through the
    // file: zip puts the entry's data first and its listing at the end, and
    // this data does not compress, so the flip lands in the payload.
    for archive in [&plain, &deep] {
        let midpoint = std::fs::metadata(archive).unwrap().len() as usize / 2;
        flip_byte(archive, midpoint);
    }

    let expected = ["notes.bin".to_string()];
    assert!(
        verify_archive(&plain, Algorithm::Zip, &expected, shallow_depth).is_ok(),
        "the default depth reads the listing, which a flipped data byte leaves intact"
    );
    let caught = verify_archive(&deep, Algorithm::Zip, &expected, deep_depth)
        .expect_err("--verify must ask for the depth that reads every entry back");
    assert!(
        caught.to_string().contains("notes.bin"),
        "the failure names the entry that went bad: {caught}"
    );

    // And the same two answers in the engine's own words.
    assert_eq!(shallow_depth, Verify::Index);
    assert_eq!(deep_depth, Verify::Contents);
}

/// The deeper check must pass on a healthy archive, for every format and both
/// shapes. It runs on the archive's way to the destination, so a false
/// positive would not merely be noise: nothing would land at all, and
/// `--verify` would be unusable. The tree carries the two entries most likely
/// to trip a reader that assumes every entry has data, an empty file and an
/// empty directory.
///
/// Falsifiable: have the contents check read a directory entry as a stream, or
/// treat a zero-length entry as a short read, and this goes red.
#[test]
fn verify_still_lands_a_correct_archive_for_every_format() {
    for (fmt, ext) in [("zip", "zip"), ("7z", "7z"), ("tar", "tar")] {
        let dir = tempfile::TempDir::new().unwrap();

        let file = dir.path().join("notes.txt");
        std::fs::write(&file, b"hello verify").unwrap();
        let file_archive = dir.path().join(format!("file.{ext}"));
        let outcome = run_ok(&[
            "collapse",
            "compress",
            file.to_str().unwrap(),
            "-f",
            fmt,
            "-o",
            file_archive.to_str().unwrap(),
            "--verify",
        ]);
        assert_eq!(checked_depth(outcome), Some(Verify::Contents), "{fmt}");
        let out = dir.path().join("file-out");
        assert_eq!(
            listing(collapse_core::extract(&file_archive, &out).unwrap()),
            vec!["notes.txt"],
            "{fmt}"
        );

        let root = dir.path().join("photos");
        std::fs::create_dir_all(root.join("empty_dir")).unwrap();
        std::fs::write(root.join("a.txt"), b"alpha").unwrap();
        std::fs::write(root.join("empty.txt"), b"").unwrap();
        let dir_archive = dir.path().join(format!("tree.{ext}"));
        let outcome = run_ok(&[
            "collapse",
            "compress",
            root.to_str().unwrap(),
            "-f",
            fmt,
            "-o",
            dir_archive.to_str().unwrap(),
            "--verify",
        ]);
        assert_eq!(checked_depth(outcome), Some(Verify::Contents), "{fmt}");
        let out = dir.path().join("tree-out");
        assert_eq!(
            listing(collapse_core::extract(&dir_archive, &out).unwrap()),
            vec!["photos/a.txt", "photos/empty.txt"],
            "{fmt}"
        );
        assert!(
            out.join("photos").join("empty_dir").is_dir(),
            "{fmt}: the empty directory survived the round trip"
        );
    }
}

// ----------------------------------------------------------------- extraction --

#[test]
fn extract_lists_and_writes_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("data.bin");
    std::fs::write(&src, b"payload").unwrap();
    let archive = dir.path().join("data.zip");
    collapse_core::compress(&src, &archive, "data.bin", Algorithm::Zip, 1, Verify::Index).unwrap();

    let out = dir.path().join("out");
    let outcome = run_ok(&[
        "collapse",
        "extract",
        archive.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    match outcome {
        Outcome::Extracted { output_dir, files } => {
            assert_eq!(output_dir, out);
            assert_eq!(listing(files), vec!["data.bin"]);
        }
        _ => panic!("expected extracted"),
    }
    assert_eq!(std::fs::read(out.join("data.bin")).unwrap(), b"payload");
}

#[test]
fn extract_creates_deep_nested_output_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("data.bin");
    std::fs::write(&src, b"deep").unwrap();
    let archive = dir.path().join("data.zip");
    collapse_core::compress(&src, &archive, "data.bin", Algorithm::Zip, 1, Verify::Index).unwrap();

    let out = dir.path().join("a/b/c");
    assert!(!out.exists());
    run_ok(&[
        "collapse",
        "extract",
        archive.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_eq!(std::fs::read(out.join("data.bin")).unwrap(), b"deep");
}

#[test]
fn extract_unknown_extension_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let fake = dir.path().join("archive.rar");
    std::fs::write(&fake, b"not an archive").unwrap();
    assert!(matches!(
        run_err(&["collapse", "extract", fake.to_str().unwrap()]),
        CliError::Core(_)
    ));
}

/// The same case-insensitive match, reached by the other road: with no
/// `--format`, the CLI infers the format from the output's extension, so
/// `-o backup.7Z` used to fall through to the zip default and write a zip
/// under a name promising a 7z. That archive was then refused by this same
/// CLI, since extraction dispatched on the extension too.
#[test]
fn compress_infers_the_format_from_an_uppercase_output_extension() {
    for (shouted, magic) in [("BACKUP.7Z", &b"7z"[..]), ("BACKUP.ZIP", &b"PK"[..])] {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("notes.txt");
        std::fs::write(&src, b"body").unwrap();
        let archive = dir.path().join(shouted);

        run_ok(&[
            "collapse",
            "compress",
            src.to_str().unwrap(),
            "-o",
            archive.to_str().unwrap(),
        ]);

        let written = std::fs::read(&archive).unwrap();
        assert_eq!(
            &written[..magic.len()],
            magic,
            "{shouted} should hold what its name promises"
        );

        // And the round trip closes: the archive this CLI wrote is one it reads.
        let out = dir.path().join("out");
        assert_eq!(
            collapse_core::extract(&archive, &out).unwrap(),
            vec!["notes.txt"],
            "{shouted}"
        );
    }
}

// ------------------------------------------------------- safety / data loss --

#[test]
fn compress_refuses_existing_output_without_force() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"body").unwrap();
    let archive = dir.path().join("out.zip");
    std::fs::write(&archive, b"pre-existing").unwrap();

    // Without --force: refused, existing file untouched.
    assert!(matches!(
        run_err(&[
            "collapse",
            "compress",
            src.to_str().unwrap(),
            "-o",
            archive.to_str().unwrap()
        ]),
        CliError::OutputExists(_)
    ));
    assert_eq!(std::fs::read(&archive).unwrap(), b"pre-existing");

    // With --force: overwritten with a real archive.
    run_ok(&[
        "collapse",
        "compress",
        src.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
        "--force",
    ]);
    let out = dir.path().join("out");
    assert_eq!(
        listing(collapse_core::extract(&archive, &out).unwrap()),
        vec!["notes.txt"]
    );
}

#[test]
fn compress_refuses_an_output_inside_the_folder_being_compressed_even_with_force() {
    // The archive is aimed at a file that is part of the tree it would archive.
    // The backends list the tree before creating the output, so that file would
    // be truncated and then archived in its truncated state: lost from the
    // archive as much as from disk, and the archive corrupt with it. --force
    // cannot buy past it, the same way it cannot buy past OutputIsSource, and
    // for the same reason: the flag means "replace this file", not "destroy
    // part of what I asked you to keep".
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let victim = root.join("sub").join("a.txt");
    std::fs::write(&victim, b"irreplaceable member").unwrap();
    std::fs::write(root.join("b.txt"), b"other member").unwrap();

    for force in [false, true] {
        let mut args = vec![
            "collapse",
            "compress",
            root.to_str().unwrap(),
            "-o",
            victim.to_str().unwrap(),
        ];
        if force {
            args.push("--force");
        }
        assert!(
            matches!(run_err(&args), CliError::OutputInsideSource(_)),
            "force={force}"
        );
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"irreplaceable member",
            "force={force}: the member must survive byte for byte"
        );
    }

    assert_eq!(std::fs::read(root.join("b.txt")).unwrap(), b"other member");
}

#[test]
fn compress_allows_an_output_inside_the_source_tree_under_a_free_name() {
    // Deliberately NOT refused. walk_tree snapshots the entries before the
    // archive is created, so an archive written under a name nothing occupies
    // can neither destroy anything nor contain itself. The guard is about
    // replacing a file, not about geography.
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"first").unwrap();
    let inside = root.join("photos.zip");

    run_ok(&[
        "collapse",
        "compress",
        root.to_str().unwrap(),
        "-o",
        inside.to_str().unwrap(),
    ]);

    let out = dir.path().join("out");
    assert_eq!(
        listing(collapse_core::extract(&inside, &out).unwrap()),
        vec!["photos/a.txt"],
        "the archive lists the tree as it was before the archive existed"
    );
}

#[test]
fn compress_refuses_to_overwrite_its_own_source() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("important.txt");
    std::fs::write(&src, b"IMPORTANT ORIGINAL CONTENT").unwrap();

    // Even with --force, writing the archive onto the source is refused.
    assert!(matches!(
        run_err(&[
            "collapse",
            "compress",
            src.to_str().unwrap(),
            "-o",
            src.to_str().unwrap(),
            "--force"
        ]),
        CliError::OutputIsSource(_)
    ));
    // The original content is intact.
    assert_eq!(std::fs::read(&src).unwrap(), b"IMPORTANT ORIGINAL CONTENT");
}

/// Give `target` a second name at `link`, and prove that is what happened.
///
/// `hard_link` returns an error rather than quietly falling back to a copy, so
/// unwrapping it keeps a filesystem without hardlinks from turning these tests
/// green for the wrong reason. The two names must then canonicalize apart: if
/// they ever collapsed onto one path, the guards below would be satisfied by
/// plain path equality and would stop exercising file identity, which is the
/// only thing they exist to check.
///
/// Only the *inequality* of the two canonical paths is asserted, never their
/// text, because `canonicalize` hands back a verbatim `\\?\C:\...` path on
/// Windows and any assertion on the spelling would be a Unix-only one.
fn hard_link_apart(target: &std::path::Path, link: &std::path::Path) {
    std::fs::hard_link(target, link).expect("the fixture needs a real hardlink");
    assert_ne!(
        target.canonicalize().unwrap(),
        link.canonicalize().unwrap(),
        "two names for one file must not resolve to one path"
    );
}

/// A hardlink is not a copy and not a pointer: it *is* a name of the file, so
/// `alias.zip` and `notes.txt` are one file wearing two names. Writing the
/// archive to the alias wrote it onto the source, which then began with the zip
/// header "PK" while its content survived neither on disk nor in the archive.
/// Nothing about that is Unix specific and hardlinks need no privilege on
/// Windows, so this test is deliberately not cfg-gated.
///
/// Both readings of the flag are refused, and both by `OutputIsSource`: an
/// `OutputExists` without `--force` would mean the identity guard never ran and
/// the protection would vanish the moment a user passes `--force`.
#[test]
fn compress_refuses_a_hardlink_of_its_source_as_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"IMPORTANT ORIGINAL CONTENT").unwrap();
    let alias = dir.path().join("alias.zip");
    hard_link_apart(&src, &alias);

    for force in [false, true] {
        let mut args = vec![
            "collapse",
            "compress",
            src.to_str().unwrap(),
            "-o",
            alias.to_str().unwrap(),
        ];
        if force {
            args.push("--force");
        }
        assert!(
            matches!(run_err(&args), CliError::OutputIsSource(_)),
            "force={force}"
        );
        assert_eq!(
            std::fs::read(&src).unwrap(),
            b"IMPORTANT ORIGINAL CONTENT",
            "force={force}: the source must survive byte for byte"
        );
    }
}

/// The containment guard cannot be a path comparison either. This archive is
/// aimed at a path outside the folder, so by geography it is unrelated, but it
/// is a second name for a member of that folder: truncating it truncates the
/// member, which is then archived empty or lost outright. Only file identity
/// sees it, and the answer must be `OutputInsideSource` with or without
/// `--force`, never `OutputExists`.
#[test]
fn compress_refuses_an_output_hardlinked_to_a_member_of_the_folder() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(&root).unwrap();
    let member = root.join("a.txt");
    std::fs::write(&member, b"irreplaceable member").unwrap();
    std::fs::write(root.join("b.txt"), b"other member").unwrap();

    // The alias lives outside the folder, which is what defeated the old
    // path-only check.
    let elsewhere = dir.path().join("archives");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let alias = elsewhere.join("archive.zip");
    hard_link_apart(&member, &alias);

    for force in [false, true] {
        let mut args = vec![
            "collapse",
            "compress",
            root.to_str().unwrap(),
            "-o",
            alias.to_str().unwrap(),
        ];
        if force {
            args.push("--force");
        }
        assert!(
            matches!(run_err(&args), CliError::OutputInsideSource(_)),
            "force={force}"
        );
        assert_eq!(
            std::fs::read(&member).unwrap(),
            b"irreplaceable member",
            "force={force}: the member must survive byte for byte"
        );
    }

    assert_eq!(std::fs::read(root.join("b.txt")).unwrap(), b"other member");
}

/// The same hazard, with the victim two levels down, so the guard is pinned to
/// walk the whole tree and not just the folder's immediate children. A check
/// that only listed the top level would archive `photos` happily and destroy
/// `photos/sub/deeper/secret.txt` on the way.
#[test]
fn compress_refuses_an_output_hardlinked_to_a_file_in_a_nested_subfolder() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    let nested = root.join("sub").join("deeper");
    std::fs::create_dir_all(&nested).unwrap();
    let buried = nested.join("secret.txt");
    std::fs::write(&buried, b"buried member").unwrap();
    std::fs::write(root.join("a.txt"), b"top member").unwrap();

    let alias = dir.path().join("archive.zip");
    hard_link_apart(&buried, &alias);

    // --force is the destructive reading, so that is the one worth pinning here.
    let error = run_err(&[
        "collapse",
        "compress",
        root.to_str().unwrap(),
        "-o",
        alias.to_str().unwrap(),
        "--force",
    ]);
    assert!(matches!(error, CliError::OutputInsideSource(_)));
    assert_eq!(std::fs::read(&buried).unwrap(), b"buried member");
    assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"top member");
}

/// Negative control for the two guards above. The identity walk has to answer
/// "no" for a file that merely happens to exist nearby, or the source's safety
/// would have been bought by breaking `--force` for everyone. The source is a
/// folder, so the walk really runs and really has to come back empty handed.
#[test]
fn compress_with_force_still_overwrites_an_unrelated_existing_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();
    let archive = dir.path().join("photos.zip");
    std::fs::write(&archive, b"stale bytes, no relation to the tree").unwrap();

    run_ok(&[
        "collapse",
        "compress",
        root.to_str().unwrap(),
        "-o",
        archive.to_str().unwrap(),
        "--force",
    ]);

    let out = dir.path().join("out");
    assert_eq!(
        listing(collapse_core::extract(&archive, &out).unwrap()),
        vec!["photos/a.txt"],
        "the stale file must have been replaced by a real archive"
    );
}

#[test]
fn compress_missing_source_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let missing = dir.path().join("ghost.txt");
    assert!(matches!(
        run_err(&["collapse", "compress", missing.to_str().unwrap()]),
        CliError::NotFound(_)
    ));
}

#[test]
fn extract_missing_archive_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let missing = dir.path().join("ghost.zip");
    assert!(run(parse(&["collapse", "extract", missing.to_str().unwrap()]).unwrap()).is_err());
}
