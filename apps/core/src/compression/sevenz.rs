use std::fs;
use std::io;
use std::path::Path;

use sevenz_rust2::lzma::LZMA2Options;
use sevenz_rust2::{SevenZArchiveEntry, SevenZMethod, SevenZMethodConfiguration, SevenZWriter};

use super::{CompressionError, NamePlan, Verify};

/// API level (1–5) → LZMA2 preset (1–9).
const SEVENZ_PRESETS: [u32; 5] = [1, 3, 5, 7, 9];

/// One sentence for a failed CRC, wherever it is raised.
///
/// The dependency reports the same condition two ways (a variant of its own for
/// the header, an `io::Error` wrapping that variant for a decoded stream), and
/// a user has no use for the difference.
const CHECKSUM_MISMATCH: &str = "the 7z archive is corrupt: a checksum did not match";

/// Translate a `sevenz_rust2::Error` into this crate's error type.
///
/// Never call `to_string()` on one of those: the dependency implements
/// `Display` as `Debug` (sevenz-rust2 0.13.2, `src/error.rs`), so stringifying
/// it hands the user a struct dump such as
/// `Io(Os { code: 2, kind: NotFound, ... }, "/var/lib/collapse/jobs/<id>/a.7z")`.
/// That is unreadable, and the absolute path in it is a disclosure once the
/// server forwards a job's error message to its clients.
///
/// So the two variants that carry an `io::Error` unwrap to
/// [`CompressionError::Io`], which is exactly what zip and tar already produce
/// for the same failure (they reach it through `std`), and every other variant
/// gets a sentence written here.
///
/// The match is deliberately exhaustive: when the dependency grows a variant
/// this stops compiling, instead of quietly reintroducing a dump through a
/// catch-all arm.
fn from_sevenz(e: sevenz_rust2::Error) -> CompressionError {
    use sevenz_rust2::Error as SevenZError;

    let message = match e {
        // The second field is the file name the dependency was working on;
        // dropping it is the point. `io::Error`'s own Display already says
        // what went wrong ("No such file or directory (os error 2)"), and the
        // caller knows which path it asked for.
        SevenZError::Io(io, _) | SevenZError::FileOpen(io, _) => return from_sevenz_io(io),
        // `Other` is prose, and it is also the variant `extract_7z` builds for
        // a rejected entry name, so passing it through unchanged is what keeps
        // the traversal message identical across the three formats.
        SevenZError::Other(reason) => reason.into_owned(),
        SevenZError::BadSignature(_) => {
            "not a 7z archive: the file does not start with a 7z signature".to_string()
        }
        SevenZError::UnsupportedVersion { major, minor } => {
            format!("unsupported 7z format version {major}.{minor}")
        }
        // Raised for the start header and for an entry alike, so the sentence
        // must not claim which one.
        SevenZError::ChecksumVerificationFailed => CHECKSUM_MISMATCH.to_string(),
        SevenZError::NextHeaderCrcMismatch => {
            "the 7z archive is corrupt: its header failed the checksum".to_string()
        }
        // The five "bad terminated" variants each name an internal header
        // section, and the byte they carry is the property id the parser did
        // not expect. Neither tells a user anything the sentence does not.
        SevenZError::BadTerminatedStreamsInfo(_)
        | SevenZError::BadTerminatedUnpackInfo
        | SevenZError::BadTerminatedPackInfo(_)
        | SevenZError::BadTerminatedSubStreamsInfo
        | SevenZError::BadTerminatedheader(_) => {
            "the 7z archive is corrupt: its header is malformed".to_string()
        }
        SevenZError::ExternalUnsupported => {
            "this 7z archive keeps its file list in an external stream, which is not supported"
                .to_string()
        }
        // The payload is quoted nowhere on purpose: two of the construction
        // sites pass a method name, a third passes `format!("{:?}", id)` over
        // the raw method id bytes, and there is no way to tell them apart.
        SevenZError::UnsupportedCompressionMethod(_) => {
            "the 7z archive uses a compression method this build cannot decode".to_string()
        }
        SevenZError::MaxMemLimited { max_kb, actaul_kb } => format!(
            "decoding the 7z archive needs {actaul_kb} KB of memory, over the {max_kb} KB limit"
        ),
        // Nothing here ever supplies a password, so an encrypted archive is
        // simply out of reach; both variants mean the same thing to a user.
        SevenZError::PasswordRequired | SevenZError::MaybeBadPassword(_) => {
            "the 7z archive is encrypted, and passwords are not supported".to_string()
        }
        // Same reasoning as UnsupportedCompressionMethod: one construction
        // site formats a method id with `{:?}`.
        SevenZError::Unsupported(_) => {
            "the 7z archive uses a feature this build does not support".to_string()
        }
        SevenZError::FileNotFound => "the entry was not found in the 7z archive".to_string(),
    };

    CompressionError::Failed(message)
}

