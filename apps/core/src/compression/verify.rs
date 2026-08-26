//! Reading an archive back to check it says what it was meant to say.
//!
//! Compressors here finalise on drop, so a run that dies partway through still
//! closes out a *structurally valid* archive: zip writes its central directory,
//! tar writes its end-of-archive blocks, and what is left opens cleanly while
//! silently missing whatever had not been written yet. Nothing about the
//! archive itself says so, which is why the check has to compare it against
//! what the caller asked for rather than merely asking whether it parses.

use std::collections::BTreeSet;
use std::path::Path;

use super::{Algorithm, CompressionError};

/// How thoroughly to check an archive once it is written.
///
/// [`compress`](super::compress) and [`compress_dir`](super::compress_dir) both
/// check before the archive reaches the destination, so a failure leaves
/// nothing there rather than an archive that looks fine and is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verify {
    /// Read the archive's own listing back and confirm it names exactly the
    /// entries that were meant to go in. Nothing is decompressed: for zip and
    /// 7z this reads a header, for tar it walks the headers skipping the data.
    ///
    /// This is the depth that catches the failure this exists for, a
    /// compression that stopped early and finalised anyway.
    Index,
    /// The listing, and then every entry decompressed into a sink. Roughly
    /// doubles the work, which is why it is the caller's choice.
    ///
    /// What that buys differs by format, and the difference is worth stating
    /// exactly rather than implying all three get the same thing:
    ///
    /// - **zip** stores a CRC32 per entry, and the `zip` crate compares it as
    ///   the entry is read to its end. A flipped bit in the data is caught.
    /// - **7z** stores a CRC per file (and per pack stream), verified the same
    ///   way while the entry is decoded. A flipped bit in the data is caught.
    /// - **tar** stores *no* checksum over an entry's data at all: its `cksum`
    ///   header field covers the 512 byte header and nothing else. So here this
    ///   reads every entry through and confirms the archive is well formed and
    ///   complete (headers intact, no member cut short, the listing as
    ///   expected), and that is the whole of it. A flipped bit inside a tar
    ///   member's data is not detectable from the archive, by this or by any
    ///   other reader.
    ///
    /// Nothing is ever written to disk, so this needs no space of its own.
    Contents,
}

/// At most this many entry names are spelled out before a message says "and N
/// more": a tree with ten thousand files must not produce a ten thousand name
/// error.
const NAMES_SHOWN: usize = 5;

/// Check that `archive` holds exactly the entries in `expected`, at `depth`.
///
/// `algorithm` says how to read it rather than the file extension, because the
/// dispatchers verify a temporary file whose name is not an archive name.
///
/// Entry names are compared as sets and with any trailing `/` removed: a
/// directory is spelled `photos/` by zip and tar and `photos` by 7z, and the
/// order entries come back in is the writer's business, not the caller's.
pub fn verify_archive(
    archive: &Path,
    algorithm: Algorithm,
    expected: &[String],
    depth: Verify,
) -> Result<(), CompressionError> {
    let found_names = match algorithm {
        Algorithm::SevenZ => super::read_7z_entries(archive, depth),
        Algorithm::Tar => super::read_tar_entries(archive, depth),
        Algorithm::Zip => super::read_zip_entries(archive, depth),
    }
    .map_err(|e| failed(archive, reason_of(e)))?;

    let expected: BTreeSet<&str> = expected.iter().map(|n| normalize(n)).collect();
    let found: BTreeSet<&str> = found_names.iter().map(|n| normalize(n)).collect();

    let missing: Vec<&str> = expected.difference(&found).copied().collect();
    let unexpected: Vec<&str> = found.difference(&expected).copied().collect();
    if missing.is_empty() && unexpected.is_empty() {
        return Ok(());
    }

    // Both halves are reported, and by name. "the archive has 3 entries, not 4"
    // tells whoever reads the log nothing about which file went missing.
    let mut problems = Vec::new();
    if !missing.is_empty() {
        problems.push(format!(
            "{} missing: {}",
            counted(&missing),
            listed(&missing)
        ));
    }
    if !unexpected.is_empty() {
        problems.push(format!(
            "{} unexpected: {}",
            counted(&unexpected),
            listed(&unexpected)
        ));
    }
    Err(failed(archive, problems.join("; ")))
}

/// Directory entries carry a trailing separator in zip and tar and none in 7z,
/// so it cannot be part of an entry's identity here.
fn normalize(name: &str) -> &str {
    name.trim_end_matches('/')
}

fn failed(archive: &Path, reason: String) -> CompressionError {
    CompressionError::VerificationFailed {
        archive: archive.to_path_buf(),
        reason,
    }
}

/// Render an error raised while reading the archive back as the `reason` half
/// of a verification failure.
///
/// The variant's own prefix ("Compression failed: ", "IO error: ") is dropped
/// because it would read as if compressing had failed, which is precisely the
/// distinction [`CompressionError::VerificationFailed`] exists to draw.
fn reason_of(error: CompressionError) -> String {
    match error {
        CompressionError::Io(io) => io.to_string(),
        CompressionError::Failed(message) => message,
        other => other.to_string(),
    }
}

fn counted(names: &[&str]) -> String {
    match names.len() {
        1 => "1 entry is".to_string(),
        n => format!("{n} entries are"),
    }
}

fn listed(names: &[&str]) -> String {
    let shown: Vec<String> = names
        .iter()
        .take(NAMES_SHOWN)
        .map(|n| format!("{n:?}"))
        .collect();
    match names.len().checked_sub(NAMES_SHOWN) {
        Some(rest) if rest > 0 => format!("{} and {rest} more", shown.join(", ")),
        _ => shown.join(", "),
    }
}
