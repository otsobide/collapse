//! Tests for the HTTP client, against a real collapse-server-backend served in-process.
//!
//! These deliberately do NOT re-do the round trips that `apps/cli/tests/remote.rs`
//! already drives through a consumer. They cover what no consumer's suite can
//! reach: the health probe (which only a settings UI calls), the mapping of a
//! server rejection, and the client-side refusal of an unusable source.

use std::path::Path;

use collapse_remote::{check_health, compress_path, RemoteError};
use collapse_core::Algorithm;

/// Serve something on an ephemeral port for the rest of the test process.
///
/// The router is built by the closure *inside* the runtime: `build_app`
/// spawns its worker with `tokio::spawn`, which panics outside one.
fn serve(build: impl FnOnce() -> axum::Router + Send + 'static) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, build()).await.unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

/// A real collapse-server-backend, with its own staging directory.
fn collapse_server() -> String {
    let storage = tempfile::TempDir::new().unwrap();
    let path = storage.path().to_path_buf();
    std::mem::forget(storage); // the server outlives the test that started it
    serve(move || {
        collapse_server_backend::build_app(path, collapse_server_backend::DEFAULT_MAX_UPLOAD_MB)
            .expect("the server builds")
    })
}

// A port from the unassigned range: nothing listens there.
const UNREACHABLE: &str = "http://127.0.0.1:9";

// ------------------------------------------------------- a server that lies --

/// A job, already finished. Only `job_id` is read from the 202; the rest is
/// what a poll would return.
const FINISHED_JOB: &str = r#"{"job_id":"stub","name":"notes.txt","archive_name":"notes.txt.zip","algorithm":"zip","level":3,"envelope":"none","status":"completed","error_message":null}"#;

/// Speak HTTP by hand, so a response can promise one length and deliver
/// another. Nothing built on hyper will do that for you, and that is exactly
/// the case worth testing.
fn raw_server(
    respond: impl Fn(&str, &mut std::net::TcpStream) + Send + Sync + 'static,
) -> String {
    use std::io::{BufRead, Read};

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

            // The upload has to be read before answering, or the client is
            // writing into a socket nobody is draining.
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

            let path = request_line.split_whitespace().nth(1).unwrap_or("/").to_string();
            respond(&path, &mut stream);
        }
    });

    format!("http://{addr}")
}

/// Write a response whose `Content-Length` says `promised` while only `body`
/// goes out. `Connection: close` keeps each exchange to its own socket, so the
/// hang-up is unambiguous.
fn respond_with(out: &mut std::net::TcpStream, promised: usize, body: &[u8], content_type: &str) {
    use std::io::Write;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {promised}\r\nConnection: close\r\n\r\n"
    );
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(body);
    let _ = out.flush();
}

// ------------------------------------------------------------ health probe --

#[test]
fn check_health_accepts_a_real_server() {
    assert!(check_health(&collapse_server()).is_ok());
}

#[test]
fn check_health_reports_an_unreachable_address() {
    let error = check_health(UNREACHABLE).expect_err("nothing is listening");
    assert!(
        matches!(error, RemoteError::Unreachable { .. }),
        "got {error:?}"
    );
}

/// Something answered, but it is not Collapse. A settings dialog has to tell
/// this apart from a working server, or a typo pointing at some other service
/// would look fine until the first upload.
#[test]
fn check_health_rejects_a_server_that_is_not_collapse() {
    let impostor = || {
        axum::Router::new().route(
            "/health",
            axum::routing::get(|| async { axum::Json(serde_json::json!({ "status": "fine" })) }),
        )
    };
    let error = check_health(&serve(impostor)).expect_err("not a Collapse server");
    assert!(
        error.to_string().contains("does not look like"),
        "got {error}"
    );
}

// -------------------------------------------------------- server rejections --

