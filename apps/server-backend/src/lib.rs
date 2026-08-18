//! HTTP API for Collapse: a small server that exposes the engine's
//! compression over HTTP, so remote clients (the CLI's `--server` flag) can
//! send a file's bytes and download the archive. Compression only for now —
//! extraction stays local to the clients.
//!
//! The flow is asynchronous, like the reference implementation's: uploading
//! answers `202 Accepted` with a job while a background worker compresses,
//! the job can be polled, the archive downloaded once completed, and the job
//! deleted afterwards. Jobs are staged on disk under a per-job directory and
//! recorded in a SQLite registry beside them, so they outlive the process;
//! `build_app` reconciles the two before serving.

// The building blocks are public because the integration tests are a separate
// crate and source files here carry no inline `mod tests`; only the wiring
// (handlers and the worker) stays private.
pub mod error;
pub mod logging;
pub mod maintenance;
pub mod models;
pub mod openapi;
pub mod registry;
pub mod storage;
pub mod validate;

mod queue;
mod routes;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use tokio::sync::mpsc;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tower_http::LatencyUnit;
use tracing::Level;

use error::StartupError;
use registry::Registry;
use storage::Storage;

/// Default cap on uploaded bodies, in mebibytes.
pub const DEFAULT_MAX_UPLOAD_MB: usize = 500;

/// How long a finished job survives without being downloaded again.
///
/// Generous on purpose: the clock only restarts on a download, so this is the
/// window a client has to come back for an archive it already asked for.
pub const DEFAULT_JOB_TTL_MINUTES: u64 = 60;

/// Shared application state, handed to every route handler.
#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) registry: Arc<Registry>,
    pub(crate) storage: Arc<Storage>,
    pub(crate) queue_tx: mpsc::UnboundedSender<String>,
}

/// Build the application: routes, state and the background compression
/// worker (spawned on the current tokio runtime).
///
/// Routes:
/// - `GET /health` — liveness probe, returns `{"status":"ok"}`.
/// - `GET /docs` — the interactive documentation page.
/// - `GET /openapi.json` — the OpenAPI 3.1 description of this server.
/// - `POST /compress?name=<file>[&algorithm=7z|tar|zip][&level=1-5]` — the
///   body is the raw file content; answers `202 Accepted` with the queued
///   job as JSON (`job_id`, `status`, `archive_name`, …) while a worker
///   compresses in the background.
/// - `GET /jobs/{job_id}` — the job's current state
///   (`queued` → `compressing` → `completed` | `failed`).
/// - `GET /jobs/{job_id}/download` — the archive bytes once `completed`
///   (409 while in progress or failed).
/// - `DELETE /jobs/{job_id}` — drop the job and its files once downloaded
///   (409 while in progress).
///
/// Errors are JSON `{"detail": "..."}` with a 4xx/5xx status. Job files are
/// staged under `storage_dir` (one directory per job); `max_upload_mb` caps
/// the accepted request body size (413 beyond it).
pub fn build_app(storage_dir: PathBuf, max_upload_mb: usize) -> Result<Router, StartupError> {
    build_app_with(
        storage_dir,
        max_upload_mb,
        Some(Duration::from_secs(DEFAULT_JOB_TTL_MINUTES * 60)),
    )
}

/// [`build_app`] with the reaper's window spelled out. `None` disables it, and
/// then only an explicit `DELETE` (or a restart, for files nothing claims)
/// ever frees a job's disk.
pub fn build_app_with(
    storage_dir: PathBuf,
    max_upload_mb: usize,
    job_ttl: Option<Duration>,
) -> Result<Router, StartupError> {
    // The registry's database lives in here, so the directory has to exist
    // before it is opened; `--storage-dir` may name one that does not yet.
    std::fs::create_dir_all(&storage_dir)?;

    let registry = Arc::new(Registry::open(&storage_dir)?);
    let storage = Arc::new(Storage::new(storage_dir));

    // Before serving anything: the rows and the staged files have to agree.
    // Nothing is compressing yet, which is what makes that judgement safe.
    let reconciled = maintenance::reconcile(&registry, &storage)?;
    if !reconciled.is_clean() {
        tracing::info!(
            interrupted = reconciled.interrupted,
            without_files = reconciled.without_files,
            orphaned = reconciled.orphaned,
            "reconciled the registry with the staging directory"
        );
    }

    let (queue_tx, queue_rx) = mpsc::unbounded_channel();
    queue::start_worker(registry.clone(), storage.clone(), queue_rx);

    if let Some(ttl) = job_ttl {
        start_reaper(registry.clone(), storage.clone(), ttl);
    }

    let state = AppState {
        registry,
        storage,
        queue_tx,
    };

    let api = Router::new()
        // Interactive documentation, the way FastAPI serves it — but built
        // into the binary, so it works with no network access.
        .route("/docs", get(routes::docs))
        .route("/openapi.json", get(routes::openapi))
        .route("/compress", post(routes::compress_create))
        .route("/jobs/{job_id}", get(routes::job_status).delete(routes::delete_job))
        .route("/jobs/{job_id}/download", get(routes::download))
        .layer(DefaultBodyLimit::max(max_upload_mb * 1024 * 1024))
        // Applied after the body limit, which makes it the outer layer, so an
        // upload rejected as too large is still logged as the 413 it is.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .latency_unit(LatencyUnit::Millis),
                ),
        );

    // /health stays outside the traced router on purpose: a container probes
    // it every ten seconds, and those lines would bury everything worth
    // reading. Its failures surface as the container's health status.
    Ok(Router::new()
        .route("/health", get(routes::health))
        .merge(api)
        .with_state(state))
}

/// Sweep finished jobs nobody came back for, forever.
///
/// The pass itself is blocking (SQLite, and a `remove_dir_all` that can take a
/// while on a large tree), so it runs off the runtime's threads. It sweeps at
/// a tenth of the window, which keeps a job from outliving its welcome by much
/// without waking an idle server every few seconds.
fn start_reaper(registry: Arc<Registry>, storage: Arc<Storage>, ttl: Duration) {
    let interval = (ttl / 10).clamp(Duration::from_millis(200), Duration::from_secs(300));

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;

            let deadline = registry::now_unix() - ttl.as_secs() as i64;
            let (registry, storage) = (registry.clone(), storage.clone());
            let swept = tokio::task::spawn_blocking(move || {
                maintenance::reap(&registry, &storage, deadline)
            })
            .await;

            match swept {
                Ok(Ok(0)) => {}
                Ok(Ok(jobs)) => tracing::info!(
                    jobs,
                    ttl_minutes = ttl.as_secs() / 60,
                    "reaped jobs nobody came back for"
                ),
                Ok(Err(e)) => tracing::error!(error = %e, "the reaper could not finish a pass"),
                Err(e) => tracing::error!(error = %e, "the reaper task died"),
            }
        }
    });
}
