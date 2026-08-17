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
