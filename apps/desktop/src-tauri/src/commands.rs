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
//!
//! **Anything that can take longer than an instant carries
//! `#[tauri::command(async)]`.** A bare `#[tauri::command]` on a synchronous
//! function compiles to what tauri-macros calls the `sync` path, which runs
//! the body inline on the thread handling the IPC message: the window stops
//! repainting for the duration, and the system eventually offers to force
//! quit. The attribute argument moves it to `sync_threadpool`, which hands the
//! call to `respond_async_serialized` and so to `async_runtime::spawn`.
//!
//! Worth knowing what that is and is not: it is the async runtime, a
//! multi-thread tokio with one worker per core, not the blocking pool, so the
//! body occupies a worker for its whole duration. That is fine here because
//! the UI runs one operation at a time and disables itself while it does, but
//! a caller that wanted several at once should move the work to
//! `spawn_blocking` rather than add more of these.

use std::collections::BTreeMap;
use std::path::PathBuf;

use collapse_core::{
    compress, compress_dir, extract_with, Algorithm, CompressionError, ExtractOptions, NameRules,
    Verify,
};
use serde::Serialize;

use crate::names::{substitutions_from, NameInspection};
use crate::paths::{inside, same_file};

/// Whether a path points at a directory (used by the UI to pick the icon and
/// the default archive name).
///
/// The one command with no `async`: it is a single `stat`, and the UI calls it
/// while the user is still choosing, so an IPC round trip through the runtime
/// would cost more than the call.
#[tauri::command]
pub fn is_directory(path: String) -> bool {
    std::path::Path::new(&path).is_dir()
}

/// Compress a file or a whole folder into `output`.
///
/// With `server` set, the work happens on a remote Collapse instance instead:
/// the bytes go out (a folder as a tar envelope), the archive comes back and
/// is written to the same `output` the local path would use. That exchange has
/// no read timeout, so it can outlast any patience: all the more reason for
/// the `async` on the attribute below (see the module header). Only `None`
/// means "this computer"; a `Some` that holds a blank string is an error from
/// `collapse-remote`, not a quiet fallback to local.
///
/// `overwrite` is the caller saying the user already agreed to replace what is
/// at `output`, which is what the native save dialog asks on every platform.
/// It is the `--force` of the CLI, and like it, it cannot buy past the two
/// guards below: agreeing to replace a file is not agreeing to destroy the
/// source, nor to destroy a file that is part of what is being archived.
///
/// `verify` picks between the two depths core checks a local archive at before
/// it is allowed to reach `output`. It is never "check or do not check": with
/// `false` the archive's own listing is read back and compared against the
/// entries it was meant to hold, which decompresses nothing and is what catches
/// the failure this exists for, a compression that died half way through and
/// finalised a valid-looking archive anyway. With `true` every entry is
/// decompressed as well, so zip's and 7z's per-entry checksums are checked; tar
/// stores no checksum over an entry's data at all, so there the deeper pass can
/// only confirm the archive is complete and well formed. It roughly doubles the
/// work, which is why it is the user's call and not the default.
///
/// It says nothing about a run with a `server`. The archive is built over there
/// and arrives as bytes this app never described, so there is no list of
/// expected entries here to check it against. The UI disables the checkbox in
/// that case, and a caller that asks anyway gets the archive rather than a
/// refusal: nothing about the request is harmful, it is just not something this
/// side can do.
#[tauri::command(async)]
pub fn compress_path(
    path: String,
    output: String,
    format: String,
    level: u32,
    server: Option<String>,
    overwrite: bool,
    verify: bool,
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
    // source before it is read, which is irrecoverable data loss). This stays
    // ahead of the check below so the more precise message wins when the output
    // is literally the source.
    if same_file(&source, &output_path) {
        return Err("The output is the same file as the source.".to_string());
    }

    if output_path.exists() {
        // Not even `overwrite` gets past this one. The backends list the tree
        // before creating the archive, so an output landing on a file that is
        // part of that tree is truncated and then archived in its truncated
        // state: the original is lost from the archive as much as from disk,
        // and the archive is corrupt too. Nobody agrees to that by answering a
        // "replace this file?" prompt, so consent cannot be the thing that
        // unlocks it.
        if inside(&source, &output_path) {
            return Err(format!(
                "The output is inside the folder being compressed: {output}. \
                 It would be destroyed instead of archived. Choose a location outside it."
            ));
        }
        if !overwrite {
            return Err(format!(
                "The output already exists: {output}. Delete it first, or choose another name."
            ));
        }
        // Deliberately NOT unlinked here. Neither branch touches this path
        // until the archive is whole: core writes a local archive to a
        // temporary beside it and renames it in only once it passes its check,
        // and the remote branch downloads every byte before it writes. So a
        // failed run leaves the previous archive exactly as it was, and
        // removing it up front would trade that away for nothing.
    }

    // Two depths, no "off": see the note on `verify` above. Unused by the
    // remote arm below, which has nothing of its own to check.
    let depth = if verify {
        Verify::Contents
    } else {
        Verify::Index
    };

    // `Some(_)` is the caller asking for a server, whatever it put in the
    // string: whether that string is usable is `collapse-remote`'s answer,
    // not this app's. Filtering here is what let the two front-ends disagree
    // (this one read `""` as "compress locally" and `"   "` as a real
    // destination, the CLI read both as a destination). "This computer" is
    // `null` from the UI, which arrives as `None`.
    match server.as_deref() {
        Some(server) => {
            let archive = collapse_remote::compress_path(server, &source, algorithm, level)
                .map_err(|e| e.to_string())?;
            std::fs::write(&output_path, archive).map_err(|e| e.to_string())?;
        }
        None if source.is_dir() => compress_dir(&source, &output_path, algorithm, level, depth)
            .map_err(|e| e.to_string())?,
        None => {
            let arcname = source
                .file_name()
                .ok_or_else(|| "Invalid source path.".to_string())?
                .to_string_lossy()
                .into_owned();
            compress(&source, &output_path, &arcname, algorithm, level, depth)
                .map_err(|e| e.to_string())?;
        }
    }

    Ok(output_path.to_string_lossy().into_owned())
}

