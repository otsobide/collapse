use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

use collapse_core::Algorithm;

use crate::error::ApiError;
use crate::models::{Job, JobStatus};
use crate::validate::{header_safe, is_bare_file_name};
use crate::AppState;

/// Query parameters for `POST /compress`. An unparseable `level` (or a
/// missing `name`) is rejected by the extractor itself — never coerced to a
/// default, unlike the reference implementation.
#[derive(Debug, Deserialize)]
pub(crate) struct CompressParams {
    /// File name the content will carry inside the archive.
    name: String,
    /// Archive format; defaults to zip, like the CLI.
    algorithm: Option<String>,
    /// Compression level 1–5; defaults to 3, like the CLI.
    level: Option<u32>,
}

fn job_or_404(state: &AppState, job_id: &str) -> Result<Job, ApiError> {
    state
        .registry
        .get(job_id)
        .ok_or_else(|| ApiError::NotFound("Job not found.".into()))
}

// ---------------------------------------------------------------------------
// GET /health
// ---------------------------------------------------------------------------

pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

// ---------------------------------------------------------------------------
// GET /openapi.json and GET /docs
// ---------------------------------------------------------------------------

pub(crate) async fn openapi() -> Json<serde_json::Value> {
    Json(crate::openapi::spec())
}

pub(crate) async fn docs() -> Html<&'static str> {
    Html(crate::openapi::DOCS_HTML)
}

// ---------------------------------------------------------------------------
// POST /compress — accept the bytes, queue the job, answer 202
// ---------------------------------------------------------------------------

pub(crate) async fn compress_create(
    State(state): State<AppState>,
    Query(params): Query<CompressParams>,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let name = validated_name(&params.name)?;

    let algorithm = match params.algorithm.as_deref() {
        Some(text) => text.parse::<Algorithm>().map_err(ApiError::BadRequest)?,
        None => Algorithm::Zip,
    };

    let level = params.level.unwrap_or(3);
    if !(1..=5).contains(&level) {
        return Err(ApiError::BadRequest(format!(
            "Invalid compression level: {level}. Must be between 1 and 5."
        )));
    }

    let job_id = Uuid::new_v4().simple().to_string();

    // Persist the upload before registering the job, so a job never exists
    // without its input (blocking I/O offloaded to the thread pool).
    let storage = state.storage.clone();
    let save_id = job_id.clone();
    let save_name = name.clone();
    let data = body.to_vec();
    tokio::task::spawn_blocking(move || storage.save_input(&save_id, &save_name, &data))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))??;

    let job = Job::new(job_id.clone(), name, algorithm, level);
    state.registry.add(job.clone());
    state
        .queue_tx
        .send(job_id)
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((StatusCode::ACCEPTED, Json(job)))
}

// ---------------------------------------------------------------------------
// GET /jobs/{job_id} — current state of a job
// ---------------------------------------------------------------------------

pub(crate) async fn job_status(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<Job>, ApiError> {
    Ok(Json(job_or_404(&state, &job_id)?))
}

// ---------------------------------------------------------------------------
// GET /jobs/{job_id}/download — the archive, once completed
// ---------------------------------------------------------------------------

pub(crate) async fn download(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let job = job_or_404(&state, &job_id)?;

    match job.status {
        JobStatus::Queued | JobStatus::Compressing => {
            return Err(ApiError::Conflict("Compression is still in progress.".into()));
        }
        JobStatus::Failed => {
            return Err(ApiError::Conflict(
                job.error_message
                    .unwrap_or_else(|| "Compression failed.".into()),
            ));
        }
        JobStatus::Completed => {}
    }

    let path = state.storage.output_path(&job_id, job.algorithm);
    let archive = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::NotFound("Archive file not found.".into()))?;

    Ok((
        [
            (header::CONTENT_TYPE, job.algorithm.media_type().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", header_safe(&job.archive_name)),
            ),
        ],
        archive,
    ))
}

// ---------------------------------------------------------------------------
// DELETE /jobs/{job_id} — drop the job and its files after downloading
// ---------------------------------------------------------------------------

pub(crate) async fn delete_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let job = job_or_404(&state, &job_id)?;

    if matches!(job.status, JobStatus::Queued | JobStatus::Compressing) {
        return Err(ApiError::Conflict(
            "Cannot delete a job while compression is in progress.".into(),
        ));
    }

    let storage = state.storage.clone();
    let delete_id = job_id.clone();
    let deleted = tokio::task::spawn_blocking(move || storage.delete_job(&delete_id))
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    state.registry.remove(&job_id);

    Ok(Json(serde_json::json!({ "job_id": job_id, "deleted": deleted })))
}

/// Require `name` to be a bare file name: the arcname goes into the archive
/// verbatim and the staging path joins it onto the job directory, so
/// separators, `..` and empty names are rejected before anything touches disk.
fn validated_name(name: &str) -> Result<String, ApiError> {
    if is_bare_file_name(name) {
        Ok(name.to_string())
    } else {
        Err(ApiError::BadRequest(format!(
            "Invalid file name: {name:?}. Must be a bare file name."
        )))
    }
}
