//! Tests for the desktop's remote compression path: `compress_path` with a
//! `server`, plus the `check_server` probe behind the settings sheet.
//!
//! Most of them run against a real `collapse-server-backend` served
//! in-process on an ephemeral port, the same harness `apps/cli/tests/remote.rs`
//! and `apps/remote/tests/client.rs` use. Every front end that speaks the job
//! flow proves it the same way, so a change in the exchange fails in all of
//! them at once instead of drifting in one. The exception is the pair of
//! truncated-download tests, which need a server that can lie about a
//! response length and so speak HTTP by hand.
//!
//! A remote archive is by design indistinguishable from a local one, so
//! comparing bytes can never prove the work crossed the wire: a dispatch that
//! quietly compressed folders locally would produce exactly the same file.
//! The harness therefore records every request the server received, and the
//! tests that have to rule out a local fallback assert on that log. It is the
//! one piece of evidence such a fallback cannot fake.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use collapse_core::compression::compress_tar_dir;
use collapse_desktop::commands::{check_server, compress_path, extract_archive};

// ------------------------------------------------------------------ harness --

/// One request as the server saw it.
struct Seen {
    /// Method, target and query, e.g. `POST /compress?name=notes.txt&...`.
    line: String,
    /// How many job directories were staged when the request arrived, which is
    /// how a test can tell "the client cleaned up after itself" from "no job
    /// was ever created".
    staged: usize,
}

/// A backend of this test's own, with a log of everything it was asked.
struct Server {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// `<storage>/jobs`: one directory per live job. The serving thread owns
    /// the TempDir above it, so this path stays valid for the whole run.
    jobs_dir: PathBuf,
}

impl Server {
    fn requests(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|seen| seen.line.clone())
            .collect()
    }

    /// The most jobs staged at once at any point the server was answering.
    fn peak_staged_jobs(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|seen| seen.staged)
            .max()
            .unwrap_or(0)
    }

    fn staged_jobs_now(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.jobs_dir)
            .expect("the job area exists")
            .map(|entry| entry.unwrap().path())
            .collect()
    }
}

/// Serve the real backend on an OS-assigned port. The thread owns the staging
/// TempDir, which is what keeps it alive for as long as the server answers.
///
/// The middleware in front of the router is what makes the exchange
/// observable: without it, no test in this file could tell a request that was
/// sent from one that never happened.
fn start_server() -> Server {
    let storage = tempfile::TempDir::new().unwrap();
    let app_storage = storage.path().to_path_buf();
    let jobs_dir = storage.path().join("jobs");
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));

    let recorder = seen.clone();
    let watched = jobs_dir.clone();
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
            .expect("the server builds")
            .layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let recorder = recorder.clone();
                    let watched = watched.clone();
                    async move {
                        let line = format!("{} {}", request.method(), request.uri());
                        let staged = std::fs::read_dir(&watched)
                            .map(|entries| entries.count())
                            .unwrap_or(0);
                        recorder.lock().unwrap().push(Seen { line, staged });
                        next.run(request).await
                    }
                },
            ));
            axum::serve(listener, app).await.unwrap();
        });
    });

    Server {
        url: format!("http://{}", rx.recv().unwrap()),
        seen,
        jobs_dir,
    }
}

/// One server for every test that just needs "a server that works". Jobs are
/// independent (own id, own staging directory, deleted once downloaded), so
/// sharing it introduces no ordering between tests. Its request log mixes the
/// tests running in parallel, so only its URL is handed out: a test that
/// asserts on the wire has to start its own.
fn shared_server() -> &'static str {
    static SERVER: OnceLock<Server> = OnceLock::new();
    SERVER.get_or_init(start_server).url.as_str()
}

/// The single `POST /compress` line a server was sent, query included. This is
/// where `name`, `algorithm`, `level` and `envelope` cross the wire, and its
/// absence is what a local fallback would look like from out here.
fn compress_request(server: &Server) -> String {
    let requests = server.requests();
    let mut uploads = requests
        .iter()
        .filter(|line| line.starts_with("POST /compress"));
    let first = uploads
        .next()
        .unwrap_or_else(|| panic!("no upload reached the server: {requests:?}"))
        .clone();
    assert!(
        uploads.next().is_none(),
        "one call must produce one upload: {requests:?}"
    );
    first
}

