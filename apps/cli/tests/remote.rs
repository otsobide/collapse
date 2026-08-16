//! Tests for the CLI's remote compression path (`--server`), driven against
//! the real collapse-api router served in-process on an ephemeral port.

use clap::Parser;
use collapse_cli::{run, Cli, CliError, Command, Outcome};

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

/// Serve the real API router on an OS-assigned port, returning its base URL.
/// The server thread lives for the rest of the test process.
fn start_server() -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let router = collapse_api::build_router(collapse_api::DEFAULT_MAX_UPLOAD_MB);
            axum::serve(listener, router).await.unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

// A port from the "unassigned" range nothing listens on in practice: the
// guard-order tests point --server here to prove no request is ever made.
const UNREACHABLE: &str = "http://127.0.0.1:9";

// ------------------------------------------------------------------ parsing --

#[test]
fn server_flag_parses() {
    let cli = parse(&["collapse", "compress", "f.txt", "--server", "http://localhost:8000"]).unwrap();
    match cli.command {
        Command::Compress { server, .. } => {
            assert_eq!(server.as_deref(), Some("http://localhost:8000"));
        }
        _ => panic!("expected compress"),
    }
}

// --------------------------------------------------------------- round-trips --

#[test]
fn remote_compress_round_trips_for_every_format() {
    let server = start_server();
    for (fmt, ext) in [("zip", "zip"), ("7z", "7z"), ("tar", "tar")] {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("notes.txt");
        std::fs::write(&src, b"compressed far away").unwrap();
        let archive = dir.path().join(format!("out.{ext}"));

        let output = compressed_output(run_ok(&[
            "collapse", "compress",
            src.to_str().unwrap(),
            "-f", fmt, "-l", "2",
            "-o", archive.to_str().unwrap(),
            "--server", &server,
        ]));
        assert_eq!(output, archive);

        let out = dir.path().join("out");
        let files = collapse_core::extract(&archive, &out).unwrap();
        assert_eq!(files, vec!["notes.txt"], "{fmt}");
        assert_eq!(
            std::fs::read(out.join("notes.txt")).unwrap(),
            b"compressed far away",
            "{fmt}"
        );
    }
}

#[test]
fn remote_compress_defaults_output_beside_source() {
    // Trailing slash on the URL must not produce a double-slash request path.
    let server = format!("{}/", start_server());
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"remote default output").unwrap();

    let output = compressed_output(run_ok(&[
        "collapse", "compress", src.to_str().unwrap(), "--server", &server,
    ]));
    assert_eq!(output.file_name().unwrap(), "notes.txt.zip");

    let out = dir.path().join("out");
    assert_eq!(collapse_core::extract(&output, &out).unwrap(), vec!["notes.txt"]);
}

// ---------------------------------------------------------------- rejections --

#[test]
fn remote_compress_rejects_directories() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"a").unwrap();

    // Rejected client-side: the unreachable server proves no request is made.
    assert!(matches!(
        run_err(&["collapse", "compress", root.to_str().unwrap(), "--server", UNREACHABLE]),
        CliError::RemoteDirectory(_)
    ));
}

#[test]
fn remote_compress_unreachable_server_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"x").unwrap();

    let err = run_err(&["collapse", "compress", src.to_str().unwrap(), "--server", UNREACHABLE]);
    assert!(matches!(err, CliError::Remote(_)), "got {err:?}");
    // No partial output file is left behind.
    assert!(!dir.path().join("notes.txt.zip").exists());
}

// ------------------------------------------------------- safety guard order --

#[test]
fn remote_compress_refuses_existing_output_without_force() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"body").unwrap();
    let archive = dir.path().join("out.zip");
    std::fs::write(&archive, b"pre-existing").unwrap();

    // The overwrite guard fires before any network I/O (unreachable server).
    assert!(matches!(
        run_err(&[
            "collapse", "compress",
            src.to_str().unwrap(),
            "-o", archive.to_str().unwrap(),
            "--server", UNREACHABLE,
        ]),
        CliError::OutputExists(_)
    ));
    assert_eq!(std::fs::read(&archive).unwrap(), b"pre-existing");
}

#[test]
fn remote_compress_force_overwrites_existing_output() {
    let server = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"fresh content").unwrap();
    let archive = dir.path().join("out.zip");
    std::fs::write(&archive, b"stale").unwrap();

    run_ok(&[
        "collapse", "compress",
        src.to_str().unwrap(),
        "-o", archive.to_str().unwrap(),
        "--force",
        "--server", &server,
    ]);

    let out = dir.path().join("out");
    assert_eq!(collapse_core::extract(&archive, &out).unwrap(), vec!["notes.txt"]);
    assert_eq!(std::fs::read(out.join("notes.txt")).unwrap(), b"fresh content");
}

#[test]
fn remote_compress_refuses_to_overwrite_its_own_source() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("important.txt");
    std::fs::write(&src, b"IMPORTANT ORIGINAL CONTENT").unwrap();

    // Same no-data-loss guarantee as local mode, even with --force.
    assert!(matches!(
        run_err(&[
            "collapse", "compress",
            src.to_str().unwrap(),
            "-o", src.to_str().unwrap(),
            "--force",
            "--server", UNREACHABLE,
        ]),
        CliError::OutputIsSource(_)
    ));
    assert_eq!(std::fs::read(&src).unwrap(), b"IMPORTANT ORIGINAL CONTENT");
}
