//! Integration tests for the collapse-server-backend app, driven in-process with
//! tower's `oneshot` — no sockets. The full job flow is exercised: upload →
//! 202, poll the status, download the archive (verified by feeding the bytes
//! back through the core extractors), delete the job.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use tower::util::ServiceExt;

use std::time::Duration;

use collapse_server_backend::storage::JOBS_DIR;
use collapse_server_backend::{build_app, build_app_with, DEFAULT_MAX_UPLOAD_MB};

/// Build the app over its own staging dir; keep the TempDir alive with it.
fn app() -> (Router, tempfile::TempDir) {
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app(storage.path().to_path_buf(), DEFAULT_MAX_UPLOAD_MB).expect("the app builds");
    (router, storage)
}

async fn request(router: &Router, method: Method, uri: &str, body: &[u8]) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body.to_vec()))
        .unwrap();
    router.clone().oneshot(request).await.unwrap()
}

async fn post_compress(router: &Router, query: &str, body: &[u8]) -> Response {
    request(router, Method::POST, &format!("/compress?{query}"), body).await
}

async fn body_bytes(response: Response) -> Vec<u8> {
    response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

async fn body_json(response: Response) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(response).await).unwrap()
}

async fn error_detail(response: Response) -> String {
    body_json(response).await["detail"].as_str().unwrap().to_string()
}

/// Poll the status endpoint until the job leaves the in-progress states,
/// returning its final JSON.
async fn wait_for_job(router: &Router, job_id: &str) -> serde_json::Value {
    for _ in 0..500 {
        let response = request(router, Method::GET, &format!("/jobs/{job_id}"), b"").await;
        assert_eq!(response.status(), StatusCode::OK);
        let job = body_json(response).await;
        match job["status"].as_str().unwrap() {
            "queued" | "compressing" => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
            _ => return job,
        }
    }
    panic!("job {job_id} never finished");
}

/// Upload, wait for completion, download: the archive bytes for a query.
async fn compress_and_download(router: &Router, query: &str, body: &[u8]) -> Response {
    let accepted = post_compress(router, query, body).await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let job_id = body_json(accepted).await["job_id"].as_str().unwrap().to_string();

    let done = wait_for_job(router, &job_id).await;
    assert_eq!(done["status"], "completed", "job failed: {done}");

    let response = request(router, Method::GET, &format!("/jobs/{job_id}/download"), b"").await;
    assert_eq!(response.status(), StatusCode::OK);
    response
}

/// Write the archive bytes to disk and extract them with the core engine,
/// returning the extracted (relative path, content) pairs.
fn extract_archive(bytes: &[u8], extension: &str) -> Vec<(String, Vec<u8>)> {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join(format!("response.{extension}"));
    std::fs::write(&archive, bytes).unwrap();

    let out = dir.path().join("out");
    let files = collapse_core::extract(&archive, &out).unwrap();
    files
        .into_iter()
        .map(|rel| {
            let content = std::fs::read(out.join(&rel)).unwrap();
            (rel, content)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// GET /health
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_ok() {
    let (router, _storage) = app();
    let response = request(&router, Method::GET, "/health", b"").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["status"], "ok");
}

// ---------------------------------------------------------------------------
// POST /compress — the 202 contract
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compress_answers_202_with_a_queued_job() {
    let (router, _storage) = app();
    let response = post_compress(&router, "name=notes.txt&algorithm=7z&level=4", b"x").await;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let job = body_json(response).await;
    assert!(!job["job_id"].as_str().unwrap().is_empty());
    assert_eq!(job["status"], "queued");
    assert_eq!(job["name"], "notes.txt");
    assert_eq!(job["archive_name"], "notes.txt.7z");
    assert_eq!(job["algorithm"], "7z");
    assert_eq!(job["level"], 4);
    assert_eq!(job["error_message"], serde_json::Value::Null);
}

// ---------------------------------------------------------------------------
// Full flow — round-trips
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compress_zip_round_trips() {
    let (router, _storage) = app();
    let content = b"zip me up through http";
    let response =
        compress_and_download(&router, "name=notes.txt&algorithm=zip&level=3", content).await;

    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/zip");
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"notes.txt.zip\""
    );

    let extracted = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(extracted, vec![("notes.txt".to_string(), content.to_vec())]);
}

#[tokio::test]
async fn compress_7z_round_trips() {
    let (router, _storage) = app();
    let content = b"seven zip over the wire";
    let response =
        compress_and_download(&router, "name=report.pdf&algorithm=7z&level=1", content).await;

    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/x-7z-compressed"
    );

    let extracted = extract_archive(&body_bytes(response).await, "7z");
    assert_eq!(extracted, vec![("report.pdf".to_string(), content.to_vec())]);
}

