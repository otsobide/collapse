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
    fn creates_valid_7z() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.7z");

        compress_7z(&src, &archive, "sample.txt", 1).unwrap();
        assert!(archive.exists());
    }

    #[test]
    fn sevenz_contains_original_filename() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.7z");

        compress_7z(&src, &archive, "my_original.txt", 1).unwrap();

        let extract = dir.path().join("extract");
        let file = std::fs::File::open(&archive).unwrap();
        sevenz_rust2::decompress(file, &extract).unwrap();
        assert!(extract.join("my_original.txt").exists());
    }

    #[test]
    fn sevenz_content_is_preserved() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.7z");

        compress_7z(&src, &archive, "sample.txt", 1).unwrap();

        let extract = dir.path().join("extract");
        let file = std::fs::File::open(&archive).unwrap();
        sevenz_rust2::decompress(file, &extract).unwrap();
        let content = std::fs::read(extract.join("sample.txt")).unwrap();
        assert_eq!(content, SAMPLE);
    }

    #[test]
    fn all_levels_produce_valid_7z() {
        for level in 1..=5 {
            let dir = tempfile::TempDir::new().unwrap();
            let src = source_file(dir.path());
            let archive = dir.path().join(format!("out_l{level}.7z"));

            compress_7z(&src, &archive, "sample.txt", level).unwrap();
            assert!(archive.exists(), "level {level} failed");
        }
    }

    // -- extract_7z tests --

    #[test]
    fn extract_7z_returns_file_list() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.7z");
        compress_7z(&src, &archive, "sample.txt", 1).unwrap();

        let out = dir.path().join("extracted");
        let files = extract_7z(&archive, &out).unwrap();
        assert_eq!(files, vec!["sample.txt"]);
    }

    #[test]
    fn extract_7z_content_matches_original() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.7z");
        compress_7z(&src, &archive, "sample.txt", 3).unwrap();

        let out = dir.path().join("extracted");
        extract_7z(&archive, &out).unwrap();
        let content = std::fs::read(out.join("sample.txt")).unwrap();
        assert_eq!(content, SAMPLE);
    }

    #[test]
    fn extract_7z_preserves_arcname() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.7z");
        compress_7z(&src, &archive, "renamed.dat", 1).unwrap();

        let out = dir.path().join("extracted");
        let files = extract_7z(&archive, &out).unwrap();
        assert_eq!(files, vec!["renamed.dat"]);
        assert!(out.join("renamed.dat").exists());
    }

    #[test]
    fn extract_7z_creates_output_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = source_file(dir.path());
        let archive = dir.path().join("out.7z");
        compress_7z(&src, &archive, "sample.txt", 1).unwrap();

        let out = dir.path().join("deep").join("nested").join("dir");
        assert!(!out.exists());
        extract_7z(&archive, &out).unwrap();
        assert!(out.join("sample.txt").exists());
    }

    #[test]
    fn extract_7z_nonexistent_archive_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = extract_7z(&dir.path().join("nope.7z"), &dir.path().join("out"));
        assert!(result.is_err());
    }

    #[test]
    fn collect_files_recursive() {
        let dir = tempfile::TempDir::new().unwrap();
        let base = dir.path().join("root");
        std::fs::create_dir_all(base.join("a/b")).unwrap();
        std::fs::write(base.join("top.txt"), b"top").unwrap();
        std::fs::write(base.join("a/mid.txt"), b"mid").unwrap();
        std::fs::write(base.join("a/b/deep.txt"), b"deep").unwrap();

        let canonical = base.canonicalize().unwrap();
        let mut out = Vec::new();
        collect_files(&canonical, &canonical, &mut out).unwrap();
        out.sort();
        assert_eq!(out, vec!["a/b/deep.txt", "a/mid.txt", "top.txt"]);
    }

    #[test]
    fn collect_files_empty_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let base = dir.path().join("empty");
        std::fs::create_dir_all(&base).unwrap();

        let canonical = base.canonicalize().unwrap();
        let mut out = Vec::new();
        collect_files(&canonical, &canonical, &mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn compress_nonexistent_source_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = compress_7z(
            &dir.path().join("nope.txt"),
            &dir.path().join("out.7z"),
            "nope.txt",
            1,
        );
        assert!(result.is_err());
    }

    #[test]
    fn extract_7z_roundtrip_all_levels() {
        for level in 1..=5 {
            let dir = tempfile::TempDir::new().unwrap();
            let src = source_file(dir.path());
            let archive = dir.path().join(format!("out_l{level}.7z"));
            compress_7z(&src, &archive, "sample.txt", level).unwrap();

            let out = dir.path().join(format!("extracted_l{level}"));
            extract_7z(&archive, &out).unwrap();
            let content = std::fs::read(out.join("sample.txt")).unwrap();
            assert_eq!(content, SAMPLE, "roundtrip failed at level {level}");
        }
    }
}
