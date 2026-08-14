//! Security tests: extraction must never write outside the output directory,
//! regardless of what entry names a malicious archive contains (path
//! traversal / "ZIP Slip"). Each test crafts an archive whose entry name tries
//! to escape, then asserts extraction is rejected AND nothing was written to
//! the parent directory.

use std::io::Write;

use collapse_core::compression::{extract_7z, extract_tar, extract_zip};
use collapse_core::{extract, Algorithm};
use sevenz_rust2::{SevenZArchiveEntry, SevenZWriter};
use tar::{Builder, Header};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

// -- archive builders with a chosen (malicious) entry name --

fn malicious_zip(archive: &std::path::Path, entry_name: &str) {
    let f = std::fs::File::create(archive).unwrap();
    let mut w = ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    // The zip crate writes the name verbatim, so a traversal name survives.
    w.start_file(entry_name, opts).unwrap();
    w.write_all(b"pwned").unwrap();
    w.finish().unwrap();
}

fn malicious_7z(archive: &std::path::Path, entry_name: &str) {
    let mut w = SevenZWriter::create(archive).unwrap();
    let mut entry = SevenZArchiveEntry::default();
    entry.name = entry_name.to_string();
    w.push_archive_entry(entry, Some(b"pwned".as_slice()))
        .unwrap();
    w.finish().unwrap();
}

fn malicious_tar(archive: &std::path::Path, entry_name: &str) {
    let f = std::fs::File::create(archive).unwrap();
    let mut builder = Builder::new(f);
    let content = b"pwned";
    let mut header = Header::new_gnu();
    header.set_size(content.len() as u64);
    header.set_mode(0o644);
    // Builder::append_data rejects `..`/absolute names, so write the raw
    // header bytes to smuggle a traversal name past its validation.
    let name = entry_name.as_bytes();
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_cksum();
    builder.append(&header, &content[..]).unwrap();
    builder.finish().unwrap();
}

/// Assert extraction failed and no file leaked into the parent of `out`.
fn assert_contained(result: Result<Vec<String>, impl std::fmt::Debug>, escaped: &std::path::Path) {
    assert!(
        result.is_err(),
        "extraction should reject traversal, got {result:?}"
    );
    assert!(
        !escaped.exists(),
        "a file escaped the output directory: {}",
        escaped.display()
    );
}

// -- ZIP --

#[test]
fn zip_rejects_parent_dir_traversal() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.zip");
    malicious_zip(&archive, "../escape.txt");

    let out = dir.path().join("out");
    assert_contained(extract_zip(&archive, &out), &dir.path().join("escape.txt"));
}

#[test]
fn zip_rejects_nested_parent_dir_traversal() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.zip");
    malicious_zip(&archive, "sub/../../escape.txt");

    let out = dir.path().join("out");
    assert_contained(extract_zip(&archive, &out), &dir.path().join("escape.txt"));
}

#[test]
fn zip_rejects_absolute_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.zip");
    let target = dir.path().join("abs_escape.txt");
    malicious_zip(&archive, target.to_str().unwrap());

    let out = dir.path().join("out");
    assert_contained(extract_zip(&archive, &out), &target);
}

// -- 7z --

#[test]
fn sevenz_rejects_parent_dir_traversal() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.7z");
    malicious_7z(&archive, "../escape.txt");

    let out = dir.path().join("out");
    assert_contained(extract_7z(&archive, &out), &dir.path().join("escape.txt"));
}

#[test]
fn sevenz_rejects_nested_parent_dir_traversal() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.7z");
    malicious_7z(&archive, "sub/../../escape.txt");

    let out = dir.path().join("out");
    assert_contained(extract_7z(&archive, &out), &dir.path().join("escape.txt"));
}

#[test]
fn sevenz_rejects_absolute_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.7z");
    let target = dir.path().join("abs_escape.txt");
    malicious_7z(&archive, target.to_str().unwrap());

    let out = dir.path().join("out");
    assert_contained(extract_7z(&archive, &out), &target);
}

// -- tar --

#[test]
fn tar_rejects_parent_dir_traversal() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.tar");
    malicious_tar(&archive, "../escape.txt");

    let out = dir.path().join("out");
    assert_contained(extract_tar(&archive, &out), &dir.path().join("escape.txt"));
}

// -- via the public extract() dispatcher, for every format --

#[test]
fn dispatch_extract_rejects_traversal_for_every_format() {
    for (ext, build) in [
        ("zip", malicious_zip as fn(&std::path::Path, &str)),
        ("7z", malicious_7z),
        ("tar", malicious_tar),
    ] {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = dir.path().join(format!("evil.{ext}"));
        build(&archive, "../escape.txt");

        let out = dir.path().join("out");
        let escaped = dir.path().join("escape.txt");
        assert!(
            extract(&archive, &out).is_err(),
            "{ext}: dispatcher should reject traversal"
        );
        assert!(!escaped.exists(), "{ext}: a file escaped the output dir");
    }
}

/// A legitimate archive with ordinary nested names must still extract fine —
/// the guard rejects traversal, not every path with a separator.
#[test]
fn benign_nested_names_still_extract() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.txt");
    std::fs::write(&src, b"safe").unwrap();

    for algo in [Algorithm::Zip, Algorithm::SevenZ, Algorithm::Tar] {
        let archive = dir.path().join(format!("ok.{}", algo.extension()));
        collapse_core::compress(&src, &archive, "nested/dir/input.txt", algo, 1).unwrap();

        let out = dir.path().join(format!("out_{}", algo.extension()));
        let files = extract(&archive, &out).unwrap();
        assert_eq!(files, vec!["nested/dir/input.txt"], "{algo}");
        assert_eq!(std::fs::read(out.join("nested/dir/input.txt")).unwrap(), b"safe");
    }
}