/// Check that a remote Collapse server is reachable, so a typo surfaces in
/// the settings panel rather than at the end of an upload.
#[tauri::command(async)]
pub fn check_server(url: String) -> Result<(), String> {
    collapse_remote::check_health(&url).map_err(|e| e.to_string())
}

/// What an archive holds that this machine cannot write as ordinary files, so
/// the UI can ask the user about it **before** anything is extracted.
///
/// Reads the archive's listing and nothing else: no entry is decompressed and
/// nothing is created, which is what makes it safe to call the moment a
/// destination has been chosen. An empty answer (`entries` empty) means there
/// is no question to ask, which on macOS and Linux is nearly always the case:
/// the rules are **this host's**, and Unix refuses only the NUL byte, while
/// Windows refuses `? * < > | "`, control characters, a trailing dot or space
/// and the device names, and silently reinterprets a colon.
///
/// An archive whose listing cannot be read reports nothing rather than
/// failing, and that is deliberate: [`extract_archive`] is about to open the
/// same file and fail on it in the extractor's own vocabulary ("Could not find
/// EOCD", "failed to unpack `x`"), which is the message the user needs. This
/// command answering first would replace it with a worse one. Core's
/// `extract_with` makes the same choice for the same reason.
#[tauri::command(async)]
pub fn unwritable_names(archive: String) -> Result<NameInspection, String> {
    let archive_path = PathBuf::from(&archive);
    if !archive_path.exists() {
        return Err(format!("Not found: {archive}"));
    }
    // Named rather than left to `ExtractOptions`' default so that the rules the
    // dialog is built from are visibly the same ones `extract_archive` will
    // judge the answers by. They must agree: a dialog that asked about a
    // different alphabet than the extractor enforces would ask the wrong
    // questions and then fail anyway.
    let rules = NameRules::host();
    let report = collapse_core::unwritable_names_with(&archive_path, rules).unwrap_or_default();
    Ok(NameInspection::new(report, rules))
}

/// What came of an extraction attempt.
///
/// Two arms because two things can come back that are not failures of the
/// machine: the archive was extracted, or **nothing was written** and the user
/// has another naming question to answer. An `Err` from [`extract_archive`]
/// stays what it always was, something the user cannot fix by typing (a
/// missing file, a corrupt archive, a full disk).
///
/// Keeping the second case out of `Err` is what lets the dialog stay open on
/// the answer that needs changing while the error banner keeps meaning "this
/// did not work". The distinction is sound rather than hopeful: core raises
/// `CompressionError::Name` only while validating the answers and while
/// planning every entry's name from the listing, both of which happen before
/// the first byte is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Extraction {
    /// Written, under these names. Never the archive's names: an entry this
    /// host had to be given a different name for is reported as it is on disk,
    /// or the UI would list files nobody can find.
    Extracted { files: Vec<String> },
    /// Nothing was written, and here is what has to be answered differently.
    NameProblem { message: String },
}

/// Extract an archive into `output_dir` with the user's answers for the names
/// this host cannot write.
///
/// `replacements` maps one character to whatever should stand in for it, and
/// is empty for the ordinary archive that needs nothing. An empty *value* is a
/// real answer meaning "drop the character". The two adjustments no one can be
/// asked about (a trailing dot or space, a reserved device name) are applied by
/// core without appearing here; [`unwritable_names`] is what tells the user
/// they are coming.
///
/// A `BTreeMap` rather than a `HashMap`: with two bad keys in one payload, the
/// message has to name the same one on every run.
#[tauri::command(async)]
pub fn extract_archive(
    archive: String,
    output_dir: String,
    replacements: BTreeMap<String, String>,
) -> Result<Extraction, String> {
    let archive_path = PathBuf::from(&archive);
    if !archive_path.exists() {
        return Err(format!("Not found: {archive}"));
    }
    let output = PathBuf::from(&output_dir);

    let answers = match substitutions_from(&replacements) {
        Ok(answers) => answers,
        // A key that is not a single character is the webview's mistake rather
        // than the user's, but it is still a naming question that wrote
        // nothing, so it travels the same way and the dialog stays open.
        Err(problem) => {
            return Ok(Extraction::NameProblem {
                message: problem.to_string(),
            })
        }
    };
    // The host's rules, spelled out rather than left to the default, because
    // they have to be the same ones `unwritable_names` built the dialog from.
    let options = ExtractOptions::new()
        .with_rules(NameRules::host())
        .with_replacements(answers);

    match extract_with(&archive_path, &output, &options) {
        Ok(files) => Ok(Extraction::Extracted { files }),
        Err(CompressionError::Name(problem)) => Ok(Extraction::NameProblem {
            message: problem.to_string(),
        }),
        Err(other) => Err(other.to_string()),
    }
}
