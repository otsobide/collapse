//! Tests for the 7z backend (`compress_7z` / `extract_7z`).

use collapse_core::compression::{compress_7z, extract_7z};
use sevenz_rust2::{SevenZArchiveEntry, SevenZWriter};

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
fn extract_7z_lists_nested_files_recursively() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("nested.7z");
    {
        let mut writer = SevenZWriter::create(&archive).unwrap();
        for (name, content) in [
            ("top.txt", b"top" as &[u8]),
            ("a/mid.txt", b"mid"),
            ("a/b/deep.txt", b"deep"),
        ] {
            let mut entry = SevenZArchiveEntry::default();
            entry.name = name.to_string();
            writer.push_archive_entry(entry, Some(content)).unwrap();
        }
        writer.finish().unwrap();
    }

    let out = dir.path().join("extracted");
    let mut files = extract_7z(&archive, &out).unwrap();
    files.sort();
    assert_eq!(files, vec!["a/b/deep.txt", "a/mid.txt", "top.txt"]);
    assert!(out.join("a/b/deep.txt").exists());
}

#[test]
fn extract_7z_empty_archive_returns_empty_list() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("empty.7z");
    SevenZWriter::create(&archive).unwrap().finish().unwrap();

    let out = dir.path().join("extracted");
    let files = extract_7z(&archive, &out).unwrap();
    assert!(files.is_empty());
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