/// Port 9 is the discard service, which no ordinary machine runs, so nothing
/// listens there in practice and pointing at it proves a failure happened
/// without any HTTP exchange.
const UNREACHABLE: &str = "http://127.0.0.1:9";

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `compress_path` with a server, spelled the way the webview spells it.
fn compress_remotely(
    server: &str,
    source: &Path,
    output: &Path,
    format: &str,
    level: u32,
) -> Result<String, String> {
    compress_path(
        text(source),
        text(output),
        format.to_string(),
        level,
        Some(server.to_string()),
        false,
    )
}

/// The same call with no server, for the comparisons that pin "remote and
/// local produce the same archive".
fn compress_locally(source: &Path, output: &Path, format: &str, level: u32) -> String {
    compress_path(
        text(source),
        text(output),
        format.to_string(),
        level,
        None,
        false,
    )
    .expect("the local compression succeeds")
}

/// Extract through the command under test and return every file with its
/// bytes, sorted: comparing content is what proves a round trip, where
/// comparing archives byte for byte would only pin the compressor's mood.
///
/// Entry names come back with the platform's own separator (core builds them
/// from `Path::components()`), so they are forward-slashed here before they
/// are sorted or compared, exactly as `tests/commands.rs`'s `listing` does.
/// Without it every nested expectation below would be a Unix-only assertion.
/// The normalized name still reads the file, because `Path::join` accepts a
/// forward slash on Windows too.
fn extracted(archive: &Path, into: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<String> = extract_archive(text(archive), text(into))
        .expect("the archive extracts cleanly")
        .into_iter()
        .map(|name| name.replace('\\', "/"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|name| {
            let bytes = std::fs::read(into.join(&name)).expect("an extracted file is readable");
            (name, bytes)
        })
        .collect()
}

fn names(entries: &[(String, Vec<u8>)]) -> Vec<&str> {
    entries.iter().map(|(name, _)| name.as_str()).collect()
}

/// A tree with a nested subdirectory and an empty folder, the shape that
/// catches an envelope losing either depth or a directory entry.
fn make_tree(parent: &Path) -> PathBuf {
    let root = parent.join("photos");
    std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("a.txt"), b"first").unwrap();
    std::fs::write(root.join("sub/b.txt"), b"second").unwrap();
    std::fs::write(root.join("sub/deeper/c.txt"), b"third").unwrap();
    root
}

// -------------------------------------------------------------- round trips --

/// The whole point of the feature: bytes go out, an archive comes back and is
/// written to the path the command was handed. All three formats, because the
/// algorithm crosses the wire as a query parameter and a mismatch there would
/// only show up per format.
#[test]
fn a_file_compressed_on_the_server_round_trips_for_every_format() {
    let server = shared_server();
    for (format, extension) in [("zip", "zip"), ("7z", "7z"), ("tar", "tar")] {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        let body = b"compressed far away".to_vec();
        std::fs::write(&source, &body).unwrap();

        let output = dir.path().join(format!("notes.txt.{extension}"));
        let returned = compress_remotely(server, &source, &output, format, 2)
            .unwrap_or_else(|e| panic!("{format}: {e}"));

        assert!(
            output.is_file(),
            "{format}: no archive at the path asked for"
        );

        // Read back through the *returned* path rather than the requested one:
        // `App.vue` puts that string straight on screen as where the archive
        // landed, so it has to name the file that was really written.
        let entries = extracted(Path::new(&returned), &dir.path().join("out"));
        assert_eq!(entries, vec![("notes.txt".to_string(), body)], "{format}");
    }
}

