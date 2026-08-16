//! Unit tests for the HTTP mapping of application errors.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;

use collapse_api::error::ApiError;

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
