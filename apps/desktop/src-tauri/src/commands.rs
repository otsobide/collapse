//! The commands the webview can invoke. Thin, safe wrappers over
//! `collapse-core` (and `collapse-remote` when a server is chosen); the UI
//! always supplies explicit paths chosen through the native dialogs, so there
//! is no default-path guessing here.
//!
//! They live in their own module, and are `pub`, for two reasons: a `pub`
//! `#[tauri::command]` at the crate root collides with its own generated
//! macro, and the integration tests in `tests/` drive these functions
//! directly (a command is an ordinary function, and this crate carries no
//! inline `mod tests`, like the rest of the workspace).

use std::path::PathBuf;

use collapse_core::{compress, compress_dir, extract, Algorithm};

use crate::paths::same_file;

/// Whether a path points at a directory (used by the UI to pick the icon and
/// the default archive name).
#[tauri::command]
pub fn is_directory(path: String) -> bool {
    std::path::Path::new(&path).is_dir()
}

/// Compress a file or a whole folder into `output`.
///
/// With `server` set, the work happens on a remote Collapse instance instead:
/// the bytes go out (a folder as a tar envelope), the archive comes back and
/// is written to the same `output` the local path would use. This command is
/// deliberately synchronous, so Tauri runs it on its blocking pool and the
/// window stays responsive while the server works.
#[tauri::command]
pub fn compress_path(
    path: String,
    output: String,
    format: String,
    level: u32,
    server: Option<String>,
) -> Result<String, String> {
    let source = PathBuf::from(&path);
    if !source.exists() {
        return Err(format!("Not found: {path}"));
    }
    if !source.is_dir() && !source.is_file() {
        return Err("Unsupported source (not a regular file or directory).".to_string());
    }

    let algorithm: Algorithm = format.parse()?;
    let output_path = PathBuf::from(&output);

    // Never write the archive onto its own source (that would truncate the
    // source before it is read, which is irrecoverable data loss).
    if same_file(&source, &output_path) {
        return Err("The output is the same file as the source.".to_string());
    }

    match server.as_deref().filter(|s| !s.is_empty()) {
        Some(server) => {
            let archive = collapse_remote::compress_path(server, &source, algorithm, level)
                .map_err(|e| e.to_string())?;
            std::fs::write(&output_path, archive).map_err(|e| e.to_string())?;
        }
        None if source.is_dir() => {
            compress_dir(&source, &output_path, algorithm, level).map_err(|e| e.to_string())?
        }
        None => {
            let arcname = source
                .file_name()
                .ok_or_else(|| "Invalid source path.".to_string())?
                .to_string_lossy()
                .into_owned();
            compress(&source, &output_path, &arcname, algorithm, level)
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(output_path.to_string_lossy().into_owned())
}

/// Check that a remote Collapse server is reachable, so a typo surfaces in
/// the settings panel rather than at the end of an upload.
#[tauri::command]
pub fn check_server(url: String) -> Result<(), String> {
    collapse_remote::check_health(&url).map_err(|e| e.to_string())
}

/// Extract an archive into `output_dir`, returning the extracted file paths.
#[tauri::command]
pub fn extract_archive(archive: String, output_dir: String) -> Result<Vec<String>, String> {
    let archive_path = PathBuf::from(&archive);
    if !archive_path.exists() {
        return Err(format!("Not found: {archive}"));
    }
    let output = PathBuf::from(&output_dir);
    extract(&archive_path, &output).map_err(|e| e.to_string())
}
