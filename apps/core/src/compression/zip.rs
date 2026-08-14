use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::CompressionError;

/// API level (1–5) → Deflate compresslevel (1–9).
const ZIP_LEVELS: [i64; 5] = [1, 3, 5, 7, 9];

pub fn compress_zip(
    source: &Path,
    output: &Path,
    arcname: &str,
    level: u32,
) -> Result<(), CompressionError> {
    let compress_level = ZIP_LEVELS[(level - 1) as usize];

    let output_file = File::create(output)?;
    let mut writer = ZipWriter::new(output_file);

    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(compress_level));

    writer
        .start_file(arcname, options)
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    let mut source_file = File::open(source)?;
    let mut buffer = Vec::new();
    source_file.read_to_end(&mut buffer)?;
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
            writer
                .start_file(&entry.archive_name, file_options)
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
            let bytes = fs::read(&entry.disk_path)?;
            writer.write_all(&bytes)?;
        }
    }

    writer
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(())
}

pub fn extract_zip(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    let canonical_output = output_dir
        .canonicalize()
        .or_else(|_| {
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
            CompressionError::Failed(format!(
                "Path traversal detected in archive entry: {name}"
            ))
        })?;
        let dest = canonical_output.join(&rel);

        if entry.is_dir() {
            fs::create_dir_all(&dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            fs::write(&dest, &buf)?;
            extracted.push(rel.to_string_lossy().to_string());
        }
    }

    Ok(extracted)
}
