//! Integration tests for the collapse-api router, driven in-process with
//! tower's `oneshot` — no sockets. Round-trips are verified by feeding the
//! response bytes back through the core extractors.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use tower::util::ServiceExt;

use collapse_api::{build_router, DEFAULT_MAX_UPLOAD_MB};

fn router() -> Router {
    build_router(DEFAULT_MAX_UPLOAD_MB)
}

async fn post_compress(router: Router, query: &str, body: &[u8]) -> Response {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/compress?{query}"))
        .body(Body::from(body.to_vec()))
        .unwrap();
    router.oneshot(request).await.unwrap()
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

async fn error_detail(response: Response) -> String {
    let bytes = body_bytes(response).await;
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["detail"].as_str().unwrap().to_string()
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
    let request = Request::builder()
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let response = router().oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = body_bytes(response).await;
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "ok");
}

// ---------------------------------------------------------------------------
// POST /compress — round-trips
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compress_zip_round_trips() {
    let content = b"zip me up through http";
    let response = post_compress(router(), "name=notes.txt&algorithm=zip&level=3", content).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/zip"
    );
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"notes.txt.zip\""
    );

    let extracted = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(extracted, vec![("notes.txt".to_string(), content.to_vec())]);
}

#[tokio::test]
async fn compress_7z_round_trips() {
    let content = b"seven zip over the wire";
    let response = post_compress(router(), "name=report.pdf&algorithm=7z&level=1", content).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/x-7z-compressed"
    );

    let extracted = extract_archive(&body_bytes(response).await, "7z");
    assert_eq!(extracted, vec![("report.pdf".to_string(), content.to_vec())]);
}

#[tokio::test]
async fn compress_tar_round_trips() {
    let content = b"tar carries this along";
    let response = post_compress(router(), "name=data.bin&algorithm=tar&level=5", content).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/x-tar"
    );

    let extracted = extract_archive(&body_bytes(response).await, "tar");
    assert_eq!(extracted, vec![("data.bin".to_string(), content.to_vec())]);
}

#[tokio::test]
async fn compress_defaults_to_zip() {
    let response = post_compress(router(), "name=plain.txt", b"defaults").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/zip"
    );
    let extracted = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(extracted[0].0, "plain.txt");
}

#[tokio::test]
async fn compress_accepts_empty_body() {
    let response = post_compress(router(), "name=empty.txt", b"").await;

    assert_eq!(response.status(), StatusCode::OK);
    let extracted = extract_archive(&body_bytes(response).await, "zip");
    assert_eq!(extracted, vec![("empty.txt".to_string(), Vec::new())]);
}

// ---------------------------------------------------------------------------
// POST /compress — rejections
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compress_rejects_unknown_algorithm() {
    let response = post_compress(router(), "name=a.txt&algorithm=rar", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(error_detail(response).await.contains("rar"));
}

#[tokio::test]
async fn compress_rejects_level_zero() {
    let response = post_compress(router(), "name=a.txt&level=0", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(error_detail(response).await.contains("level"));
}

#[tokio::test]
async fn compress_rejects_level_six() {
    let response = post_compress(router(), "name=a.txt&level=6", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn compress_rejects_unparseable_level() {
    // The reference implementation silently coerced this to a default level;
    // here it must be a hard 400 from the query extractor.
    let response = post_compress(router(), "name=a.txt&level=fast", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn compress_rejects_missing_name() {
    let response = post_compress(router(), "algorithm=zip", b"x").await;

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn compress_rejects_name_with_separators() {
    for name in ["../evil.txt", "a/b.txt", "a%5Cb.txt", "..", "."] {
        let response =
            post_compress(router(), &format!("name={name}&algorithm=zip"), b"x").await;
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
    let response = post_compress(
        build_router(1),
        "name=big.bin",
        &vec![0u8; 1024 * 1024 + 1],
    )
    .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn content_disposition_strips_header_breaking_characters() {
    // %22 is a double quote in the file name: legal on disk, but it must not
    // escape the quoted Content-Disposition value.
    let response = post_compress(router(), "name=we%22ird.txt", b"x").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CONTENT_DISPOSITION],
        "attachment; filename=\"weird.txt.zip\""
    );
}