/// Unwrap an `io::Error` the dependency handed back.
///
/// Almost always it is a real IO failure and belongs in
/// [`CompressionError::Io`] untouched. The exception is the CRC guard on a
/// decoded stream: it reports a mismatch as an `io::Error` *wrapping*
/// `sevenz_rust2::Error::ChecksumVerificationFailed`, and since that type's
/// `Display` is its `Debug`, passing it through prints the bare variant name at
/// the user. It is not an IO problem either, so it gets the same sentence the
/// header-side mismatch already gets.
fn from_sevenz_io(io: std::io::Error) -> CompressionError {
    let checksum = io
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<sevenz_rust2::Error>())
        .is_some_and(|inner| matches!(inner, sevenz_rust2::Error::ChecksumVerificationFailed));
    if checksum {
        CompressionError::Failed(CHECKSUM_MISMATCH.to_string())
    } else {
        CompressionError::Io(io)
    }
}

/// Read a 7z back for [`verify_archive`](super::verify_archive), returning the
/// names it holds.
///
/// [`Verify::Index`] decodes the archive header (which is itself compressed)
/// and reads no entry. [`Verify::Contents`] decodes every entry into a sink,
/// which is what makes the dependency compare each one against the CRC it
/// stores per file.
pub(crate) fn read_7z_entries(
    archive: &Path,
    depth: Verify,
) -> Result<Vec<String>, CompressionError> {
    if depth == Verify::Index {
        let listing = sevenz_rust2::Archive::open(archive).map_err(from_sevenz)?;
        return Ok(listing.files.iter().map(|f| f.name.clone()).collect());
    }

    let mut reader = sevenz_rust2::SevenZReader::open(archive, sevenz_rust2::Password::empty())
        .map_err(from_sevenz)?;
    let mut names = Vec::new();
    reader
        .for_each_entries(|entry, stream| {
            names.push(entry.name.clone());
            // Reading to the end is what makes the CRC guard fire; the bytes
            // themselves are not wanted anywhere.
            match io::copy(stream, &mut io::sink()) {
                Ok(_) => Ok(true),
                // `Other` is the one variant `from_sevenz` passes through
                // unchanged, which is how a sentence written here survives the
                // mapping and keeps the entry's name attached to it.
                Err(e) => Err(sevenz_rust2::Error::other(format!(
                    "entry {:?} could not be read back: {}",
                    entry.name,
                    describe_stream_failure(e)
                ))),
            }
        })
        .map_err(from_sevenz)?;
    Ok(names)
}

/// Say in words why a decoded stream stopped, with no prefix: the caller is
/// building a longer sentence around it.
fn describe_stream_failure(e: std::io::Error) -> String {
    match from_sevenz_io(e) {
        CompressionError::Failed(message) => message,
        CompressionError::Io(io) => io.to_string(),
        other => other.to_string(),
    }
}

pub fn compress_7z(
    source: &Path,
    output: &Path,
    arcname: &str,
    level: u32,
) -> Result<(), CompressionError> {
    let preset = SEVENZ_PRESETS[(level - 1) as usize];

    let content = fs::read(source)?;

    let mut writer = SevenZWriter::create(output).map_err(from_sevenz)?;

    let lzma2_opts = LZMA2Options::with_preset(preset);
    writer.set_content_methods(vec![
        SevenZMethodConfiguration::new(SevenZMethod::LZMA2).with_options(lzma2_opts.into())
    ]);

    let mut entry = SevenZArchiveEntry::default();
    entry.name = arcname.to_string();

    writer
        .push_archive_entry(entry, Some(content.as_slice()))
        .map_err(from_sevenz)?;

    // `finish` is the one call here that answers with a plain `io::Error`, so
    // `?` alone already lands it in `CompressionError::Io`.
    writer.finish()?;

    Ok(())
}

