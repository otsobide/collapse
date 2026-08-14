use std::fs;
use std::path::Path;

use sevenz_rust2::lzma::LZMA2Options;
use sevenz_rust2::{SevenZArchiveEntry, SevenZMethodConfiguration, SevenZMethod, SevenZWriter};

use super::CompressionError;

/// API level (1–5) → LZMA2 preset (1–9).
const SEVENZ_PRESETS: [u32; 5] = [1, 3, 5, 7, 9];

pub fn compress_7z(
    source: &Path,
    output: &Path,
    arcname: &str,
    level: u32,
) -> Result<(), CompressionError> {
    let preset = SEVENZ_PRESETS[(level - 1) as usize];

    let content = fs::read(source)?;

    let mut writer =
        SevenZWriter::create(output).map_err(|e| CompressionError::Failed(e.to_string()))?;

    let lzma2_opts = LZMA2Options::with_preset(preset);
    writer.set_content_methods(vec![SevenZMethodConfiguration::new(SevenZMethod::LZMA2)
        .with_options(lzma2_opts.into())]);

    let mut entry = SevenZArchiveEntry::default();
    entry.name = arcname.to_string();

    writer
        .push_archive_entry(entry, Some(content.as_slice()))
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    writer
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

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

    let mut writer =
        SevenZWriter::create(output).map_err(|e| CompressionError::Failed(e.to_string()))?;
    let lzma2_opts = LZMA2Options::with_preset(preset);
    writer.set_content_methods(vec![SevenZMethodConfiguration::new(SevenZMethod::LZMA2)
        .with_options(lzma2_opts.into())]);

    for entry in entries {
        // from_path sets is_directory/has_stream from the on-disk node.
        let sz_entry = SevenZArchiveEntry::from_path(&entry.disk_path, entry.archive_name);
        if entry.is_dir {
            writer
                .push_archive_entry::<&[u8]>(sz_entry, None)
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
        } else {
            let content = fs::read(&entry.disk_path)?;
            writer
                .push_archive_entry(sz_entry, Some(content.as_slice()))
                .map_err(|e| CompressionError::Failed(e.to_string()))?;
        }
    }

    writer
        .finish()
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(())
}

pub fn extract_7z(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    fs::create_dir_all(output_dir)?;
    let canonical_output = output_dir.canonicalize()?;

    let file = std::fs::File::open(archive)?;

    // Validate each entry name and write it ourselves, so a malicious name is
    // rejected *before* any bytes reach disk. (`decompress` writes first and
    // asks questions later, which lets `..` entries escape output_dir.)
    let mut extracted = Vec::new();
    sevenz_rust2::decompress_with_extract_fn(file, &canonical_output, |entry, reader, _dest| {
        let name = entry.name().to_string();
        let rel = super::sanitize_entry_path(&name).ok_or_else(|| {
            sevenz_rust2::Error::other(format!(
                "Path traversal detected in archive entry: {name}"
            ))
        })?;
        let dest = canonical_output.join(&rel);

        if entry.is_directory() {
            fs::create_dir_all(&dest).map_err(sevenz_rust2::Error::io)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(sevenz_rust2::Error::io)?;
            }
            let mut buf = Vec::new();
            reader.read_to_end(&mut buf).map_err(sevenz_rust2::Error::io)?;
            fs::write(&dest, &buf).map_err(sevenz_rust2::Error::io)?;
            extracted.push(rel.to_string_lossy().to_string());
        }
        Ok(true)
    })
    .map_err(|e| CompressionError::Failed(e.to_string()))?;

    Ok(extracted)
}