/// The server validates too, and its reason has to reach the caller. No
/// consumer's suite hits this: the CLI clamps the level before it can.
#[test]
fn a_rejection_carries_the_servers_reason() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"x").unwrap();

    let error = compress_path(&collapse_server(), &source, Algorithm::Zip, 9)
        .expect_err("level 9 is out of range");

    match error {
        RemoteError::Rejected { status, ref message } => {
            assert_eq!(status, 400);
            assert!(message.contains("level"), "got {message}");
        }
        other => panic!("expected a rejection, got {other:?}"),
    }
}

// ------------------------------------------------- a download cut in half --

/// The property everything else leans on: a transfer that stops early is an
/// error, never a short archive handed back as if it were whole.
///
/// A server that goes away mid-download (a container stopping, a network that
/// drops) delivers exactly this: a `200 OK` whose body ends before its
/// `Content-Length`. The status was sent long before anything went wrong, so
/// only the length can give it away.
#[test]
fn a_download_that_stops_early_is_an_error_not_a_short_archive() {
    let promised = 200_000;
    let server = raw_server(move |path, out| {
        if path.ends_with("/download") {
            respond_with(out, promised, &vec![b'A'; promised / 2], "application/zip");
        } else {
            respond_with(
                out,
                FINISHED_JOB.len(),
                FINISHED_JOB.as_bytes(),
                "application/json",
            );
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"upload me").unwrap();

    let result = compress_path(&server, &source, Algorithm::Zip, 3);

    match result {
        Err(RemoteError::Io(_)) => {}
        Err(other) => panic!("expected the short body to surface as I/O, got {other:?}"),
        Ok(archive) => panic!(
            "a half-delivered archive was accepted as complete ({} of {promised} bytes)",
            archive.len()
        ),
    }
}

/// The control for the test above: the same stub telling the truth. Without
/// it, a broken harness would fail every call and the truncation test would
/// pass for the wrong reason.
#[test]
fn the_same_stub_telling_the_truth_delivers_the_archive() {
    let archive = vec![b'A'; 200_000];
    let served = archive.clone();
    let server = raw_server(move |path, out| {
        if path.ends_with("/download") {
            respond_with(out, served.len(), &served, "application/zip");
        } else {
            respond_with(
                out,
                FINISHED_JOB.len(),
                FINISHED_JOB.as_bytes(),
                "application/json",
            );
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"upload me").unwrap();

    let delivered = compress_path(&server, &source, Algorithm::Zip, 3)
        .expect("a complete response is accepted");
    assert_eq!(delivered, archive, "byte for byte what the server sent");
}

/// The same lie, told about a response the client parses rather than stores.
/// A half-read JSON body must not be silently treated as a malformed server or
/// worse, as an empty job.
#[test]
fn a_status_response_cut_short_does_not_look_like_a_finished_job() {
    let server = raw_server(|path, out| {
        if path.starts_with("/jobs/") {
            respond_with(out, 4096, &FINISHED_JOB.as_bytes()[..20], "application/json")
        } else {
            respond_with(
                out,
                FINISHED_JOB.len(),
                FINISHED_JOB.as_bytes(),
                "application/json",
            )
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"upload me").unwrap();

    let error = compress_path(&server, &source, Algorithm::Zip, 3)
        .expect_err("a truncated status response is not a completed job");
    assert!(
        matches!(error, RemoteError::Malformed(_) | RemoteError::Io(_)),
        "got {error:?}"
    );
}

// ------------------------------------------------------------ bad sources --

#[test]
fn a_path_with_no_name_cannot_be_uploaded() {
    // The root has no file name, so there is nothing to call the archive.
    let error = compress_path(UNREACHABLE, Path::new("/"), Algorithm::Zip, 3)
        .expect_err("the root has no file name");
    assert!(
        matches!(error, RemoteError::Packing { .. }),
        "got {error:?}"
    );
}

#[test]
fn a_missing_source_fails_before_any_request() {
    // Pointed at an unreachable server: reaching the network would be the bug.
    let dir = tempfile::tempdir().unwrap();
    let error = compress_path(UNREACHABLE, &dir.path().join("ghost.txt"), Algorithm::Zip, 3)
        .expect_err("the source does not exist");
    assert!(matches!(error, RemoteError::Io(_)), "got {error:?}");
}
