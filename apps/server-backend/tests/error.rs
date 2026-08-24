//! Unit tests for the HTTP mapping of application errors.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;

use collapse_core::CompressionError;
use collapse_server_backend::error::{failure_message, ApiError};

async fn detail_of(response: Response) -> String {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    json["detail"].as_str().unwrap().to_string()
}

#[test]
fn each_variant_maps_to_its_status() {
    for (error, expected) in [
        (ApiError::BadRequest("bad".into()), StatusCode::BAD_REQUEST),
        (ApiError::NotFound("gone".into()), StatusCode::NOT_FOUND),
        (ApiError::Conflict("busy".into()), StatusCode::CONFLICT),
        (
            ApiError::Internal("boom".into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        assert_eq!(error.into_response().status(), expected);
    }
}

/// Clients (the CLI included) read the message out of `detail`; the key is
/// part of the API contract.
#[tokio::test]
async fn the_body_carries_the_message_under_detail() {
    let response = ApiError::NotFound("Job not found.".into()).into_response();
    assert_eq!(detail_of(response).await, "Job not found.");
}

#[tokio::test]
async fn error_responses_are_json() {
    let response = ApiError::Conflict("busy".into()).into_response();
    assert_eq!(
        response.headers()[axum::http::header::CONTENT_TYPE],
        "application/json"
    );
}

/// Staging failures reach the handlers as `io::Error` through `?`, and must
/// never surface as anything but a 500.
#[tokio::test]
async fn io_errors_become_internal_errors() {
    let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
    let response = ApiError::from(io).into_response();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(detail_of(response).await.contains("denied"));
}

// ------------------------------------------------ what a failed job says --

/// The engine names the file it read back, and for a job that is a path inside
/// the staging directory: `/var/lib/collapse/jobs/<uuid>/archive.zip`. Handing
/// that to a client tells them where the server keeps its files and nothing
/// they can act on, so the archive's own name replaces it.
#[test]
fn a_verification_failure_names_the_archive_not_the_servers_own_path() {
    let error = CompressionError::VerificationFailed {
        archive: std::path::PathBuf::from("/var/lib/collapse/jobs/abc123/archive.zip"),
        reason: "1 entry is missing: \"photos/b.jpg\"".to_string(),
    };

    let message = failure_message("photos.zip", &error);

    assert!(
        message.contains("photos.zip"),
        "names the archive the client asked for: {message}"
    );
    assert!(
        !message.contains("/var/lib/collapse"),
        "and not where the server keeps it: {message}"
    );
    assert!(
        !message.contains("abc123"),
        "nor the staging directory's name: {message}"
    );
}

/// It has to read as a sentence, because clients print `error_message`
/// verbatim: the CLI to a terminal, the web app into the page.
#[test]
fn a_verification_failure_reads_as_a_sentence_and_keeps_the_reason() {
    let error = CompressionError::VerificationFailed {
        archive: std::path::PathBuf::from("/tmp/x/archive.zip"),
        reason: "2 entries are missing: \"a.txt\", \"b.txt\"".to_string(),
    };

    assert_eq!(
        failure_message("photos.zip", &error),
        "photos.zip was compressed but did not check out, so it was discarded: \
         2 entries are missing: \"a.txt\", \"b.txt\""
    );
}

/// Every other engine error already says something the client can act on, and
/// rewriting those would throw away the only description of what went wrong.
#[test]
fn other_engine_errors_are_passed_through_word_for_word() {
    for error in [
        CompressionError::Failed("unexpected end of file".to_string()),
        CompressionError::InvalidLevel(9),
        CompressionError::Io(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        )),
    ] {
        assert_eq!(failure_message("photos.zip", &error), error.to_string());
    }
}
