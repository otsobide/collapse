//! Integration tests for the collapse-api app, driven in-process with
//! tower's `oneshot` — no sockets. The full job flow is exercised: upload →
//! 202, poll the status, download the archive (verified by feeding the bytes
//! back through the core extractors), delete the job.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use tower::util::ServiceExt;

use collapse_api::{build_app, DEFAULT_MAX_UPLOAD_MB};

/// Build the app over its own staging dir; keep the TempDir alive with it.
fn app() -> (Router, tempfile::TempDir) {
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app(storage.path().to_path_buf(), DEFAULT_MAX_UPLOAD_MB);
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
    assert!(storage.path().join(&job_id).exists());

    let response = request(&router, Method::DELETE, &format!("/jobs/{job_id}"), b"").await;
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["deleted"], true);

    // Files gone, and the job no longer exists for any endpoint.
    assert!(!storage.path().join(&job_id).exists());
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
    let router = build_app(storage.path().to_path_buf(), 1);
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
