use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::{CompressionError, NamePlan, Verify};

/// API level (1–5) → Deflate compresslevel (1–9).
const ZIP_LEVELS: [i64; 5] = [1, 3, 5, 7, 9];

pub fn compress_zip(
    source: &Path,
    output: &Path,
    arcname: &str,
    level: u32,
) -> Result<(), CompressionError> {
    let compress_level = ZIP_LEVELS[(level - 1) as usize];

    // Read the source before creating the output, the way `compress_tar` and
    // `compress_7z` already do: the other order left a zero-byte `.zip` behind
    // whenever the source could not be opened.
    let mut source_file = File::open(source)?;
    let mut buffer = Vec::new();
    source_file.read_to_end(&mut buffer)?;

    let output_file = File::create(output)?;
    let mut writer = ZipWriter::new(output_file);

    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(compress_level));

    writer
        .start_file(arcname, options)
        .map_err(|e| CompressionError::Failed(e.to_string()))?;
    writer.write_all(&buffer)?;

    writer
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(())
}

/// Archive a whole directory tree into a standard ZIP.
///
/// Entries are stored relative to (and prefixed with) the directory's own
/// name, and directory entries are emitted so empty folders survive the
/// round-trip. `level` maps the same way as [`compress_zip`].
pub fn compress_zip_dir(
    source_dir: &Path,
    output: &Path,
    level: u32,
) -> Result<(), CompressionError> {
    let entries = super::walk_tree(source_dir)?;
    let compress_level = ZIP_LEVELS[(level - 1) as usize];

    let output_file = File::create(output)?;
    let mut writer = ZipWriter::new(output_file);

    let file_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(compress_level));
    let dir_options = SimpleFileOptions::default();

    for entry in entries {
        if entry.is_dir {
            writer
                .add_directory(&entry.archive_name, dir_options)
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
        } else {
            // Read the member before naming it in the archive. The other order
            // put the name in first, so a member that could not be read still
            // appeared in the archive with nothing behind it, and the CRC
            // written for it was the CRC of nothing: an archive no reader could
            // fault, holding an empty file where a real one belonged. Whole
            // files are buffered here either way, so this costs nothing.
            let bytes = fs::read(&entry.disk_path)?;
            writer
                .start_file(&entry.archive_name, file_options)
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
            writer.write_all(&bytes)?;
        }
    }

    writer
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(())
}

/// Read a ZIP back for [`verify_archive`](super::verify_archive), returning the
/// names it holds.
///
/// At [`Verify::Index`] this reads the central directory and stops there. At
/// [`Verify::Contents`] every file entry is decompressed into a sink, which is
/// what makes the `zip` crate compare it against the CRC32 stored beside it:
/// the check fires on the read that reports end of file, so an entry has to be
/// read all the way through for it to happen at all.
pub(crate) fn read_zip_entries(
    archive: &Path,
    depth: Verify,
) -> Result<Vec<String>, CompressionError> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| {
        CompressionError::Failed(format!("the archive could not be read back: {e}"))
    })?;

    if depth == Verify::Index {
        return Ok(zip.file_names().map(|name| name.to_string()).collect());
    }

    let mut names = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| {
            CompressionError::Failed(format!("entry {i} could not be read back: {e}"))
        })?;
        let name = entry.name().to_string();
        if !entry.is_dir() {
            io::copy(&mut entry, &mut io::sink()).map_err(|e| {
                CompressionError::Failed(format!("entry {name:?} could not be read back: {e}"))
            })?;
        }
        names.push(name);
    }
    Ok(names)
}

/// The names a ZIP holds, in the order its entries are stored, reading the
/// central directory and nothing else.
///
/// Kept apart from [`read_zip_entries`] because the two answer different
/// questions and their failures read differently: that one is verifying an
/// archive this process just wrote, this one is looking at a file someone
/// handed us, so its error is the one extraction would give.
pub(crate) fn list_zip_entries(archive: &Path) -> Result<Vec<String>, CompressionError> {
    let file = File::open(archive)?;
    let zip = zip::ZipArchive::new(file).map_err(|e| CompressionError::Failed(e.to_string()))?;
    Ok((0..zip.len())
        .filter_map(|i| zip.name_for_index(i).map(str::to_string))
        .collect())
}

pub fn extract_zip(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    extract_zip_planned(archive, output_dir, &NamePlan::identity())
}

/// [`extract_zip`], writing each entry under the name `plan` gives it.
///
/// An entry the plan says nothing about keeps the name the archive spells,
/// which is what makes the plain [`extract_zip`] the same function.
pub(crate) fn extract_zip_planned(
    archive: &Path,
    output_dir: &Path,
    plan: &NamePlan,
) -> Result<Vec<String>, CompressionError> {
    let file = File::open(archive)?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| CompressionError::Failed(e.to_string()))?;

    let canonical_output = output_dir.canonicalize().or_else(|_| {
        fs::create_dir_all(output_dir)?;
        output_dir.canonicalize()
    })?;

    let mut extracted = Vec::new();

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| CompressionError::Failed(e.to_string()))?;

        let name = entry.name().to_string();

        // Prevent ZIP Slip: reject absolute paths and `..` components before
        // touching the filesystem, rather than trusting a resolved-path check
        // (which fails open when the target does not exist yet).
        let rel = super::sanitize_entry_path(&name).ok_or_else(|| {
            CompressionError::Failed(format!("Path traversal detected in archive entry: {name}"))
        })?;
        // The plan is built from the same name, one component at a time, so it
        // can only rename inside the output directory. `ensure_inside` below is
        // the backstop for the cases a lexical rule cannot reach: a caller that
        // judged the name under another host's rules, and a symlink already
        // sitting in the output.
        let rel = plan.written_as(&name).map_or(rel, Path::to_path_buf);
        let dest = canonical_output.join(&rel);

        if entry.is_dir() {
            fs::create_dir_all(&dest).map_err(|e| super::entry_error(&name, &dest, e))?;
            super::ensure_inside(&canonical_output, &dest, &name)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(|e| super::entry_error(&name, &dest, e))?;
                super::ensure_inside(&canonical_output, parent, &name)?;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            fs::write(&dest, &buf).map_err(|e| super::entry_error(&name, &dest, e))?;
            extracted.push(rel.to_string_lossy().to_string());
        }
    }

    Ok(extracted)
}
