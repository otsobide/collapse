//! Tests for the desktop's remote compression path: `compress_path` with a
//! `server`, plus the `check_server` probe behind the settings sheet.
//!
//! They run against a real `collapse-server-backend` served in-process on an
//! ephemeral port, the same harness `apps/cli/tests/remote.rs` and
//! `apps/remote/tests/client.rs` use. Every front end that speaks the job flow
//! proves it the same way, so a change in the exchange fails in all of them at
//! once instead of drifting in one.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use collapse_core::compression::compress_tar_dir;
use collapse_desktop::commands::{check_server, compress_path, extract_archive};

// ------------------------------------------------------------------ harness --

/// Serve the real backend on an OS-assigned port, returning its base URL and
/// its storage directory (so a test can observe server-side housekeeping).
/// The thread owns the staging TempDir, which is what keeps it alive for as
/// long as the server is answering.
fn start_server() -> (String, PathBuf) {
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

/// One server for every test that just needs "a server that works". Jobs are
/// independent (own id, own staging directory, deleted once downloaded), so
/// sharing it introduces no ordering between tests; the tests that inspect the
/// server's own directories start their own.
fn shared_server() -> &'static str {
    static SERVER: OnceLock<String> = OnceLock::new();
    SERVER.get_or_init(|| start_server().0)
}

/// A port from the unassigned range: nothing listens there in practice, so
/// pointing at it proves a failure happened without any HTTP exchange.
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
    )
}

/// The same call with no server, for the comparisons that pin "remote and
/// local produce the same archive".
fn compress_locally(source: &Path, output: &Path, format: &str, level: u32) -> String {
    compress_path(text(source), text(output), format.to_string(), level, None)
        .expect("the local compression succeeds")
}

/// Extract through the command under test and return every file with its
/// bytes, sorted: comparing content is what proves a round trip, where
/// comparing archives byte for byte would only pin the compressor's mood.
fn extracted(archive: &Path, into: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files =
        extract_archive(text(archive), text(into)).expect("the archive extracts cleanly");
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

// -------------------------------------------------------------- round trips --

/// The whole point of the feature: bytes go out, an archive comes back and is
/// written to the very path a local compression would have used. All three
/// formats, because the algorithm crosses the wire as a query parameter and a
/// mismatch there would only show up per format.
#[test]
fn a_file_compressed_on_the_server_round_trips_for_every_format() {
    let server = shared_server();
    for (format, extension) in [("zip", "zip"), ("7z", "7z"), ("tar", "tar")] {
        let dir = tempfile::TempDir::new().unwrap();
        let source = dir.path().join("notes.txt");
        let body = b"compressed far away".to_vec();
        std::fs::write(&source, &body).unwrap();

        // The path the UI's default naming produces, so this also pins that
        // remote mode writes where local mode would.
        let output = dir.path().join(format!("notes.txt.{extension}"));
        let returned = compress_remotely(server, &source, &output, format, 2)
            .unwrap_or_else(|e| panic!("{format}: {e}"));

        assert_eq!(returned, text(&output), "{format}");
        assert!(output.is_file(), "{format}: no archive was written");

        let entries = extracted(&output, &dir.path().join("out"));
        assert_eq!(entries, vec![("notes.txt".to_string(), body)], "{format}");
    }
}

/// A directory cannot travel over HTTP as it is, so the client packs it into a
/// tar envelope and the server unpacks it before compressing. The contract is
/// that none of that is visible in the result: same entries, same bytes, same
/// folder-name prefix as a local run. A real tree with a subdirectory and
/// several files is the only way to catch an envelope that loses depth.
#[test]
fn a_directory_compressed_on_the_server_matches_a_local_run() {
    let server = shared_server();
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("a.txt"), b"first").unwrap();
    std::fs::write(root.join("sub/b.txt"), b"second").unwrap();
    std::fs::write(root.join("sub/deeper/c.txt"), b"third").unwrap();

    let remote_archive = dir.path().join("remote.zip");
    compress_remotely(server, &root, &remote_archive, "zip", 3).expect("the server compresses");
    let local_archive = dir.path().join("local.zip");
    compress_locally(&root, &local_archive, "zip", 3);

    let remote = extracted(&remote_archive, &dir.path().join("r"));
    let local = extracted(&local_archive, &dir.path().join("l"));
    assert_eq!(remote, local, "the envelope must not change the result");
    assert_eq!(
        names(&remote),
        vec!["photos/a.txt", "photos/sub/b.txt", "photos/sub/deeper/c.txt"],
        "entries keep the folder's own name as their prefix"
    );
}

