//! Tests for the HTTP client, against a real collapse-server-backend served in-process.
//!
//! These deliberately do NOT re-do the round trips that `apps/cli/tests/remote.rs`
//! already drives through a consumer. They cover what no consumer's suite can
//! reach: the health probe (which only a settings UI calls), the mapping of a
//! server rejection, the client-side refusal of an unusable source, and the
//! refusal of an unusable address at both entry points (the guard the
//! front-ends now lean on instead of each having their own).

use std::path::Path;
use std::sync::{Arc, Mutex};

use collapse_core::Algorithm;
use collapse_remote::{check_health, compress_path, RemoteError};

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
///
/// The responder is handed the request target and the request body, so a test
/// can also assert on what the client uploaded, not only on what it does with
/// the answer.
fn raw_server(
    respond: impl Fn(&str, &[u8], &mut std::net::TcpStream) + Send + Sync + 'static,
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
            let mut body = vec![0u8; length];
            if length > 0 {
                let _ = reader.read_exact(&mut body);
            }

            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            respond(&path, &body, &mut stream);
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
    // Half-close explicitly instead of letting the drop do it: an orderly FIN
    // is what tells the client "the body ends here", and dropping a socket is
    // the one place where that can come out as a reset instead, which the
    // client would report as a different error.
    let _ = out.shutdown(std::net::Shutdown::Write);
}

// --------------------------------------------------------- a blank address --

/// Both entry points refuse a blank address, so a front-end cannot forget to
/// ask. Nothing is listening on `""`, so without the guard these would come
/// back as `Unreachable` naming a server with no name, which is the message
/// issue #65 was filed about.
#[test]
fn a_blank_address_is_refused_by_both_entry_points() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"stay home").unwrap();

    for blank in ["", "   ", "\t"] {
        let error = check_health(blank).expect_err("a blank address is not a server");
        assert!(
            matches!(error, RemoteError::BlankServer),
            "check_health({blank:?}) gave {error:?}"
        );

        let error = compress_path(blank, &source, Algorithm::Zip, 3)
            .expect_err("a blank address is not a server");
        assert!(
            matches!(error, RemoteError::BlankServer),
            "compress_path({blank:?}) gave {error:?}"
        );
    }
}

/// The address is checked before the source is touched. A directory would
/// otherwise be walked and packed into a tar envelope in full before the
/// destination turns out to be unusable, and the error would name the wrong
/// problem: here a missing source proves the ordering with no tree to build.
#[test]
fn a_blank_address_is_refused_before_the_source_is_read() {
    let missing = Path::new("/nonexistent/collapse-issue-65/notes.txt");

    let error = compress_path("", missing, Algorithm::Zip, 3).expect_err("the address is blank");

    assert!(
        matches!(error, RemoteError::BlankServer),
        "the address must be judged before the source: got {error:?}"
    );
}

