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

pub fn extract_7z(archive: &Path, output_dir: &Path) -> Result<Vec<String>, CompressionError> {
    fs::create_dir_all(output_dir)?;
    let canonical_output = output_dir.canonicalize()?;

    let file = std::fs::File::open(archive)?;
    sevenz_rust2::decompress(file, &canonical_output)
        .map_err(|e| CompressionError::Failed(e.to_string()))?;

    // Verify all extracted files stay within output_dir (path traversal check).
    let mut extracted = Vec::new();
    collect_files(&canonical_output, &canonical_output, &mut extracted)?;
    for rel in &extracted {
        let full = canonical_output.join(rel).canonicalize()?;
        if !full.starts_with(&canonical_output) {
            return Err(CompressionError::Failed(format!(
                "Path traversal detected in archive entry: {rel}"
            )));
        }
    }
    Ok(extracted)
}

fn collect_files(
    base: &Path,
    dir: &Path,
    out: &mut Vec<String>,
) -> Result<(), CompressionError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .to_string();
            out.push(rel);
        }
    }
    Ok(())
}
