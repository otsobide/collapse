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

#[cfg(test)]
mod tests {
    use super::*;
    const SAMPLE: &[u8] = b"Hello, Collapse! Hello, Collapse! Hello, Collapse! ";

    fn source_file(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("sample.txt");
        std::fs::write(&p, SAMPLE).unwrap();
        p
    }

    #[test]
    fn creates_valid_zip() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.zip");

        compress_zip(&src, &archive, "sample.txt", 1).unwrap();

        assert!(archive.exists());
        let f = std::fs::File::open(&archive).unwrap();
        assert!(zip::ZipArchive::new(f).is_ok());
    }

    #[test]
    fn zip_contains_original_filename() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.zip");

        compress_zip(&src, &archive, "my_original.txt", 1).unwrap();

        let f = std::fs::File::open(&archive).unwrap();
        let ar = zip::ZipArchive::new(f).unwrap();
        assert!(ar.file_names().any(|n| n == "my_original.txt"));
    }

    #[test]
    fn zip_content_is_preserved() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.zip");

        compress_zip(&src, &archive, "sample.txt", 3).unwrap();

        let f = std::fs::File::open(&archive).unwrap();
        let mut ar = zip::ZipArchive::new(f).unwrap();
        let mut entry = ar.by_name("sample.txt").unwrap();
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).unwrap();
        assert_eq!(buf, SAMPLE);
    }

    #[test]
    fn all_levels_produce_valid_zip() {
        for level in 1..=5 {
            let dir = tempfile::TempDir::new().unwrap();
            let src = source_file(dir.path());
            let archive = dir.path().join(format!("out_l{level}.zip"));

            compress_zip(&src, &archive, "sample.txt", level).unwrap();

            let f = std::fs::File::open(&archive).unwrap();
            assert!(zip::ZipArchive::new(f).is_ok(), "level {level} failed");
        }
    }

    // -- extract_zip tests --

    #[test]
    fn extract_zip_returns_file_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.zip");
        compress_zip(&src, &archive, "sample.txt", 1).unwrap();

        let out = dir.path().join("extracted");
        let files = extract_zip(&archive, &out).unwrap();
        assert_eq!(files, vec!["sample.txt"]);
    }

    #[test]
    fn extract_zip_content_matches_original() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.zip");
        compress_zip(&src, &archive, "sample.txt", 3).unwrap();

        let out = dir.path().join("extracted");
        extract_zip(&archive, &out).unwrap();
        let content = std::fs::read(out.join("sample.txt")).unwrap();
        assert_eq!(content, SAMPLE);
    }

    #[test]
    fn extract_zip_preserves_arcname() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.zip");
        compress_zip(&src, &archive, "renamed.dat", 1).unwrap();

        let out = dir.path().join("extracted");
        let files = extract_zip(&archive, &out).unwrap();
        assert_eq!(files, vec!["renamed.dat"]);
        assert!(out.join("renamed.dat").exists());
    }

    #[test]
    fn extract_zip_creates_output_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.zip");
        compress_zip(&src, &archive, "sample.txt", 1).unwrap();

        let out = dir.path().join("deep").join("nested").join("dir");
        assert!(!out.exists());
        extract_zip(&archive, &out).unwrap();
        assert!(out.join("sample.txt").exists());
    }

    #[test]
    fn extract_zip_nonexistent_archive_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = extract_zip(&dir.path().join("nope.zip"), &dir.path().join("out"));
        assert!(result.is_err());
    }

    #[test]
    fn extract_zip_directory_entries_are_skipped_in_file_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = dir.path().join("with_dir.zip");

        // Manually create a ZIP with a directory entry + a file inside it.
        {
            let f = std::fs::File::create(&archive).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            w.add_directory("subdir/", opts).unwrap();
            w.start_file("subdir/inner.txt", opts).unwrap();
            w.write_all(b"inner content").unwrap();
            w.finish().unwrap();
        }

        let out = dir.path().join("extracted");
        let files = extract_zip(&archive, &out).unwrap();
        // Directory entries must NOT appear in the returned list.
        assert_eq!(files, vec!["subdir/inner.txt"]);
        assert!(out.join("subdir/inner.txt").exists());
    }

    #[test]
    fn extract_zip_roundtrip_all_levels() {
        for level in 1..=5 {
            let dir = tempfile::TempDir::new().unwrap();
            let src = source_file(dir.path());
            let archive = dir.path().join(format!("out_l{level}.zip"));
            compress_zip(&src, &archive, "sample.txt", level).unwrap();

            let out = dir.path().join(format!("extracted_l{level}"));
            extract_zip(&archive, &out).unwrap();
            let content = std::fs::read(out.join("sample.txt")).unwrap();
            assert_eq!(content, SAMPLE, "roundtrip failed at level {level}");
        }
    }
}