/// The same ordering for the branch it actually costs something in. A missing
/// file (above) is refused by a single `read`; a directory is walked and
/// copied into a tar on disk before a byte goes out, so a guard sitting after
/// `pack_directory` would spend the whole tree to learn the destination was
/// never usable. Only a real directory reaches that code, so only a real
/// directory can pin it.
///
/// A directory nobody may read is what makes the ordering observable: packing
/// it fails as `Packing`, so `BlankServer` can only be the answer if the
/// address was judged first. The unreachable-server call is the control that
/// proves the trap is armed rather than the test passing for want of anything
/// to trip over: with an address that is merely unusable, the same call really
/// does reach the packing and dies there.
///
/// Unix only (permissions), and skipped when the process can read the
/// directory anyway, which is what happens under root.
#[cfg(unix)]
#[test]
fn a_blank_address_is_refused_before_a_directory_is_packed() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("photos");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("a.txt"), b"never packed").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    if std::fs::read_dir(&locked).is_ok() {
        // Root ignores the permission bits, so the trap cannot be armed here.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let control = compress_path(UNREACHABLE, &locked, Algorithm::Zip, 3)
        .expect_err("the tree cannot be read, so it cannot be packed");
    assert!(
        matches!(control, RemoteError::Packing { .. }),
        "the control must fail in the packing this test claims to precede: got {control:?}"
    );

    let error = compress_path("   ", &locked, Algorithm::Zip, 3).expect_err("the address is blank");
    assert!(
        matches!(error, RemoteError::BlankServer),
        "a blank address must be refused before the tree is packed: got {error:?}"
    );

    // Restore, or the TempDir cannot clean itself up.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The road the first version of the blank guard left open, closed and pinned
/// where a user meets it.
///
/// An address of nothing but slashes is not blank, so an emptiness check on
/// the raw input let it through; it then lost its slashes and came back as
/// "cannot reach the server at : ", naming a server with no name, which is the
/// message issue #65 was filed about. Normalizing before deciding folds it into
/// the same refusal, and this asserts the user meets that refusal rather than
/// the transport failure.
#[test]
fn an_address_of_only_slashes_is_refused_by_name() {
    let error = check_health("///").expect_err("there is no address in a run of slashes");

    assert!(
        matches!(error, RemoteError::BlankServer),
        "expected the blank refusal, got {error:?}"
    );
    assert!(
        !error.to_string().contains("cannot reach the server at : "),
        "the nameless-server message is what this guard exists to prevent: {error}"
    );
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
        RemoteError::Rejected {
            status,
            ref message,
        } => {
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
    let server = raw_server(move |path, _body, out| {
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
    let server = raw_server(move |path, _body, out| {
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
    let server = raw_server(|path, _body, out| {
        if path.starts_with("/jobs/") {
            respond_with(
                out,
                4096,
                &FINISHED_JOB.as_bytes()[..20],
                "application/json",
            )
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

// ------------------------------------------------- the directory envelope --

/// What a directory looks like on the wire.
///
/// A tar's entry names are forward-slash separated by the format itself, on
/// every platform. A client that let a native separator through would upload
/// `photos\a.txt`, which is one flat file name to every other tool and to the
/// server's single-root check, so the failure would be a silently mangled
/// archive rather than an error. Nothing else in this crate's suite sends a
/// directory, so without this the shape of the envelope would only ever be
/// observed indirectly, on Unix, through the CLI's suite.
#[test]
fn a_directory_is_uploaded_as_a_tar_with_forward_slash_entry_names() {
    /// Requests the stub kept: the target (path and query) and the body.
    type Captured = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

    let uploads: Captured = Arc::default();
    let captured = Arc::clone(&uploads);

    let server = raw_server(move |path, body, out| {
        if path.starts_with("/compress") {
            captured
                .lock()
                .unwrap()
                .push((path.to_string(), body.to_vec()));
        }
        if path.ends_with("/download") {
            respond_with(out, 3, b"zip", "application/zip");
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
    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("a.txt"), b"first").unwrap();
    std::fs::write(root.join("sub/b.txt"), b"second").unwrap();

    compress_path(&server, &root, Algorithm::Zip, 3).expect("the stub completes the job");

    let (target, envelope) = uploads
        .lock()
        .unwrap()
        .pop()
        .expect("the directory reached the server");
    assert!(target.contains("envelope=tar"), "got {target}");
    assert!(target.contains("name=photos"), "got {target}");

    // Read the names out of the tar as bytes rather than as paths: the archive
    // is the contract, and a stray separator has to stay visible whatever
    // platform reads it back.
    let mut archive = tar::Archive::new(std::io::Cursor::new(envelope));
    let names: Vec<String> = archive
        .entries()
        .expect("the envelope is a tar")
        .map(|entry| String::from_utf8_lossy(&entry.expect("a readable entry").path_bytes()).into())
        .collect();

    // The two directory entries carry no trailing slash: they are typed as
    // directories in the header instead, which is what the server reads.
    assert_eq!(
        names,
        ["photos", "photos/a.txt", "photos/sub", "photos/sub/b.txt"],
        "the envelope must be forward-slash separated on every platform"
    );
}

// ------------------------------------------------------------ bad sources --

#[test]
fn a_path_with_no_name_cannot_be_uploaded() {
    // A root has no file name, so there is nothing to call the archive. `/`
    // is a root on Windows too (it parses as a bare root component, with no
    // name), and a drive root is the shape a Windows caller actually reaches:
    // a naive split on separators would hand the server `C:` as the arcname.
    // `cfg!` rather than `#[cfg]` so both arms keep compiling everywhere.
    let roots: &[&str] = if cfg!(windows) {
        &["/", "C:\\"]
    } else {
        &["/"]
    };

    for root in roots {
        let error = compress_path(UNREACHABLE, Path::new(root), Algorithm::Zip, 3)
            .expect_err("a root has no file name");
        assert!(
            matches!(error, RemoteError::Packing { .. }),
            "{root}: got {error:?}"
        );
    }
}

#[test]
fn a_missing_source_fails_before_any_request() {
    // Pointed at an unreachable server: reaching the network would be the bug.
    let dir = tempfile::tempdir().unwrap();
    let error = compress_path(
        UNREACHABLE,
        &dir.path().join("ghost.txt"),
        Algorithm::Zip,
        3,
    )
    .expect_err("the source does not exist");
    assert!(matches!(error, RemoteError::Io(_)), "got {error:?}");
}

// ------------------------------------------------------------- the backoff --

/// A job still compressing, so the client keeps polling.
const COMPRESSING_JOB: &str = r#"{"job_id":"stub","name":"notes.txt","archive_name":"notes.txt.zip","algorithm":"zip","level":3,"envelope":"none","status":"compressing","error_message":null}"#;

/// Issue #48: the wait between polls used to be a flat 200 ms from the first
/// one, so a job the server had already finished still cost the caller that
/// much of `wait_for_completion` sleeping.
///
/// The stub reports `compressing` three times and then `completed`, so the
/// client sleeps three times. Under the old schedule that was 600 ms; under
/// the backoff it is 10 + 20 + 40 = 70 ms. The assertion has a wide margin on
/// purpose, since a loaded CI runner is not a stopwatch, but 400 ms is still
/// far below what a fixed 200 ms interval could achieve here, so a revert
/// fails this rather than merely slowing it down.
#[test]
fn a_job_that_finishes_quickly_is_not_made_to_wait_out_the_ceiling() {
    let polls = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&polls);
    let archive = vec![b'Z'; 32];
    let served = archive.clone();

    let server = raw_server(move |path, _body, out| {
        if path.ends_with("/download") {
            respond_with(out, served.len(), &served, "application/zip");
            return;
        }
        if path.starts_with("/jobs/") {
            let mut seen = counter.lock().unwrap();
            *seen += 1;
            // The first three say "still working"; the fourth settles it.
            let body = if *seen <= 3 {
                COMPRESSING_JOB
            } else {
                FINISHED_JOB
            };
            respond_with(out, body.len(), body.as_bytes(), "application/json");
            return;
        }
        // POST /compress
        respond_with(
            out,
            FINISHED_JOB.len(),
            FINISHED_JOB.as_bytes(),
            "application/json",
        );
    });

    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"tiny").unwrap();

    let started = std::time::Instant::now();
    let delivered =
        compress_path(&server, &source, Algorithm::Zip, 3).expect("the stub finishes the job");
    let elapsed = started.elapsed();

    assert_eq!(delivered, archive);
    // It really did poll four times, so the timing below is measuring the
    // schedule and not a stub that answered "done" straight away.
    assert!(
        *polls.lock().unwrap() >= 4,
        "the stub was not polled as expected: {:?}",
        polls.lock().unwrap()
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(70),
        "it cannot have slept the schedule in {elapsed:?}"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(400),
        "three waits took {elapsed:?}; the fixed 200 ms interval is back"
    );
}