#[tokio::test]
async fn compress_tar_round_trips() {
    let (router, _storage) = app();
    let content = b"tar carries this along";
    let response =
        compress_and_download(&router, "name=data.bin&algorithm=tar&level=5", content).await;

    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/x-tar");

    let extracted = extract_archive(&body_bytes(response).await, "tar");
    assert_eq!(extracted, vec![("data.bin".to_string(), content.to_vec())]);
}

#[tokio::test]
async fn compress_defaults_to_zip() {
    let (router, _storage) = app();
    let response = compress_and_download(&router, "name=plain.txt", b"defaults").await;

    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/zip");
    let extracted = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(extracted[0].0, "plain.txt");
}

#[tokio::test]
async fn compress_accepts_empty_body() {
    let (router, _storage) = app();
    let response = compress_and_download(&router, "name=empty.txt", b"").await;

    let extracted = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(extracted, vec![("empty.txt".to_string(), Vec::new())]);
}

/// A file whose name matches the archive's own file name must still
/// round-trip: the upload is staged apart from the output path, so the
/// backends that create the output before reading the source cannot truncate
/// their own input.
#[tokio::test]
async fn compress_a_file_named_like_the_archive() {
    let (router, _storage) = app();
    let content = b"not clobbered by its own output";
    let response =
        compress_and_download(&router, "name=archive.zip&algorithm=zip", content).await;

    let extracted = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(
        extracted,
        vec![("archive.zip".to_string(), content.to_vec())]
    );
}

// ---------------------------------------------------------------------------
// POST /compress?envelope=tar — directory uploads
// ---------------------------------------------------------------------------

/// Pack a directory tree the way a client does, returning the tar bytes.
fn tar_envelope(build: impl Fn(&std::path::Path)) -> (tempfile::TempDir, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(&root).unwrap();
    build(&root);

    let tar = dir.path().join("upload.tar");
    collapse_core::compression::compress_tar_dir(&root, &tar).unwrap();
    let bytes = std::fs::read(&tar).unwrap();
    (dir, bytes)
}

#[tokio::test]
async fn a_tar_envelope_is_unwrapped_and_compressed_as_a_tree() {
    let (router, _storage) = app();
    let (_dir, tar) = tar_envelope(|root| {
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"first").unwrap();
        std::fs::write(root.join("sub/b.txt"), b"second").unwrap();
    });

    let response =
        compress_and_download(&router, "name=photos&algorithm=zip&envelope=tar", &tar).await;

    // The archive is named after the directory, not after the envelope.
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"photos.zip\""
    );

    let mut extracted = extract_archive(&body_bytes(response).await, "zip");
    extracted.sort();
    assert_eq!(
        extracted,
        vec![
            ("photos/a.txt".to_string(), b"first".to_vec()),
            ("photos/sub/b.txt".to_string(), b"second".to_vec()),
        ]
    );
}

/// Without the flag the same bytes are just a file to compress, which is why
/// the flag exists: a real .tar upload must stay compressible as itself.
#[tokio::test]
async fn without_the_flag_a_tar_is_compressed_as_a_file() {
    let (router, _storage) = app();
    let (_dir, tar) = tar_envelope(|root| {
        std::fs::write(root.join("a.txt"), b"first").unwrap();
    });

    let response = compress_and_download(&router, "name=photos.tar&algorithm=zip", &tar).await;

    let extracted = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(extracted, vec![("photos.tar".to_string(), tar)]);
}