/// The archive name travels as a query parameter, which is the one place in
/// this exchange where encoding can break: a space or an accent sent raw would
/// either corrupt the request line or come back mangled inside the archive.
#[test]
fn a_file_name_with_a_space_and_an_accent_survives_the_query_string() {
    let server = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let name = "año nuevo.txt";
    let source = dir.path().join(name);
    let body = b"feliz ano nuevo".to_vec();
    std::fs::write(&source, &body).unwrap();
    let output = dir.path().join(format!("{name}.zip"));

    compress_remotely(&server.url, &source, &output, "zip", 3).expect("the server compresses");

    let request = compress_request(&server);
    assert_eq!(
        request.split(' ').count(),
        2,
        "a raw space would split the request target in two: {request}"
    );
    assert!(
        request.is_ascii(),
        "the name must be percent-encoded, not sent as raw UTF-8: {request}"
    );
    assert!(
        request.contains("%C3%B1"),
        "the accent travels as percent-encoded UTF-8: {request}"
    );

    // And it comes back spelled exactly as it went out.
    let entries = extracted(&output, &dir.path().join("out"));
    assert_eq!(entries, vec![(name.to_string(), body)]);
}

/// A directory cannot travel over HTTP as it is, so the client packs it into a
/// tar envelope and the server unpacks it before compressing. The contract is
/// that none of that is visible in the result: same entries, same bytes, same
/// folder-name prefix as a local run.
///
/// The comparison alone cannot see the wire (a local fallback would match
/// itself perfectly), so the request log is what pins that the folder was
/// really uploaded, and uploaded as a tar envelope.
#[test]
fn a_directory_compressed_on_the_server_matches_a_local_run() {
    let server = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let root = make_tree(dir.path());

    let remote_archive = dir.path().join("remote.zip");
    compress_remotely(&server.url, &root, &remote_archive, "zip", 3)
        .expect("the server compresses");
    let local_archive = dir.path().join("local.zip");
    compress_locally(&root, &local_archive, "zip", 3);

    let request = compress_request(&server);
    assert!(
        request.contains("name=photos") && request.contains("envelope=tar"),
        "the folder must go out as a named tar envelope: {request}"
    );

    let remote = extracted(&remote_archive, &dir.path().join("r"));
    let local = extracted(&local_archive, &dir.path().join("l"));
    assert_eq!(remote, local, "the envelope must not change the result");
    assert_eq!(
        names(&remote),
        vec![
            "photos/a.txt",
            "photos/sub/b.txt",
            "photos/sub/deeper/c.txt"
        ],
        "entries keep the folder's own name as their prefix"
    );
    // Directories are excluded from the listing, so the only way to see an
    // empty folder is on disk. It has to survive the envelope, the unpack and
    // the compression, exactly as it survives a local run.
    for side in ["r", "l"] {
        assert!(
            dir.path().join(side).join("photos/empty").is_dir(),
            "the empty folder was lost on the {side} side"
        );
    }
}

/// tar is both the envelope and a target format, so here the server untars an
/// upload only to tar it again. It is the one combination where a confused
/// envelope would still produce a plausible-looking archive, which is why the
/// request that carries both spellings is asserted alongside the result.
#[test]
fn a_directory_compressed_to_tar_on_the_server_round_trips() {
    let server = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"tar inside tar").unwrap();

    let archive = dir.path().join("docs.tar");
    compress_remotely(&server.url, &root, &archive, "tar", 3).expect("the server compresses");

    let request = compress_request(&server);
    assert!(
        request.contains("algorithm=tar") && request.contains("envelope=tar"),
        "the target format and the envelope are two separate parameters: {request}"
    );

    let entries = extracted(&archive, &dir.path().join("out"));
    assert_eq!(
        entries,
        vec![("docs/a.txt".to_string(), b"tar inside tar".to_vec())]
    );
}

/// The envelope flag is decided by what the source *is*, never by what it is
/// called: `photos.tar` may well be a tarball a user wants compressed as a
/// single file. The source here is a genuine tar whose single root entry is a
/// directory, so a client that inferred the envelope from the extension would
/// not fail loudly, it would silently hand back an archive of the unpacked
/// tree instead of an archive of the file.
#[test]
fn a_file_named_like_a_tarball_is_compressed_as_a_file() {
    let server = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let tree = dir.path().join("staging/photos.tar");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join("inside.txt"), b"unpacked by mistake").unwrap();

    let tarball = dir.path().join("photos.tar");
    compress_tar_dir(&tree, &tarball).expect("the decoy tarball is built");
    let original = std::fs::read(&tarball).unwrap();

    let archive = dir.path().join("photos.tar.zip");
    compress_remotely(&server.url, &tarball, &archive, "zip", 3).expect("the server compresses");

    let request = compress_request(&server);
    assert!(
        request.contains("envelope=none"),
        "a file is a file whatever it is called: {request}"
    );

    let entries = extracted(&archive, &dir.path().join("out"));
    assert_eq!(
        entries,
        vec![("photos.tar".to_string(), original)],
        "the tarball must come back as one stored file, not as its contents"
    );
}

