//! Tauri backend for the Collapse desktop app. Thin, safe commands over
//! `collapse-core`; the UI always supplies explicit paths chosen through the
//! native dialogs, so there is no default-path guessing here.

use std::path::{Path, PathBuf};

use collapse_core::{compress, compress_dir, extract, Algorithm};

/// Whether a path points at a directory (used by the UI to pick the icon and
/// the default archive name).
#[tauri::command]
fn is_directory(path: String) -> bool {
    Path::new(&path).is_dir()
}

/// Compress a file or a whole folder into `output`.
#[tauri::command]
fn compress_path(
    path: String,
    output: String,
    format: String,
    level: u32,
) -> Result<String, String> {
    let source = PathBuf::from(&path);
    if !source.exists() {
        return Err(format!("Not found: {path}"));
    }

    let algorithm: Algorithm = format.parse()?;
    let output_path = PathBuf::from(&output);

    // Never write the archive onto its own source (that would truncate the
    // source before it is read — irrecoverable data loss).
    if same_file(&source, &output_path) {
        return Err("The output is the same file as the source.".to_string());
    }

    if source.is_dir() {
        compress_dir(&source, &output_path, algorithm, level).map_err(|e| e.to_string())?;
    } else if source.is_file() {
        let arcname = source
            .file_name()
            .ok_or_else(|| "Invalid source path.".to_string())?
            .to_string_lossy()
            .into_owned();
        compress(&source, &output_path, &arcname, algorithm, level).map_err(|e| e.to_string())?;
    } else {
        return Err("Unsupported source (not a regular file or directory).".to_string());
    }

    Ok(output_path.to_string_lossy().into_owned())
}

/// Extract an archive into `output_dir`, returning the extracted file paths.
#[tauri::command]
fn extract_archive(archive: String, output_dir: String) -> Result<Vec<String>, String> {
    let archive_path = PathBuf::from(&archive);
    if !archive_path.exists() {
        return Err(format!("Not found: {archive}"));
    }
    let output = PathBuf::from(&output_dir);
    extract(&archive_path, &output).map_err(|e| e.to_string())
}

/// True when both paths resolve to the same existing file — by resolved path
/// (symlinks, `.`/`..`) and, on Unix, by inode/device so two hardlinks to the
/// same file are also caught.
fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) else {
        return false;
    };
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(ma), Ok(mb)) = (std::fs::metadata(&a), std::fs::metadata(&b)) {
            return ma.ino() == mb.ino() && ma.dev() == mb.dev();
        }
    }
    false
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            is_directory,
            compress_path,
            extract_archive
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
