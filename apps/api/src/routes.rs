use std::fs;

use axum::body::Bytes;
use axum::extract::Query;
use axum::http::header;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;

use collapse_core::{compress, Algorithm};

use crate::error::ApiError;

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

// ---------------------------------------------------------------------------
// GET /health
// ---------------------------------------------------------------------------

pub(crate) async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

// ---------------------------------------------------------------------------
// POST /compress
// ---------------------------------------------------------------------------

pub(crate) async fn compress_file(
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

    // The engine works on paths, so stage the bytes in a per-request temp
    // directory; it is dropped (and deleted) inside the blocking task once
    // the archive bytes are read back.
    let dir = tempfile::tempdir()?;
    let input = dir.path().join(&name);
    let output = dir.path().join(format!("archive.{}", algorithm.extension()));
    let data = body.to_vec();
    let arcname = name.clone();

    let archive = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ApiError> {
        fs::write(&input, &data)?;
        compress(&input, &output, &arcname, algorithm, level)?;
        let bytes = fs::read(&output)?;
        drop(dir);
        Ok(bytes)
    })
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))??;

    let filename = format!("{name}.{}", algorithm.extension());
    Ok((
        [
            (header::CONTENT_TYPE, algorithm.media_type().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", header_safe(&filename)),
            ),
        ],
        archive,
    ))
}

/// Require `name` to be a bare file name: the arcname goes into the archive
/// verbatim and the staging path joins it onto the temp dir, so separators,
/// `..` and empty names are rejected before anything touches disk.
fn validated_name(name: &str) -> Result<String, ApiError> {
    let bare = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0');
    if bare {
        Ok(name.to_string())
    } else {
        Err(ApiError::BadRequest(format!(
            "Invalid file name: {name:?}. Must be a bare file name."
        )))
    }
}

/// Strip the characters that could break out of the quoted
/// Content-Disposition filename.
fn header_safe(filename: &str) -> String {
    filename
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\n' | '\r'))
        .collect()
}
