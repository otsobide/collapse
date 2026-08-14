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
        let dest = canonical_output.join(&name);

        // Prevent ZIP Slip: ensure the resolved path stays within output_dir.
        let canonical_dest = dest.canonicalize().unwrap_or_else(|_| dest.clone());
        if !canonical_dest.starts_with(&canonical_output) {
            return Err(CompressionError::Failed(format!(
                "Path traversal detected in archive entry: {name}"
            )));
        }

        if entry.is_dir() {
            fs::create_dir_all(&dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            fs::write(&dest, &buf)?;
            extracted.push(name);
        }
    }

    Ok(extracted)
}
