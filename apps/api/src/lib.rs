//! HTTP API for Collapse: a small, stateless server that exposes the engine's
//! compression over HTTP, so remote clients (the CLI's `--server` flag) can
//! send a file's bytes and get the archive back in the response. Compression
//! only for now — extraction stays local to the clients.
//!
//! Unlike the reference implementation there is no job queue, registry or
//! on-disk storage: each request is handled synchronously in a per-request
//! temporary directory that vanishes when the response is built.

mod error;
mod routes;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;

/// Default cap on uploaded bodies, in mebibytes.
pub const DEFAULT_MAX_UPLOAD_MB: usize = 500;

/// Build the API router.
///
/// Routes:
/// - `GET /health` — liveness probe, returns `{"status":"ok"}`.
/// - `POST /compress?name=<file name>[&algorithm=7z|tar|zip][&level=1-5]` —
///   body is the raw file content; the response body is the archive.
///   Errors are JSON `{"detail": "..."}` with a 4xx/5xx status.
///
/// `max_upload_mb` caps the accepted request body size (413 beyond it).
pub fn build_router(max_upload_mb: usize) -> Router {
    Router::new()
        .route("/health", get(routes::health))
        .route("/compress", post(routes::compress_file))
        .layer(DefaultBodyLimit::max(max_upload_mb * 1024 * 1024))
}