// ---------------------------------------------------------------- failures --

/// A server that is not there has to say so, by name, and must not leave a
/// stub file where the archive would have gone.
///
/// Both source shapes, because they take different routes to the wire (a file
/// is read and posted, a folder is packed into a tar envelope first). The
/// folder case is the one that makes the remote branch's directory half
/// falsifiable at all: if directories were quietly compressed locally, this
/// call would succeed instead of failing.
#[test]
fn an_unreachable_server_is_reported_for_a_file_and_for_a_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, b"never sent").unwrap();
    let folder = make_tree(dir.path());

    for (label, source) in [("file", file), ("directory", folder)] {
        let output = dir.path().join(format!("{label}.zip"));

        let error = match compress_remotely(UNREACHABLE, &source, &output, "zip", 3) {
            Err(error) => error,
            // The directory half of the dispatch is only remote if this call
            // needs the network. A local fallback would report success here.
            Ok(written) => panic!("{label}: nothing is listening, yet it wrote {written}"),
        };

        assert!(
            error.starts_with(&format!("cannot reach the server at {UNREACHABLE}:")),
            "{label}: the message names the unreachable server: {error}"
        );
        assert!(
            !output.exists(),
            "{label}: a half-written archive was left behind"
        );
    }
}

/// The level is not validated on this side of the wire, so the server's own
/// refusal is what the user sees: `App.vue` puts the returned string straight
/// into its error banner. This pins the wording that gets there, the server's
/// `detail` included, since a bare "HTTP 400" would be useless in a window.
#[test]
fn an_out_of_range_level_surfaces_the_servers_own_refusal() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"level nine does not exist").unwrap();
    let output = dir.path().join("notes.txt.zip");

    let error = compress_remotely(shared_server(), &source, &output, "zip", 9)
        .expect_err("level 9 is out of range");

    assert!(
        error.contains("the server rejected the request (HTTP 400)"),
        "got {error}"
    );
    assert!(
        error.contains("Invalid compression level: 9. Must be between 1 and 5."),
        "the server's own detail reaches the user: {error}"
    );
    assert!(!output.exists(), "a rejected job writes nothing");
}

// ------------------------------------------------- a transfer that breaks --

/// A server that answers the job flow but hangs up half way through the
/// download, the way a stopped container does. Speaking HTTP by hand is what
/// lets the response promise one length and deliver another; the real backend
/// cannot be made to lie like this. Ported from `apps/cli/tests/remote.rs`,
/// because the property it tests is the desktop's too.
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
            // The download promises 200 kB and delivers 100 kB, then closes.
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

/// The archive is written only once every byte is in hand, so a broken
/// transfer leaves nothing that looks like an archive and is not one. This is
/// the test that would catch someone streaming the download straight into the
/// output file: an unreachable server cannot, because it fails before the
/// first byte of the response exists.
#[test]
fn a_truncated_download_writes_no_output_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"compress me").unwrap();
    let output = dir.path().join("notes.txt.zip");

    let error = compress_remotely(&truncating_server(), &source, &output, "zip", 3)
        .expect_err("the download is cut short");

    assert!(
        error.starts_with("IO error:"),
        "a broken transfer is reported as the IO failure it is: {error}"
    );
    assert!(
        !output.exists(),
        "a half-delivered archive must not be left on disk"
    );
}