#[tokio::test]
async fn compress_rejects_an_unknown_envelope() {
    let (router, _storage) = app();
    let response = post_compress(&router, "name=a.txt&envelope=zip", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(error_detail(response).await.contains("envelope"));
}

#[tokio::test]
async fn a_tar_envelope_that_is_not_a_tar_fails_the_job() {
    let (router, _storage) = app();
    let accepted = post_compress(&router, "name=photos&envelope=tar", b"not a tar at all").await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let job_id = body_json(accepted).await["job_id"].as_str().unwrap().to_string();

    let done = wait_for_job(&router, &job_id).await;
    assert_eq!(done["status"], "failed");
    assert!(!done["error_message"].as_str().unwrap().is_empty());

    // A failed job refuses to serve an archive rather than serving a broken one.
    let download =
        request(&router, Method::GET, &format!("/jobs/{job_id}/download"), b"").await;
    assert_eq!(download.status(), StatusCode::CONFLICT);
}

/// The name the job was created for has to be the tree that arrives.
#[tokio::test]
async fn a_tar_envelope_holding_another_directory_fails_the_job() {
    let (router, _storage) = app();
    let (_dir, tar) = tar_envelope(|root| {
        std::fs::write(root.join("a.txt"), b"first").unwrap();
    });

    let accepted = post_compress(&router, "name=somethingelse&envelope=tar", &tar).await;
    let job_id = body_json(accepted).await["job_id"].as_str().unwrap().to_string();

    let done = wait_for_job(&router, &job_id).await;
    assert_eq!(done["status"], "failed");
    assert!(done["error_message"].as_str().unwrap().contains("photos"));
}

#[tokio::test]
async fn download_can_be_repeated_until_deleted() {
    let (router, _storage) = app();
    let accepted = post_compress(&router, "name=a.txt", b"again and again").await;
    let job_id = body_json(accepted).await["job_id"].as_str().unwrap().to_string();
    wait_for_job(&router, &job_id).await;

    for _ in 0..2 {
        let response =
            request(&router, Method::GET, &format!("/jobs/{job_id}/download"), b"").await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}

// ---------------------------------------------------------------------------
// DELETE /jobs/{job_id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_removes_the_job_and_its_files() {
    let (router, storage) = app();
    let accepted = post_compress(&router, "name=a.txt", b"delete me after").await;
    let job_id = body_json(accepted).await["job_id"].as_str().unwrap().to_string();
    wait_for_job(&router, &job_id).await;
    assert!(storage.path().join(JOBS_DIR).join(&job_id).exists());

    let response = request(&router, Method::DELETE, &format!("/jobs/{job_id}"), b"").await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["deleted"], true);

    // Files gone, and the job no longer exists for any endpoint.
    assert!(!storage.path().join(JOBS_DIR).join(&job_id).exists());
    let status = request(&router, Method::GET, &format!("/jobs/{job_id}"), b"").await;
    assert_eq!(status.status(), StatusCode::NOT_FOUND);
    let download =
        request(&router, Method::GET, &format!("/jobs/{job_id}/download"), b"").await;
    assert_eq!(download.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Unknown jobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_job_is_404_everywhere() {
    let (router, _storage) = app();
    for (method, uri) in [
        (Method::GET, "/jobs/nope"),
        (Method::GET, "/jobs/nope/download"),
        (Method::DELETE, "/jobs/nope"),
    ] {
        let response = request(&router, method.clone(), uri, b"").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

// ---------------------------------------------------------------------------
// POST /compress — rejections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compress_rejects_unknown_algorithm() {
    let (router, _storage) = app();
    let response = post_compress(&router, "name=a.txt&algorithm=rar", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(error_detail(response).await.contains("rar"));
}

#[tokio::test]
async fn compress_rejects_level_zero() {
    let (router, _storage) = app();
    let response = post_compress(&router, "name=a.txt&level=0", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(error_detail(response).await.contains("level"));
}

#[tokio::test]
async fn compress_rejects_level_six() {
    let (router, _storage) = app();
    let response = post_compress(&router, "name=a.txt&level=6", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn compress_rejects_unparseable_level() {
    // The reference implementation silently coerced this to a default level;
    // here it must be a hard 400 from the query extractor.
    let (router, _storage) = app();
    let response = post_compress(&router, "name=a.txt&level=fast", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn compress_rejects_missing_name() {
    let (router, _storage) = app();
    let response = post_compress(&router, "algorithm=zip", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn compress_rejects_name_with_separators() {
    let (router, _storage) = app();
    for name in ["../evil.txt", "a/b.txt", "a%5Cb.txt", "..", "."] {
        let response =
            post_compress(&router, &format!("name={name}&algorithm=zip"), b"x").await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "name {name:?} should be rejected"
        );
    }
}

#[tokio::test]
async fn compress_rejects_body_over_the_limit() {
    // A 1 MiB cap and a body just past it.
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app(storage.path().to_path_buf(), 1).expect("the app builds");
    let response = post_compress(&router, "name=big.bin", &vec![0u8; 1024 * 1024 + 1]).await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn content_disposition_strips_header_breaking_characters() {
    // %22 is a double quote in the file name: legal on disk, but it must not
    // escape the quoted Content-Disposition value.
    let (router, _storage) = app();
    let response = compress_and_download(&router, "name=we%22ird.txt", b"x").await;

    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"weird.txt.zip\""
    );
}

#[tokio::test]
async fn the_reaper_collects_a_job_nobody_came_back_for() {
    // The background sweep, running for real: a one-second window, a job left
    // undownloaded, and the server cleaning up after it without being asked.
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app_with(
        storage.path().to_path_buf(),
        DEFAULT_MAX_UPLOAD_MB,
        Some(Duration::from_secs(1)),
    )
    .expect("the app builds");

    let job = body_json(post_compress(&router, "name=notes.txt", b"forget me").await).await;
    let job_id = job["job_id"].as_str().unwrap().to_string();
    assert_eq!(wait_for_job(&router, &job_id).await["status"], "completed");

    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let response = request(&router, Method::GET, &format!("/jobs/{job_id}"), b"").await;
        if response.status() == StatusCode::NOT_FOUND {
            let staged: Vec<_> = std::fs::read_dir(storage.path().join(JOBS_DIR))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.is_dir())
                .collect();
            assert!(staged.is_empty(), "its files went with it: {staged:?}");
            return;
        }
    }
    panic!("the reaper never collected the job");
}

#[tokio::test]
async fn the_reaper_can_be_turned_off() {
    // `--job-ttl-minutes 0` for someone who would rather keep every job until
    // a client deletes it.
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app_with(storage.path().to_path_buf(), DEFAULT_MAX_UPLOAD_MB, None)
        .expect("the app builds");

    let job = body_json(post_compress(&router, "name=notes.txt", b"keep me").await).await;
    let job_id = job["job_id"].as_str().unwrap().to_string();
    wait_for_job(&router, &job_id).await;

    tokio::time::sleep(Duration::from_millis(1500)).await;

    let response = request(&router, Method::GET, &format!("/jobs/{job_id}"), b"").await;
    assert_eq!(response.status(), StatusCode::OK, "nothing reaps it");
}

#[tokio::test]
async fn the_name_a_client_sends_reaches_the_archive_but_never_the_staging_paths() {
    // Where the name still belongs: inside the archive. Where it no longer
    // goes: any path this server builds. That split is what makes the layout
    // safe by construction instead of by the name validation holding.
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app(storage.path().to_path_buf(), DEFAULT_MAX_UPLOAD_MB)
        .expect("the app builds");

    let accepted = post_compress(&router, "name=my%20odd%20notes.txt", b"payload").await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let job_id = body_json(accepted).await["job_id"].as_str().unwrap().to_string();
    assert_eq!(wait_for_job(&router, &job_id).await["status"], "completed");

    // Every path under the staging directory, while the job is still there.
    let mut staged = Vec::new();
    let mut pending = vec![storage.path().join(JOBS_DIR)];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            staged.push(path.file_name().unwrap().to_string_lossy().into_owned());
            if path.is_dir() {
                pending.push(path);
            }
        }
    }
    assert!(
        !staged.iter().any(|name| name.contains("odd")),
        "the caller's name became a path: {staged:?}"
    );
    assert!(
        staged.iter().any(|name| name == "upload"),
        "the upload is staged under the server's own name: {staged:?}"
    );

    let response = request(&router, Method::GET, &format!("/jobs/{job_id}/download"), b"").await;
    let files = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(
        files,
        vec![("my odd notes.txt".to_string(), b"payload".to_vec())],
        "and the archive still carries the name the caller asked for"
    );
}

#[tokio::test]
async fn a_tar_envelope_is_staged_under_the_same_fixed_name() {
    // The envelope path has three files in play at once (the upload, the tree
    // it unpacks into, the archive it produces), which is exactly where a
    // name that came off the wire would have done the most damage. None of
    // them is named after anything the caller sent.
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app(storage.path().to_path_buf(), DEFAULT_MAX_UPLOAD_MB)
        .expect("the app builds");
    let (_dir, tar) = tar_envelope(|root| {
        std::fs::write(root.join("a.txt"), b"first").unwrap();
    });

    let accepted = post_compress(&router, "name=photos&algorithm=zip&envelope=tar", &tar).await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let job_id = body_json(accepted).await["job_id"].as_str().unwrap().to_string();
    assert_eq!(wait_for_job(&router, &job_id).await["status"], "completed");

    let job_dir = storage.path().join(JOBS_DIR).join(&job_id);
    assert!(
        job_dir.join("input").join("upload").is_file(),
        "the envelope is staged under the server's own name"
    );
    assert!(
        !job_dir.join("input").join("photos").exists()
            && !job_dir.join("input").join("photos.tar").exists(),
        "and never under the caller's"
    );

    // The unpacked tree does carry the caller's name, because that name is the
    // tar's own content and the server checks it is the single root it was
    // promised. That is a different thing from building a path out of it.
    assert!(job_dir.join("tree").join("photos").is_dir());
    assert!(job_dir.join("archive.zip").is_file());
}

#[tokio::test]
async fn a_job_this_build_cannot_read_answers_500_with_an_explanation() {
    // The status is honest: the server has a state it cannot interpret, which
    // is its problem, not the caller's. What changed is the message. It used
    // to be SQLite's own words ("Invalid column type Text at index 3"), which
    // told nobody anything.
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app(storage.path().to_path_buf(), DEFAULT_MAX_UPLOAD_MB)
        .expect("the app builds");

    // A row from a version that knows a format this one does not.
    let database = storage
        .path()
        .join(collapse_server_backend::storage::REGISTRY_DIR)
        .join(collapse_server_backend::registry::DATABASE_FILE);
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute(
            "INSERT INTO jobs
                 (job_id, name, archive_name, algorithm, level, envelope, status,
                  error_message, created_at, updated_at, server_version)
             VALUES ('future', 'notes.txt', 'notes.txt.zst', 'zstd', 3, 'none',
                     'completed', NULL, 0, 0, '0.9.0')",
            [],
        )
        .unwrap();

    let response = request(&router, Method::GET, "/jobs/future", b"").await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let detail = error_detail(response).await;
    assert!(detail.contains("0.9.0"), "names the build that wrote it: {detail}");
    assert!(detail.contains("zstd"), "names the value: {detail}");
    assert!(detail.contains("algorithm"), "names the field: {detail}");
    assert!(
        !detail.contains("column"),
        "and not the database's own words: {detail}"
    );
}