/// Archive a whole directory tree into a standard 7z.
///
/// Entries are stored relative to (and prefixed with) the directory's own
/// name, and directory entries are emitted so empty folders survive the
/// round-trip. `level` maps the same way as [`compress_7z`].
pub fn compress_7z_dir(
    source_dir: &Path,
    output: &Path,
    level: u32,
) -> Result<(), CompressionError> {
    let entries = super::walk_tree(source_dir)?;
    let preset = SEVENZ_PRESETS[(level - 1) as usize];

    let mut writer = SevenZWriter::create(output).map_err(from_sevenz)?;
    let lzma2_opts = LZMA2Options::with_preset(preset);
    writer.set_content_methods(vec![
        SevenZMethodConfiguration::new(SevenZMethod::LZMA2).with_options(lzma2_opts.into())
    ]);

    for entry in entries {
        // from_path sets is_directory/has_stream from the on-disk node.
        let sz_entry = SevenZArchiveEntry::from_path(&entry.disk_path, entry.archive_name);
        if entry.is_dir {
            writer
                .push_archive_entry::<&[u8]>(sz_entry, None)
                .map_err(from_sevenz)?;
        } else {
            let content = fs::read(&entry.disk_path)?;
            writer
                .push_archive_entry(sz_entry, Some(content.as_slice()))
                .map_err(from_sevenz)?;
        }
    }

    // See `compress_7z`: `finish` fails with a plain `io::Error`.
    writer.finish()?;

    Ok(())
}

/// The names a 7z holds, decoding its header and no entry.
///
/// The one format where reading a listing to plan names and reading one to
/// verify an archive are the same operation with the same error vocabulary
/// (zip and tar each need their own; see their `list_*` functions), so this is
/// [`read_7z_entries`] at [`Verify::Index`] rather than a second copy of it.
pub(crate) fn list_7z_entries(archive: &Path) -> Result<Vec<String>, CompressionError> {
    read_7z_entries(archive, Verify::Index)
}

pub fn extract_7z(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    extract_7z_planned(archive, output_dir, &NamePlan::identity())
}

/// [`extract_7z`], writing each entry under the name `plan` gives it.
pub(crate) fn extract_7z_planned(
    archive: &Path,
    output_dir: &Path,
    plan: &NamePlan,
) -> Result<Vec<String>, CompressionError> {
    fs::create_dir_all(output_dir)?;
    let canonical_output = output_dir.canonicalize()?;

    let file = std::fs::File::open(archive)?;

    // A write failure has to come back with the entry that caused it, and the
    // callback can only fail with the dependency's own error type, whose
    // variants have no room for that. So the real error is set aside here and
    // the callback returns a placeholder that never reaches a user: it is
    // replaced below, before the dependency's own error is even looked at.
    let mut write_failure: Option<CompressionError> = None;

    // Validate each entry name and write it ourselves, so a malicious name is
    // rejected *before* any bytes reach disk. (`decompress` writes first and
    // asks questions later, which lets `..` entries escape output_dir.)
    let mut extracted = Vec::new();
    let outcome = sevenz_rust2::decompress_with_extract_fn(
        file,
        &canonical_output,
        |entry, reader, _dest| {
            let name = entry.name().to_string();
            let rel = super::sanitize_entry_path(&name).ok_or_else(|| {
                sevenz_rust2::Error::other(format!(
                    "Path traversal detected in archive entry: {name}"
                ))
            })?;
            // See `extract_zip_planned`: the plan renames inside the output,
            // and `ensure_inside` below is the backstop.
            let rel = plan.written_as(&name).map_or(rel, Path::to_path_buf);
            let dest = canonical_output.join(&rel);

            // The callback can only fail with sevenz's own error type, so the
            // real one is stashed for the caller and a placeholder returned.
            let mut stash = |e: CompressionError| {
                write_failure = Some(e);
                sevenz_rust2::Error::other("the entry could not be written")
            };

            if entry.is_directory() {
                fs::create_dir_all(&dest)
                    .map_err(|e| super::entry_error(&name, &dest, e))
                    .and_then(|()| super::ensure_inside(&canonical_output, &dest, &name))
                    .map_err(&mut stash)?;
            } else {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| super::entry_error(&name, &dest, e))
                        .and_then(|()| super::ensure_inside(&canonical_output, parent, &name))
                        .map_err(&mut stash)?;
                }
                let mut buf = Vec::new();
                reader
                    .read_to_end(&mut buf)
                    .map_err(sevenz_rust2::Error::io)?;
                fs::write(&dest, &buf)
                    .map_err(|e| super::entry_error(&name, &dest, e))
                    .map_err(&mut stash)?;
                extracted.push(rel.to_string_lossy().to_string());
            }
            Ok(true)
        },
    );

    if let Some(failure) = write_failure {
        return Err(failure);
    }
    outcome.map_err(from_sevenz)?;

    Ok(extracted)
}