/// The destructive twin: the desktop has no clobber guard (the native save
/// dialog does the asking), so the output path routinely already holds a file.
/// A failed remote run must leave it exactly as it was.
#[test]
fn a_truncated_download_leaves_an_existing_output_untouched() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"compress me").unwrap();

    let output = dir.path().join("notes.txt.zip");
    let previous = b"an archive from a previous run";
    std::fs::write(&output, previous).unwrap();

    compress_remotely(&truncating_server(), &source, &output, "zip", 3)
        .expect_err("the download is cut short");

    assert_eq!(
        std::fs::read(&output).unwrap(),
        previous,
        "the archive that was already there was damaged by a failed run"
    );
}

// ------------------------------------------------------- guards before I/O --

// The first two tests here point at UNREACHABLE: nothing can be uploaded to
// it, so an exact match on the guard's own message is what proves the guard
// ran before any network I/O was attempted. Move either check below the
// dispatch and the call comes back with something else entirely. That
// ordering is the property, not the messages alone. The same-file guard
// needs a reachable server too, for the reason its own comment gives.

#[test]
fn a_missing_source_is_refused_before_any_network_io() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("ghost.txt");
    let output = dir.path().join("ghost.txt.zip");

    let error = compress_remotely(UNREACHABLE, &source, &output, "zip", 3)
        .expect_err("the source does not exist");

    assert_eq!(error, format!("Not found: {}", text(&source)));
    assert!(!output.exists());
}

#[test]
fn an_unknown_format_is_refused_before_any_network_io() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"body").unwrap();
    let output = dir.path().join("notes.rar");

    let error = compress_remotely(UNREACHABLE, &source, &output, "rar", 3)
        .expect_err("rar is not a format this toolkit knows");

    assert_eq!(error, "Unknown algorithm: rar");
    assert!(!output.exists());
}

/// The no-data-loss guarantee, unchanged by the destination.
///
/// Two destinations, because each makes a different half of the claim live.
/// Against an unreachable port the upload dies long before the archive would
/// be written back, so the source survives with or without the guard and only
/// the message is evidence. Against a real server, deleting the guard really
/// does replace the source with a zip of itself, so there the byte-for-byte
/// assertion is the one that fails; the empty request log adds that the file
/// was not even uploaded on the way to being destroyed.
/// The guard added for the data loss on the local side has to hold here too,
/// and for a stronger reason: the remote branch downloads the whole archive and
/// only then writes it, so a refusal that came too late would have burned an
/// upload, a compression and a download before destroying the file. The empty
/// request log is what proves the refusal happens before any of that.
#[test]
fn an_existing_output_is_refused_before_any_network_io() {
    let server = start_server();
    for destination in [UNREACHABLE, server.url.as_str()] {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        std::fs::write(&source, b"hello").unwrap();
        let output = dir.path().join("out.zip");
        std::fs::write(&output, b"an older archive nobody asked to replace").unwrap();

        let error = compress_remotely(destination, &source, &output, "zip", 3)
            .expect_err("the output already exists");

        assert_eq!(
            error,
            format!(
                "The output already exists: {}. Delete it first, or choose another name.",
                output.display()
            ),
            "{destination}"
        );
        assert_eq!(
            std::fs::read(&output).unwrap(),
            b"an older archive nobody asked to replace",
            "{destination}: the file that was already there must be untouched"
        );
    }

    let requests = server.requests();
    assert!(
        requests.is_empty(),
        "a request went out for an output that could never be written: {requests:?}"
    );
}

#[test]
fn an_output_equal_to_the_source_is_refused_before_any_network_io() {
    let server = start_server();
    for destination in [UNREACHABLE, server.url.as_str()] {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("important.txt");
        std::fs::write(&source, b"IMPORTANT ORIGINAL CONTENT").unwrap();

        let error = compress_remotely(destination, &source, &source, "zip", 3)
            .expect_err("the output is the source");

        assert_eq!(error, "The output is the same file as the source.");
        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"IMPORTANT ORIGINAL CONTENT",
            "{destination}: the source was modified"
        );
    }

    let requests = server.requests();
    assert!(
        requests.is_empty(),
        "the guard let the source reach the network: {requests:?}"
    );
}

// ----------------------------------------------------------- health probe --

/// What the settings sheet calls when a user adds a server: a typo has to
/// surface there, not at the end of an upload.
#[test]
fn check_server_accepts_a_real_server() {
    assert_eq!(check_server(shared_server().to_string()), Ok(()));
}

