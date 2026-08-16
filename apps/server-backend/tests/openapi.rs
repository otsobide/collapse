//! Tests for the OpenAPI document and the page that renders it.
//!
//! The point of most of these is that the document must not LIE. It is
//! hand-written, so nothing but a test stops it drifting from the server it
//! claims to describe.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::response::Response;
use axum::Router;
use http_body_util::BodyExt;
use tower::util::ServiceExt;

use collapse_server_backend::{build_app, DEFAULT_MAX_UPLOAD_MB};

fn app() -> (Router, tempfile::TempDir) {
    let storage = tempfile::TempDir::new().unwrap();
    let router = build_app(storage.path().to_path_buf(), DEFAULT_MAX_UPLOAD_MB);
    (router, storage)
}

async fn send(router: &Router, method: Method, uri: &str, body: &[u8]) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body.to_vec()))
        .unwrap();
    router.clone().oneshot(request).await.unwrap()
}

async fn bytes_of(response: Response) -> Vec<u8> {
    response.into_body().collect().await.unwrap().to_bytes().to_vec()
}

async fn json_of(response: Response) -> serde_json::Value {
    serde_json::from_slice(&bytes_of(response).await).unwrap()
}

async fn spec(router: &Router) -> serde_json::Value {
    json_of(send(router, Method::GET, "/openapi.json", b"").await).await
}

/// Post a file and return the accepted job.
async fn queue(router: &Router, query: &str) -> serde_json::Value {
    let response = send(router, Method::POST, &format!("/compress?{query}"), b"body").await;
    assert_eq!(response.status(), StatusCode::ACCEPTED, "query was {query:?}");
    json_of(response).await
}

// ---------------------------------------------------------------- serving --

#[tokio::test]
async fn the_document_is_served_at_the_crate_version() {
    let (router, _storage) = app();
    let spec = spec(&router).await;

    assert_eq!(spec["openapi"], "3.1.0");
    // Substituted from CARGO_PKG_VERSION, so it cannot drift from the crate.
    assert_eq!(spec["info"]["version"], env!("CARGO_PKG_VERSION"));
}

#[tokio::test]
async fn the_docs_page_is_served_as_html() {
    let (router, _storage) = app();
    let response = send(&router, Method::GET, "/docs", b"").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert!(String::from_utf8_lossy(&bytes_of(response).await).contains("<!doctype html>"));
}

/// The page is only useful on an offline host if it pulls nothing from the
/// network: the invariant a well-meaning edit is most likely to break.
#[test]
fn the_docs_page_has_no_external_dependencies() {
    let html = collapse_server_backend::openapi::DOCS_HTML;
    for marker in ["http://", "https://", "//cdn", "unpkg", "jsdelivr", "fonts.googleapis"] {
        assert!(
            !html.contains(marker),
            "the docs page references {marker}, so it would break offline"
        );
    }
}

// ------------------------------------------------- the document must not lie --

/// Every documented path must actually be routed. An unmatched axum route
/// answers 404 with an EMPTY body, while our own handlers always answer with a
/// JSON `detail`, which is what tells the two apart.
#[tokio::test]
async fn every_documented_path_is_routed() {
    let (router, _storage) = app();
    let spec = spec(&router).await;
    let methods = ["get", "post", "put", "patch", "delete"];

    for (path, item) in spec["paths"].as_object().unwrap() {
        for (method, _) in item.as_object().unwrap().iter().filter(|(m, _)| methods.contains(&m.as_str())) {
            let uri = path.replace("{job_id}", "does-not-exist");
            let verb = Method::from_bytes(method.to_uppercase().as_bytes()).unwrap();
            let response = send(&router, verb.clone(), &uri, b"").await;

            assert_ne!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is documented but not routed"
            );
            let status = response.status();
            let body = bytes_of(response).await;
            assert!(
                !(status == StatusCode::NOT_FOUND && body.is_empty()),
                "{method} {path} is documented but no route matched it"
            );
        }
    }
}

/// Every value the document advertises for an enum must really be accepted.
/// Adding one to the document without teaching the server about it would
/// otherwise ship a lie.
#[tokio::test]
async fn documented_algorithms_are_all_accepted() {
    let (router, _storage) = app();
    let spec = spec(&router).await;

    let algorithms = spec["components"]["schemas"]["Algorithm"]["enum"].as_array().unwrap().clone();
    assert!(!algorithms.is_empty(), "the document lists no algorithms");

    for algorithm in algorithms {
        let value = algorithm.as_str().unwrap();
        let job = queue(&router, &format!("name=a.txt&algorithm={value}")).await;
        assert_eq!(job["algorithm"], value);
    }
}

#[tokio::test]
async fn documented_envelopes_are_all_accepted() {
    let (router, _storage) = app();
    let spec = spec(&router).await;

    let envelopes = spec["components"]["schemas"]["Envelope"]["enum"].as_array().unwrap().clone();
    assert!(!envelopes.is_empty(), "the document lists no envelopes");

    for envelope in envelopes {
        let value = envelope.as_str().unwrap();
        // `tar` will fail the job later (the body is not a tar), but the
        // parameter itself has to be understood.
        let job = queue(&router, &format!("name=a&envelope={value}")).await;
        assert_eq!(job["envelope"], value);
    }
}

/// The documented defaults are what a caller relies on when omitting a
/// parameter, so they have to match what the server actually does.
#[tokio::test]
async fn documented_defaults_match_the_behaviour() {
    let (router, _storage) = app();
    let spec = spec(&router).await;

    let documented = |name: &str| {
        spec["paths"]["/compress"]["post"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not documented"))["schema"]["default"]
            .clone()
    };

    let job = queue(&router, "name=a.txt").await;
    assert_eq!(job["algorithm"], documented("algorithm"));
    assert_eq!(job["level"], documented("level"));
    assert_eq!(job["envelope"], documented("envelope"));
}
