//! End-to-end tests: the real binary, launched the way an operator launches
//! it, driven over a real socket by a real HTTP client.
//!
//! Everything else in this directory drives the router in-process, which is
//! fast and precise but never proves that the program starts, parses its
//! flags, opens its registry or survives being stopped. These do, which is why
//! they are also the only tests that can show a job outliving the process that
//! created it.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use collapse_server_backend::maintenance::INTERRUPTED;
use collapse_server_backend::models::{Envelope, Job, JobStatus};
use collapse_server_backend::registry::Registry;
use collapse_server_backend::storage::{Storage, JOBS_DIR, REGISTRY_DIR};
use collapse_core::Algorithm;
use tempfile::TempDir;

/// A running `collapse-server-backend` process.
struct Server {
    child: Child,
    base: String,
    log: Arc<Mutex<Vec<String>>>,
}

impl Server {
    /// Start the binary on a port the operating system picks, and wait until
    /// it answers. `--port 0` is what keeps concurrent tests off each other's
    /// ports; the bound address comes back from the startup log line.
    fn start(storage: &Path, extra: &[&str]) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_collapse-server-backend"))
            .args(["--host", "127.0.0.1", "--port", "0"])
            .args(["--storage-dir", storage.to_str().unwrap()])
            .args(extra)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the server binary runs");

        let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));

        // Collecting starts before the port is known: the reconciliation
        // reports itself *before* the server announces where it is listening,
        // so a reader that skipped those lines would miss the ones a startup
        // test is looking for.
        let log = Arc::new(Mutex::new(Vec::new()));
        let addr = read_bound_address(&mut stdout, &log);

        // Drain the rest in the background: the process writes a line per
        // request, and a full pipe would block it mid-test.
        let collected = log.clone();
        std::thread::spawn(move || {
            for line in stdout.lines().map_while(Result::ok) {
                collected.lock().unwrap().push(line);
            }
        });

        let server = Server {
            child,
            base: format!("http://{addr}"),
            log,
        };
        server.await_health();
        server
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn await_health(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if get(&self.url("/health")).0 == 200 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the server never answered /health");
    }

    /// Poll a job until it settles, the way every client does.
    fn await_status(&self, job_id: &str) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let (status, body) = get(&self.url(&format!("/jobs/{job_id}")));
            assert_eq!(status, 200, "polling a job that exists: {body}");
            let job: serde_json::Value = serde_json::from_str(&body).unwrap();
            if job["status"] != "queued" && job["status"] != "compressing" {
                return job;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the job never settled");
    }

    fn logged(&self, needle: &str) -> bool {
        self.log
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(needle))
    }

    /// Stop it the way a `docker stop` would, and wait for it to be gone, so a
    /// second server can take over the same staging directory.
    fn stop(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Read the startup line and pull the address out of `addr=127.0.0.1:PORT`.
fn read_bound_address(
    stdout: &mut BufReader<std::process::ChildStdout>,
    log: &Arc<Mutex<Vec<String>>>,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        if stdout.read_line(&mut line).expect("the server logs") == 0 {
            panic!("the server exited before it announced a port");
        }
        log.lock().unwrap().push(line.trim_end().to_string());
        if let Some(rest) = line.split("addr=").nth(1) {
            return rest.split_whitespace().next().unwrap().to_string();
        }
    }
    panic!("the server never announced a port");
}

// ------------------------------------------------------------ HTTP helpers --

fn finish(result: Result<ureq::Response, ureq::Error>) -> (u16, String) {
    match result {
        Ok(response) => (response.status(), response.into_string().unwrap()),
        Err(ureq::Error::Status(code, response)) => (code, response.into_string().unwrap()),
        Err(e) => panic!("the request never completed: {e}"),
    }
}

fn get(url: &str) -> (u16, String) {
    finish(ureq::get(url).call())
}

fn delete(url: &str) -> (u16, String) {
    finish(ureq::delete(url).call())
}