/// tar is both the envelope and a target format, so here the server untars an
/// upload only to tar it again. It is the one combination where a confused
/// envelope would still produce a plausible-looking archive.
#[test]
fn a_directory_compressed_to_tar_on_the_server_round_trips() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("docs");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"tar inside tar").unwrap();

    let archive = dir.path().join("docs.tar");
    compress_remotely(shared_server(), &root, &archive, "tar", 3).expect("the server compresses");

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
    let dir = tempfile::TempDir::new().unwrap();
    let tree = dir.path().join("staging/photos.tar");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join("inside.txt"), b"unpacked by mistake").unwrap();

    let tarball = dir.path().join("photos.tar");
    compress_tar_dir(&tree, &tarball).expect("the decoy tarball is built");
    let original = std::fs::read(&tarball).unwrap();

    let archive = dir.path().join("photos.tar.zip");
    compress_remotely(shared_server(), &tarball, &archive, "zip", 3)
        .expect("the server compresses");

    let entries = extracted(&archive, &dir.path().join("out"));
    assert_eq!(
        entries,
        vec![("photos.tar".to_string(), original)],
        "the tarball must come back as one stored file, not as its contents"
    );
}

// ---------------------------------------------------------------- failures --

/// A server that is not there has to say so, and must not leave a stub file
/// where the archive would have gone: the bytes are written only once the
/// whole archive is in hand.
#[test]
fn an_unreachable_server_is_reported_and_writes_no_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"never sent").unwrap();
    let output = dir.path().join("notes.txt.zip");

    let error = compress_remotely(UNREACHABLE, &source, &output, "zip", 3)
        .expect_err("nothing is listening there");

    assert!(
        error.contains("cannot reach the server"),
        "the message names the real problem: {error}"
    );
    assert!(!output.exists(), "no half-written archive is left behind");
    assert_eq!(std::fs::read(&source).unwrap(), b"never sent");
}

/// The level is not validated on this side of the wire, so the server's own
/// refusal is what the user sees. This pins the wording that reaches the
/// dialog, `detail` included: a bare "HTTP 400" would be useless there.
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

// ------------------------------------------------------- guards before I/O --

// Every test below points at UNREACHABLE: if any guard ran after the upload
// started, these would fail with a network error instead of the guard's own
// message. That ordering is the property, not the messages alone.

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

/// The no-data-loss guarantee, unchanged by the destination: writing the
/// archive onto its own source would truncate it, and remote mode reads the
/// source *after* that write would have happened, so this is the one guard
/// whose ordering is worth a byte-for-byte assertion.
#[test]
fn an_output_equal_to_the_source_is_refused_before_any_network_io() {
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("important.txt");
    std::fs::write(&source, b"IMPORTANT ORIGINAL CONTENT").unwrap();

    let error = compress_remotely(UNREACHABLE, &source, &source, "zip", 3)
        .expect_err("the output is the source");

    assert_eq!(error, "The output is the same file as the source.");
    assert_eq!(std::fs::read(&source).unwrap(), b"IMPORTANT ORIGINAL CONTENT");
}

// ----------------------------------------------------------- health probe --

/// What the settings sheet calls when a user adds a server: a typo has to
/// surface there, not at the end of an upload.
#[test]
fn check_server_accepts_a_real_server() {
    assert_eq!(check_server(shared_server().to_string()), Ok(()));
}

/// A trailing slash is what a user pastes from a browser, and it must not
/// produce a `//health` request path.
#[test]
fn check_server_accepts_a_real_server_with_a_trailing_slash() {
    assert_eq!(check_server(format!("{}/", shared_server())), Ok(()));
}

#[test]
fn check_server_reports_an_unreachable_address() {
    let error = check_server(UNREACHABLE.to_string()).expect_err("nothing is listening");
    assert!(error.contains("cannot reach the server"), "got {error}");
}

/// A URL no HTTP client can even parse is still just a failed probe: the
/// settings sheet gets a message, and the app does not come down with it.
#[test]
fn check_server_rejects_a_syntactically_broken_url() {
    let error = check_server("not a url".to_string()).expect_err("that is not a URL");
    assert!(
        error.contains("cannot reach the server"),
        "an unusable address is reported like any other unreachable one: {error}"
    );
}

// ------------------------------------------------- server-side housekeeping --

/// The client deletes the job once the archive is safely downloaded, so a
/// desktop user compressing all day does not fill the server's disk. This
/// server is private to the test: a shared one would see other tests' jobs.
///
/// It also pins the two-halved storage layout the sweep depends on: the
/// registry database lives outside the job area, so "every directory under
/// jobs/ is a job" stays true.
#[test]
fn a_finished_job_leaves_nothing_in_the_servers_job_area() {
    let (server, storage) = start_server();
    let dir = tempfile::TempDir::new().unwrap();
    let source = dir.path().join("notes.txt");
    std::fs::write(&source, b"leave nothing behind").unwrap();
    let output = dir.path().join("notes.txt.zip");

    compress_remotely(&server, &source, &output, "zip", 3).expect("the server compresses");
    assert!(output.is_file());

    let leftovers: Vec<PathBuf> = std::fs::read_dir(storage.join("jobs"))
        .expect("the job area exists")
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(leftovers.is_empty(), "jobs left behind: {leftovers:?}");
    assert!(
        storage.join("registry/jobs.db").is_file(),
        "the registry is a file beside the job area, never inside it"
    );
}
