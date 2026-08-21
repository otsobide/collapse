//! Tests for collapse-cli, driving the real clap parser in-process via
//! `Cli::try_parse_from` and asserting the filesystem effects of `run`.

use clap::Parser;
use collapse_cli::{run, Cli, CliError, Command, Outcome};

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
        Outcome::Compressed { output } => output,
        other => panic!("expected compressed, got {other:?}"),
    }
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
            format,
            server,
        } => {
            assert_eq!(path.to_str().unwrap(), "file.txt");
            assert_eq!(level, 3);
            assert!(output.is_none());
            assert!(format.is_none());
            assert!(!force);
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
        assert_eq!(files, vec!["notes.txt"], "{fmt}");
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
        Outcome::Extracted { files, .. } => assert_eq!(files, vec!["data.txt"]),
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
    let mut files = collapse_core::extract(&archive, &out).unwrap();
    files.sort();
    assert_eq!(files, vec!["photos/a.txt", "photos/sub/b.txt"]);
}

// ------------------------------------------------------------- default output --

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
        collapse_core::extract(&output, &out).unwrap(),
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
        collapse_core::extract(&archive, &out).unwrap(),
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

// ----------------------------------------------------------------- extraction --

#[test]
fn extract_lists_and_writes_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("data.bin");
    std::fs::write(&src, b"payload").unwrap();
    let archive = dir.path().join("data.zip");
    collapse_core::compress(&src, &archive, "data.bin", collapse_core::Algorithm::Zip, 1).unwrap();

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
            assert_eq!(files, vec!["data.bin"]);
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
    collapse_core::compress(&src, &archive, "data.bin", collapse_core::Algorithm::Zip, 1).unwrap();

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
        collapse_core::extract(&archive, &out).unwrap(),
        vec!["notes.txt"]
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
