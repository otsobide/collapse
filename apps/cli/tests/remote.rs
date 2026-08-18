//! Tests for the CLI's remote compression path (`--server`), driven against
//! the real collapse-server-backend router served in-process on an ephemeral port.

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

/// Serve the real API app on an OS-assigned port, returning its base URL and
/// its staging directory (to observe server-side cleanup). The server thread
/// (which keeps the staging TempDir alive) lives for the rest of the test
/// process.
fn start_server() -> (String, std::path::PathBuf) {
    let storage = tempfile::TempDir::new().unwrap();
    let storage_path = storage.path().to_path_buf();
    let app_storage = storage_path.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _storage = storage; // keep the staging dir alive with the server
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            let app = collapse_server_backend::build_app(
                app_storage,
                collapse_server_backend::DEFAULT_MAX_UPLOAD_MB,
            )
            .expect("the server builds");
            axum::serve(listener, app).await.unwrap();
        });
    });
    (format!("http://{}", rx.recv().unwrap()), storage_path)
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
    let (server, _storage) = start_server();
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
    let (base, _storage) = start_server();
    let server = format!("{base}/");
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

#[test]
fn remote_compress_cleans_up_the_job_server_side() {
    let (server, storage) = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"leave nothing behind").unwrap();

    run_ok(&["collapse", "compress", src.to_str().unwrap(), "--server", &server]);

    // The CLI deletes the job after downloading, so no job directory (input
    // or archive) survives in the server's staging area. The registry's own
    // database sits there too, as files rather than directories, which is
    // exactly what keeps the two apart.
    let leftovers: Vec<_> = std::fs::read_dir(&storage)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    assert!(leftovers.is_empty(), "job files left behind: {leftovers:?}");
}

// ---------------------------------------------------------------- rejections --

/// A directory travels as a tar envelope, and the archive that comes back
/// must be indistinguishable from one produced locally.
#[test]
fn remote_compress_a_directory_matches_the_local_result() {
    let (server, _storage) = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("a.txt"), b"first").unwrap();
    std::fs::write(root.join("sub/b.txt"), b"second").unwrap();
    std::fs::write(root.join("sub/deeper/c.txt"), b"third").unwrap();

    let remote_archive = dir.path().join("remote.zip");
    run_ok(&[
        "collapse", "compress",
        root.to_str().unwrap(),
        "-o", remote_archive.to_str().unwrap(),
        "--server", &server,
    ]);

    let local_archive = dir.path().join("local.zip");
    run_ok(&[
        "collapse", "compress",
        root.to_str().unwrap(),
        "-o", local_archive.to_str().unwrap(),
    ]);

    let extract_all = |archive: &std::path::Path, into: &str| {
        let out = dir.path().join(into);
        let mut files = collapse_core::extract(archive, &out).unwrap();
        files.sort();
        let contents: Vec<Vec<u8>> = files
            .iter()
            .map(|f| std::fs::read(out.join(f)).unwrap())
            .collect();
        (files, contents)
    };

    assert_eq!(extract_all(&remote_archive, "r"), extract_all(&local_archive, "l"));
    let (files, _) = extract_all(&remote_archive, "r2");
    assert_eq!(
        files,
        vec!["photos/a.txt", "photos/sub/b.txt", "photos/sub/deeper/c.txt"]
    );
}

/// tar is both the envelope and a target format, so this is the case where
/// the server tars, untars and tars again. It must still match local output.
#[test]
fn remote_compress_a_directory_to_tar_round_trips() {
    let (server, _storage) = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"tar inside tar").unwrap();

    let archive = dir.path().join("docs.tar");
    run_ok(&[
        "collapse", "compress",
        root.to_str().unwrap(),
        "-f", "tar",
        "-o", archive.to_str().unwrap(),
        "--server", &server,
    ]);

    let out = dir.path().join("out");
    assert_eq!(collapse_core::extract(&archive, &out).unwrap(), vec!["docs/a.txt"]);
    assert_eq!(std::fs::read(out.join("docs/a.txt")).unwrap(), b"tar inside tar");
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
    let (server, _storage) = start_server();
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

// ------------------------------------------------- a transfer that breaks --

/// A server that answers the job flow but hangs up half way through the
/// download, the way a stopped container does. Speaking HTTP by hand is what
/// lets the response promise one length and deliver another.
fn truncating_server() -> String {
    use std::io::{BufRead, Read, Write};

    const JOB: &str = r#"{"job_id":"stub","name":"notes.txt","archive_name":"notes.txt.zip","algorithm":"zip","level":3,"envelope":"none","status":"completed","error_message":null}"#;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());

            let mut request_line = String::new();
            if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                continue;
            }
            let mut length = 0usize;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                    break;
                }
                let lower = header.to_ascii_lowercase();
                if let Some(value) = lower.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            if length > 0 {
                let mut body = vec![0u8; length];
                let _ = reader.read_exact(&mut body);
            }

            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            let (promised, body, kind): (usize, Vec<u8>, &str) = if path.ends_with("/download") {
                (200_000, vec![b'A'; 100_000], "application/zip")
            } else {
                (JOB.len(), JOB.as_bytes().to_vec(), "application/json")
            };

            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {promised}\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });

    format!("http://{addr}")
}

/// A download that breaks must leave nothing behind. The archive is written
/// only once every byte is in hand, so a user is never handed a file that
/// looks like an archive and is not one.
#[test]
fn a_broken_download_writes_no_output_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("notes.txt");
    std::fs::write(&src, b"compress me").unwrap();
    let output = dir.path().join("notes.txt.zip");

    let error = run_err(&[
        "collapse",
        "compress",
        src.to_str().unwrap(),
        "--server",
        &truncating_server(),
        "-o",
        output.to_str().unwrap(),
    ]);

    assert!(
        matches!(error, CliError::Remote(_)),
        "the failure is reported as a remote one: {error:?}"
    );
    assert!(
        !output.exists(),
        "a half-delivered archive must not be left on disk"
    );
    assert!(src.exists(), "and the source is untouched");
}