/// A trailing slash is what a user pastes from a browser, and it must not
/// produce a `//health` request path: the backend answers 404 to that, so a
/// lost trim turns a healthy server into a rejected one.
#[test]
fn check_server_accepts_a_real_server_with_a_trailing_slash() {
    assert_eq!(check_server(format!("{}/", shared_server())), Ok(()));
}

#[test]
fn check_server_reports_an_unreachable_address() {
    let error = check_server(UNREACHABLE.to_string()).expect_err("nothing is listening");

    let reason = error
        .strip_prefix(&format!("cannot reach the server at {UNREACHABLE}: "))
        .unwrap_or_else(|| panic!("the address the user typed is not named in: {error}"));
    // The probe reached the point of asking for an endpoint, which is what
    // separates a refused connection from an address no client could use.
    assert!(
        reason.contains("/health"),
        "the failure happened probing /health: {reason}"
    );
}

/// A blank address never leaves the machine: the probe says the address is
/// the problem instead of reporting a server with no name as unreachable.
/// `sources.js` refuses a blank before the sheet can send one, so reaching
/// this means a stale stored value, and "cannot reach" would point the user
/// at the network for it (issue #65).
#[test]
fn check_server_rejects_a_blank_address() {
    for blank in ["", "   ", "\t"] {
        let error = check_server(blank.to_string()).expect_err("a blank address is not a server");
        assert!(
            error.contains("the server address is blank")
                && error.contains("http://localhost:8000"),
            "{blank:?} must name the address as the mistake: {error}"
        );
        assert!(
            !error.contains("cannot reach"),
            "{blank:?} is not a reachability failure: {error}"
        );
    }
}

/// A URL no HTTP client can even parse is still just a failed probe: the
/// settings sheet gets a message, and the app does not come down with it. The
/// message has to say more than "unreachable", or a user typing a broken
/// address hunts for a network problem that is not there.
#[test]
fn check_server_rejects_a_syntactically_broken_url() {
    let error = check_server("not a url".to_string()).expect_err("that is not a URL");

    let reason = error
        .strip_prefix("cannot reach the server at not a url: ")
        .unwrap_or_else(|| panic!("the address the user typed is not named in: {error}"));
    // Wording owned by the HTTP client ("Bad URL: failed to parse URL: ..."),
    // pinned because it is the only thing distinguishing this from a server
    // that is simply down: that one's reason names an endpoint instead.
    assert!(
        reason.to_lowercase().contains("url"),
        "an unparseable address must not read like a refused connection: {reason}"
    );
}

// ------------------------------------------------- server-side housekeeping --

/// The client deletes the job once the archive is safely downloaded, so a
/// desktop user compressing all day does not fill the server's disk.
///
/// An empty job area proves nothing on its own: it is also what an ignored
/// server looks like. So the order is, first that a job really existed (the
/// server staged one while it was answering), then that the client asked for
/// that same job by id, then that nothing is left.
#[test]
fn a_finished_job_is_deleted_from_the_server_once_it_is_downloaded() {
    let server = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"leave nothing behind").unwrap();
    let output = dir.path().join("notes.txt.zip");

    compress_remotely(&server.url, &source, &output, "zip", 3).expect("the server compresses");
    assert!(output.is_file());

    let requests = server.requests();
    assert_eq!(
        server.peak_staged_jobs(),
        1,
        "the server never staged a job for this call: {requests:?}"
    );

    let downloaded = requests
        .iter()
        .find_map(|line| {
            line.strip_prefix("GET /jobs/")
                .and_then(|rest| rest.strip_suffix("/download"))
        })
        .unwrap_or_else(|| panic!("no archive was downloaded: {requests:?}"));
    let deleted = requests
        .iter()
        .find_map(|line| line.strip_prefix("DELETE /jobs/"))
        .unwrap_or_else(|| panic!("the job was never deleted: {requests:?}"));
    assert_eq!(
        deleted, downloaded,
        "the client deleted a different job than the one it downloaded: {requests:?}"
    );

    let leftovers = server.staged_jobs_now();
    assert!(leftovers.is_empty(), "jobs left behind: {leftovers:?}");
}
