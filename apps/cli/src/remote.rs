//! HTTP client for a remote collapse-api server: ship a file's bytes to
//! `POST /compress` and return the archive bytes from the response.

use std::io::Read;
use std::path::Path;

use collapse_core::Algorithm;

use crate::CliError;

pub(crate) fn compress_remote(
    server: &str,
    source: &Path,
    arcname: &str,
    algorithm: Algorithm,
    level: u32,
) -> Result<Vec<u8>, CliError> {
    let data = std::fs::read(source)?;
    let url = format!("{}/compress", server.trim_end_matches('/'));

    let response = ureq::post(&url)
        .query("name", arcname)
        .query("algorithm", algorithm.extension())
        .query("level", &level.to_string())
        .send_bytes(&data);

    match response {
        Ok(response) => {
            let mut archive = Vec::new();
            response.into_reader().read_to_end(&mut archive)?;
            Ok(archive)
        }
        Err(ureq::Error::Status(code, response)) => {
            Err(CliError::Remote(rejection_message(code, response)))
        }
        Err(err) => Err(CliError::Remote(format!(
            "cannot reach the server at {server}: {err}"
        ))),
    }
}

/// Render an HTTP error response, preferring the `detail` field of the
/// server's JSON error body, falling back to the raw body if it isn't JSON.
fn rejection_message(code: u16, response: ureq::Response) -> String {
    let body = response.into_string().unwrap_or_default();
    let detail = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("detail").and_then(|d| d.as_str()).map(String::from))
        .unwrap_or(body);
    if detail.is_empty() {
        format!("the server rejected the request (HTTP {code})")
    } else {
        format!("the server rejected the request (HTTP {code}): {detail}")
    }
}