fn post(url: &str, body: &[u8]) -> (u16, String) {
    finish(ureq::post(url).send_bytes(body))
}

/// Downloads are bytes, not text: an archive is not valid UTF-8.
fn download(url: &str) -> (u16, Vec<u8>) {
    match ureq::get(url).call() {
        Ok(response) => {
            let mut bytes = Vec::new();
            response.into_reader().read_to_end(&mut bytes).unwrap();
            (200, bytes)
        }
        Err(ureq::Error::Status(code, _)) => (code, Vec::new()),
        Err(e) => panic!("the download never completed: {e}"),
    }
}

fn job_id(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body).unwrap()["job_id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The job directories in a staging area, which is what has to be empty once
/// clients have cleaned up after themselves (the registry's own database lives
/// there too, as files).
fn staged_dirs(storage: &Path) -> Vec<String> {
    let jobs = storage.join(JOBS_DIR);
    if !jobs.is_dir() {
        return Vec::new();
    }
    std::fs::read_dir(jobs)
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

// --------------------------------------------------------- the plain routes --

#[test]
fn health_docs_and_openapi_are_served_by_the_binary() {
    let storage = TempDir::new().unwrap();
    let server = Server::start(storage.path(), &[]);

    let (status, body) = get(&server.url("/health"));
    assert_eq!(status, 200);
    assert_eq!(body.trim(), r#"{"status":"ok"}"#);

    let (status, body) = get(&server.url("/openapi.json"));
    assert_eq!(status, 200);
    let spec: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["info"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(
        !body.contains("__VERSION__"),
        "the version placeholder is substituted at runtime"
    );

    let (status, body) = get(&server.url("/docs"));
    assert_eq!(status, 200);
    assert!(body.contains("<html"), "the docs page is HTML");

    // axum 0.8 does not normalize trailing slashes, and a served page that
    // 404s on a plausible URL is worth knowing about.
    assert_eq!(get(&server.url("/docs/")).0, 404);
}

// --------------------------------------------------------------- the flow --

#[test]
fn a_file_round_trips_through_the_whole_flow() {
    let storage = TempDir::new().unwrap();
    let server = Server::start(storage.path(), &[]);
    let content = b"end to end, over a real socket\n".repeat(100);

    let (status, body) = post(
        &server.url("/compress?name=notes.txt&algorithm=zip&level=5"),
        &content,
    );
    assert_eq!(status, 202, "an upload is accepted, not awaited: {body}");
    let queued: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(queued["status"], "queued");
    assert_eq!(queued["archive_name"], "notes.txt.zip");
    let id = job_id(&body);

    let job = server.await_status(&id);
    assert_eq!(job["status"], "completed");
    assert!(job["error_message"].is_null());

    let (status, archive) = download(&server.url(&format!("/jobs/{id}/download")));
    assert_eq!(status, 200);

    // The bytes are a real archive: extract them with the same engine any
    // other client would use.
    let out = TempDir::new().unwrap();
    let archive_path = out.path().join("downloaded.zip");
    std::fs::write(&archive_path, &archive).unwrap();
    let extracted = collapse_core::extract(&archive_path, out.path()).unwrap();
    assert_eq!(extracted, vec!["notes.txt"]);
    assert_eq!(std::fs::read(out.path().join("notes.txt")).unwrap(), content);

    let (status, body) = delete(&server.url(&format!("/jobs/{id}")));
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["deleted"],
        true
    );

    assert_eq!(get(&server.url(&format!("/jobs/{id}"))).0, 404);
    assert!(
        staged_dirs(storage.path()).is_empty(),
        "the job's files go with it"
    );
}

#[test]
fn a_directory_round_trips_as_a_tar_envelope() {
    let storage = TempDir::new().unwrap();
    let server = Server::start(storage.path(), &[]);

    // A directory a client would have packed before uploading.
    let source = TempDir::new().unwrap();
    let photos = source.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    std::fs::write(photos.join("one.txt"), b"first").unwrap();
    std::fs::create_dir(photos.join("nested")).unwrap();
    std::fs::write(photos.join("nested/two.txt"), b"second").unwrap();

    let mut builder = tar::Builder::new(Vec::new());
    builder.append_dir_all("photos", &photos).unwrap();
    let envelope = builder.into_inner().unwrap();

    let (status, body) = post(
        &server.url("/compress?name=photos&envelope=tar&algorithm=7z&level=3"),
        &envelope,
    );
    assert_eq!(status, 202, "{body}");
    let id = job_id(&body);

    let job = server.await_status(&id);
    assert_eq!(job["status"], "completed", "{job}");
    assert_eq!(job["archive_name"], "photos.7z");
    assert_eq!(job["envelope"], "tar");

    let (_, archive) = download(&server.url(&format!("/jobs/{id}/download")));
    let out = TempDir::new().unwrap();
    let archive_path = out.path().join("photos.7z");
    std::fs::write(&archive_path, &archive).unwrap();

    let mut extracted = collapse_core::extract(&archive_path, out.path()).unwrap();
    extracted.sort();
    assert_eq!(
        extracted,
        vec![
            "photos/nested/two.txt".to_string(),
            "photos/one.txt".to_string()
        ]
    );
}

// ------------------------------------------------------------- rejections --

#[test]
fn bad_requests_are_rejected_before_anything_is_staged() {
    let storage = TempDir::new().unwrap();
    let server = Server::start(storage.path(), &[]);

    let cases = [
        ("/compress?name=../escape.txt", "a path, not a bare name"),
        ("/compress?name=sub/dir.txt", "a separator in the name"),
        ("/compress?name=notes.txt&level=9", "a level out of range"),
        ("/compress?name=notes.txt&algorithm=rar", "an unknown format"),
        ("/compress?name=notes.txt&envelope=zip", "an unknown envelope"),
    ];
    for (query, what) in cases {
        let (status, body) = post(&server.url(query), b"payload");
        assert_eq!(status, 400, "{what} is a 400, got {status}: {body}");
        assert!(
            body.contains("detail"),
            "errors carry a detail: {body} ({what})"
        );
    }

    // A missing name never reaches the handler: the extractor rejects it.
    assert_eq!(post(&server.url("/compress"), b"payload").0, 400);

    assert_eq!(get(&server.url("/jobs/does-not-exist")).0, 404);
    assert_eq!(delete(&server.url("/jobs/does-not-exist")).0, 404);
    assert_eq!(
        get(&server.url("/jobs/does-not-exist/download")).0,
        404,
        "downloading a job that never existed"
    );

    assert!(
        staged_dirs(storage.path()).is_empty(),
        "a rejected request stages nothing"
    );
}

#[test]
fn a_failed_job_reports_why_and_refuses_its_download() {
    let storage = TempDir::new().unwrap();
    let server = Server::start(storage.path(), &[]);

    // A tar envelope that is not a tar: the failure happens in the worker,
    // after the 202, which is exactly the path a client has to handle.
    let (status, body) = post(
        &server.url("/compress?name=photos&envelope=tar"),
        b"definitely not a tar archive",
    );
    assert_eq!(status, 202);
    let id = job_id(&body);

    let job = server.await_status(&id);
    assert_eq!(job["status"], "failed");
    assert!(
        job["error_message"].as_str().unwrap().len() > 5,
        "a failure says why: {job}"
    );

    assert_eq!(
        get(&server.url(&format!("/jobs/{id}/download"))).0,
        409,
        "there is no archive to download"
    );

    // A failed job is still the client's to clean up.
    assert_eq!(delete(&server.url(&format!("/jobs/{id}"))).0, 200);
    assert!(staged_dirs(storage.path()).is_empty());
}

#[test]
fn an_upload_over_the_cap_is_refused() {
    let storage = TempDir::new().unwrap();
    let server = Server::start(storage.path(), &["--max-upload-mb", "1"]);

    // The server answers 413 as soon as the body passes the cap and stops
    // reading, so whether the *client* gets to read that answer depends on how
    // much it was still sending: curl asks with `Expect: 100-continue` and
    // sees a clean 413, while a client that writes the whole body first (ureq,
    // which is what this test and collapse-remote use) can have its connection
    // reset mid-upload instead. The refusal is the server's behaviour, so that
    // is what is asserted here, from the server's own log.
    let _ = std::panic::catch_unwind(|| {
        post(
            &server.url("/compress?name=big.bin"),
            &vec![0u8; 8 * 1024 * 1024],
        )
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !server.logged("status=413") {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        server.logged("status=413"),
        "the body limit is enforced by the binary: {:?}",
        server.log.lock().unwrap()
    );
    assert!(
        staged_dirs(storage.path()).is_empty(),
        "a refused upload stages nothing"
    );

    // The cap is the only thing rejected: a small upload still works, and the
    // server is still healthy after refusing one.
    let (status, body) = post(&server.url("/compress?name=small.txt"), b"tiny");
    assert_eq!(status, 202);
    assert_eq!(server.await_status(&job_id(&body))["status"], "completed");
}

// ------------------------------------------------------- across a restart --

#[test]
fn a_finished_job_outlives_the_process_that_made_it() {
    // The whole point of the persistent registry, and the one thing no
    // in-process test can show.
    let storage = TempDir::new().unwrap();
    let content = b"survive the restart".repeat(50);

    let first = Server::start(storage.path(), &[]);
    let (_, body) = post(&first.url("/compress?name=notes.txt"), &content);
    let id = job_id(&body);
    assert_eq!(first.await_status(&id)["status"], "completed");
    let (_, before) = download(&first.url(&format!("/jobs/{id}/download")));
    first.stop(); // the client never deleted the job

    let second = Server::start(storage.path(), &[]);

    let (status, body) = get(&second.url(&format!("/jobs/{id}")));
    assert_eq!(status, 200, "the job is still known after a restart");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&body).unwrap()["status"],
        "completed"
    );

    let (status, after) = download(&second.url(&format!("/jobs/{id}/download")));
    assert_eq!(status, 200);
    assert_eq!(after, before, "the same archive, byte for byte");

    // And it can finally be cleaned up, which is what used to be impossible.
    assert_eq!(delete(&second.url(&format!("/jobs/{id}"))).0, 200);
    assert!(staged_dirs(storage.path()).is_empty());
}

#[test]
fn files_no_job_claims_are_swept_at_startup() {
    let storage = TempDir::new().unwrap();

    // Files left by an older server, or by a crash between staging an upload
    // and recording it.
    let stray = storage.path().join(JOBS_DIR).join("stray-job-from-before");
    std::fs::create_dir_all(stray.join("input")).unwrap();
    std::fs::write(stray.join("input/notes.txt"), b"stranded").unwrap();

    let server = Server::start(storage.path(), &[]);

    assert!(
        staged_dirs(storage.path()).is_empty(),
        "the orphan is gone before the server serves anything"
    );
    assert!(
        server.logged("orphaned=1"),
        "and the server said so: {:?}",
        server.log.lock().unwrap()
    );
}

#[test]
fn a_job_caught_mid_compression_comes_back_failed() {
    let storage = TempDir::new().unwrap();

    // What a stop during compression leaves behind: a row that claims to be
    // running, with its upload staged. Written through the crate's own API so
    // the test does not hand-roll the schema.
    {
        let registry_dir = storage.path().join(REGISTRY_DIR);
        let jobs_dir = storage.path().join(JOBS_DIR);
        std::fs::create_dir_all(&registry_dir).unwrap();
        std::fs::create_dir_all(&jobs_dir).unwrap();
        let registry = Registry::open(&registry_dir).unwrap();
        let storage_area = Storage::new(jobs_dir);
        let job = Job::new(
            "interrupted-job".to_string(),
            "notes.txt".to_string(),
            Algorithm::Zip,
            3,
            Envelope::None,
        );
        registry.add(&job).unwrap();
        registry
            .update_status("interrupted-job", JobStatus::Compressing, None)
            .unwrap();
        storage_area
            .save_input("interrupted-job", b"half done")
            .unwrap();
    }

    let server = Server::start(storage.path(), &[]);

    let (status, body) = get(&server.url("/jobs/interrupted-job"));
    assert_eq!(status, 200, "the job is not lost, it is resolved");
    let job: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        job["status"], "failed",
        "a client polling it is told, not left waiting"
    );
    assert_eq!(job["error_message"], INTERRUPTED);
    assert!(server.logged("interrupted=1"));
}

/// A staging directory that does not exist yet is created, not a reason to
/// refuse to start: `--storage-dir` often names a fresh volume mount.
#[test]
fn a_missing_storage_directory_is_created() {
    let parent = TempDir::new().unwrap();
    let storage: PathBuf = parent.path().join("not/created/yet");

    let server = Server::start(&storage, &[]);

    assert!(storage.is_dir());
    let (_, body) = post(&server.url("/compress?name=notes.txt"), b"hello");
    assert_eq!(server.await_status(&job_id(&body))["status"], "completed");
}

#[test]
fn the_reaping_window_is_configurable_and_reported() {
    let storage = TempDir::new().unwrap();

    // The default is announced at startup, so an operator can see what the
    // server will collect and when without reading the source.
    let server = Server::start(storage.path(), &[]);
    assert!(
        server.logged("job_ttl_minutes=60"),
        "the window is in the startup line: {:?}",
        server.log.lock().unwrap()
    );

    // A finished job is not collected the moment it finishes: the window is an
    // hour, and a client has that long to come back for its archive.
    let (_, body) = post(&server.url("/compress?name=notes.txt"), b"still wanted");
    let id = job_id(&body);
    assert_eq!(server.await_status(&id)["status"], "completed");
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(get(&server.url(&format!("/jobs/{id}"))).0, 200);
    server.stop();

    // And it can be turned off for someone who would rather keep every job
    // until a client deletes it.
    let never = Server::start(storage.path(), &["--job-ttl-minutes", "0"]);
    assert!(never.logged("job_ttl_minutes=0"));
    assert_eq!(
        get(&never.url(&format!("/jobs/{id}"))).0,
        200,
        "the job from the previous run is still there, untouched"
    );
}

// --------------------------------------------------------------- stopping --

#[cfg(unix)]
impl Server {
    /// Ask it to stop the way `docker stop` and systemd do.
    fn terminate(&self) {
        Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .expect("kill runs");
    }

    /// Wait for the process to be gone, up to a deadline.
    fn await_exit(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }
}

/// An archive big enough that its transfer cannot sit in a socket buffer, so a
/// slow reader really does hold the request open.
#[cfg(unix)]
fn staged_archive(server: &Server, megabytes: usize) -> String {
    // Random bytes stored in a tar: incompressible and uncompressed, so the
    // archive is the size of what went in.
    let mut content = Vec::with_capacity(megabytes * 1024 * 1024);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    while content.len() < megabytes * 1024 * 1024 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        content.extend_from_slice(&seed.to_le_bytes());
    }

    let (status, body) = post(
        &server.url("/compress?name=big.bin&algorithm=tar"),
        &content,
    );
    assert_eq!(status, 202, "{body}");
    let id = job_id(&body);
    assert_eq!(server.await_status(&id)["status"], "completed");
    id
}

#[test]
#[cfg(unix)]
fn a_download_in_flight_survives_a_stop() {
    // The reason graceful shutdown is worth having: before it, this same
    // sequence handed the client a truncated archive.
    let storage = TempDir::new().unwrap();
    let mut server = Server::start(storage.path(), &[]);
    let id = staged_archive(&server, 24);

    let response = ureq::get(&server.url(&format!("/jobs/{id}/download")))
        .call()
        .expect("the download starts");
    let expected: usize = response
        .header("content-length")
        .expect("the archive's length is known")
        .parse()
        .unwrap();
    let mut body = response.into_reader();

    // Read a little, so the request is unmistakably in flight, then stop the
    // server. The rest of the archive is still on the far side of the socket.
    let mut received = vec![0u8; 8 * 1024];
    let first = body.read(&mut received).unwrap();
    assert!(first > 0);
    server.terminate();
    std::thread::sleep(Duration::from_millis(200));

    let mut rest = Vec::new();
    body.read_to_end(&mut rest)
        .expect("the rest of the archive arrives");

    assert_eq!(
        first + rest.len(),
        expected,
        "the whole archive arrived, not a truncated one"
    );
    assert!(
        server.await_exit(Duration::from_secs(10)),
        "and the server exited once it was done"
    );
}

#[test]
#[cfg(unix)]
fn a_stop_refuses_new_work_while_it_drains() {
    let storage = TempDir::new().unwrap();
    let mut server = Server::start(storage.path(), &[]);
    let id = staged_archive(&server, 24);

    let response = ureq::get(&server.url(&format!("/jobs/{id}/download")))
        .call()
        .expect("the download starts");
    let mut body = response.into_reader();
    let mut opening = vec![0u8; 8 * 1024];
    body.read(&mut opening).unwrap();

    server.terminate();
    std::thread::sleep(Duration::from_millis(300));

    // Draining is not the same as still open for business.
    assert!(
        ureq::get(&server.url("/health")).call().is_err(),
        "a stopping server takes no new connections"
    );

    let mut rest = Vec::new();
    body.read_to_end(&mut rest).expect("the download still finishes");
    assert!(server.await_exit(Duration::from_secs(10)));
}

#[test]
#[cfg(unix)]
fn a_clean_stop_takes_the_temporary_staging_area_with_it() {
    // Without --storage-dir the staging area is a temporary directory whose
    // guard only runs on a clean exit. A process killed mid-response leaves it
    // behind, one directory per restart.
    let mut child = Command::new(env!("CARGO_BIN_EXE_collapse-server-backend"))
        .args(["--host", "127.0.0.1", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("the server binary runs");

    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    read_bound_address(&mut stdout, &log);

    let line = log.lock().unwrap().last().unwrap().clone();
    let staging = line
        .split("storage_dir=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("the staging directory is in the startup line")
        .to_string();
    assert!(Path::new(&staging).is_dir());

    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    child.wait().unwrap();

    assert!(
        !Path::new(&staging).exists(),
        "the temporary staging area goes with the process"
    );
}

#[test]
#[cfg(unix)]
fn a_client_that_stops_reading_cannot_hold_the_stop_open() {
    // The other half of a graceful stop: it finishes what is in flight, but a
    // client that walks away mid-download must not keep the process alive for
    // as long as it likes. That is what --shutdown-grace-seconds bounds.
    let storage = TempDir::new().unwrap();
    let mut server = Server::start(storage.path(), &["--shutdown-grace-seconds", "1"]);
    let id = staged_archive(&server, 24);

    let response = ureq::get(&server.url(&format!("/jobs/{id}/download")))
        .call()
        .expect("the download starts");
    let mut body = response.into_reader();
    let mut opening = vec![0u8; 8 * 1024];
    body.read(&mut opening).unwrap();

    // From here the archive is never read again, so the request stays open.
    server.terminate();

    assert!(
        server.await_exit(Duration::from_secs(10)),
        "the deadline ends the wait even with a request still open"
    );
    drop(body); // held until now, so the connection really was in flight
}
