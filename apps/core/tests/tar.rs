//! Tests for the tar backend (`compress_tar` / `extract_tar`).

use std::io::Read;

use collapse_core::compression::{compress_tar, compress_tar_dir, extract_tar};
use tar::{Builder, EntryType, Header};

const SAMPLE: &[u8] = b"Hello, Collapse! Hello, Collapse! Hello, Collapse! ";

fn source_file(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("sample.txt");
    std::fs::write(&p, SAMPLE).unwrap();
    p
}

fn data_header(size: u64) -> Header {
    let mut header = Header::new_gnu();
    header.set_size(size);
    header.set_mode(0o644);
    header
}

#[test]
fn creates_valid_tar() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let archive = dir.path().join("out.tar");

    compress_tar(&src, &archive, "sample.txt").unwrap();

    assert!(archive.exists());
    let f = std::fs::File::open(&archive).unwrap();
    let mut ar = tar::Archive::new(f);
    assert_eq!(ar.entries().unwrap().count(), 1);
}

#[test]
fn tar_contains_original_filename() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let archive = dir.path().join("out.tar");

    compress_tar(&src, &archive, "my_original.txt").unwrap();

    let f = std::fs::File::open(&archive).unwrap();
    let mut ar = tar::Archive::new(f);
    let names: Vec<String> = ar
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["my_original.txt"]);
}

#[test]
fn tar_content_is_preserved() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let archive = dir.path().join("out.tar");

    compress_tar(&src, &archive, "sample.txt").unwrap();

    let f = std::fs::File::open(&archive).unwrap();
    let mut ar = tar::Archive::new(f);
    let mut entry = ar.entries().unwrap().next().unwrap().unwrap();
    let mut buf = Vec::new();
    entry.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, SAMPLE);
}

// -- extract_tar tests --

#[test]
fn extract_tar_returns_file_list() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let archive = dir.path().join("out.tar");
    compress_tar(&src, &archive, "sample.txt").unwrap();

    let out = dir.path().join("extracted");
    let files = extract_tar(&archive, &out).unwrap();
    assert_eq!(files, vec!["sample.txt"]);
}

#[test]
fn extract_tar_content_matches_original() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let archive = dir.path().join("out.tar");
    compress_tar(&src, &archive, "sample.txt").unwrap();

    let out = dir.path().join("extracted");
    extract_tar(&archive, &out).unwrap();
    let content = std::fs::read(out.join("sample.txt")).unwrap();
    assert_eq!(content, SAMPLE);
}

#[test]
fn extract_tar_preserves_arcname() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let archive = dir.path().join("out.tar");
    compress_tar(&src, &archive, "renamed.dat").unwrap();

    let out = dir.path().join("extracted");
    let files = extract_tar(&archive, &out).unwrap();
    assert_eq!(files, vec!["renamed.dat"]);
    assert!(out.join("renamed.dat").exists());
}

#[test]
fn extract_tar_creates_output_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let archive = dir.path().join("out.tar");
    compress_tar(&src, &archive, "sample.txt").unwrap();

    let out = dir.path().join("deep").join("nested").join("dir");
    assert!(!out.exists());
    extract_tar(&archive, &out).unwrap();
    assert!(out.join("sample.txt").exists());
}

#[test]
fn extract_tar_nonexistent_archive_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let result = extract_tar(&dir.path().join("nope.tar"), &dir.path().join("out"));
    assert!(result.is_err());
}

#[test]
fn extract_tar_lists_nested_files_recursively() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("nested.tar");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut builder = Builder::new(f);
        for (name, content) in [
            ("top.txt", b"top" as &[u8]),
            ("a/mid.txt", b"mid"),
            ("a/b/deep.txt", b"deep"),
        ] {
            let mut header = data_header(content.len() as u64);
            builder.append_data(&mut header, name, content).unwrap();
        }
        builder.finish().unwrap();
    }

    let out = dir.path().join("extracted");
    let mut files = extract_tar(&archive, &out).unwrap();
    files.sort();
    assert_eq!(files, vec!["a/b/deep.txt", "a/mid.txt", "top.txt"]);
    assert!(out.join("a/b/deep.txt").exists());
}

#[test]
fn extract_tar_empty_archive_returns_empty_list() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("empty.tar");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut builder = Builder::new(f);
        builder.finish().unwrap();
    }

    let out = dir.path().join("extracted");
    let files = extract_tar(&archive, &out).unwrap();
    assert!(files.is_empty());
}

#[test]
fn extract_tar_directory_entries_are_skipped_in_file_list() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("with_dir.tar");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut builder = Builder::new(f);

        let mut dir_header = data_header(0);
        dir_header.set_entry_type(EntryType::Directory);
        dir_header.set_mode(0o755);
        builder
            .append_data(&mut dir_header, "subdir/", &b""[..])
            .unwrap();

        let content = b"inner content";
        let mut file_header = data_header(content.len() as u64);
        builder
            .append_data(&mut file_header, "subdir/inner.txt", &content[..])
            .unwrap();
        builder.finish().unwrap();
    }

    let out = dir.path().join("extracted");
    let files = extract_tar(&archive, &out).unwrap();
    // Directory entries must NOT appear in the returned list.
    assert_eq!(files, vec!["subdir/inner.txt"]);
    assert!(out.join("subdir/inner.txt").exists());
}

#[test]
fn compress_nonexistent_source_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("out.tar");
    let result = compress_tar(&dir.path().join("nope.txt"), &archive, "nope.txt");
    assert!(result.is_err());
    // The source is opened before the output is created, so no partial
    // archive is left behind.
    assert!(!archive.exists());
}

#[test]
fn extract_tar_reports_absolute_entry_names_as_written() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("abs.tar");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut builder = Builder::new(f);
        let content = b"abs";
        let mut header = data_header(content.len() as u64);
        // Bypass Builder validation to store an absolute entry name.
        let name = b"/abs.txt";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        builder.append(&header, &content[..]).unwrap();
        builder.finish().unwrap();
    }

    // The entry is extracted with the root stripped; the returned list must
    // match the path actually written, relative to the output dir.
    let out = dir.path().join("extracted");
    let files = extract_tar(&archive, &out).unwrap();
    assert_eq!(files, vec!["abs.txt"]);
    assert!(out.join("abs.txt").exists());
}

// -- compress_tar_dir (whole-directory archiving) --

/// Build a small tree under `<parent>/photos` and return the `photos` dir.
fn sample_tree(parent: &std::path::Path) -> std::path::PathBuf {
    let root = parent.join("photos");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("top.txt"), b"top").unwrap();
    std::fs::write(root.join("sub/inner.txt"), b"inner").unwrap();
    root
}

#[test]
fn compress_tar_dir_round_trips_tree() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = sample_tree(dir.path());
    let archive = dir.path().join("photos.tar");

    compress_tar_dir(&root, &archive).unwrap();

    // Entries are prefixed with the directory's own name.
    let out = dir.path().join("out");
    let mut files = extract_tar(&archive, &out).unwrap();
    files.sort();
    assert_eq!(files, vec!["photos/sub/inner.txt", "photos/top.txt"]);
    assert_eq!(std::fs::read(out.join("photos/top.txt")).unwrap(), b"top");
    assert_eq!(
        std::fs::read(out.join("photos/sub/inner.txt")).unwrap(),
        b"inner"
    );
}

#[test]
fn compress_tar_dir_preserves_empty_subdir() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("file.txt"), b"x").unwrap();
    let archive = dir.path().join("photos.tar");

    compress_tar_dir(&root, &archive).unwrap();

    let out = dir.path().join("out");
    extract_tar(&archive, &out).unwrap();
    assert!(
        out.join("photos/empty").is_dir(),
        "empty subdir was not preserved"
    );
}

#[test]
fn compress_tar_dir_rejects_non_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = source_file(dir.path());
    let archive = dir.path().join("out.tar");

    let result = compress_tar_dir(&file, &archive);
    assert!(result.is_err());
    assert!(
        !archive.exists(),
        "no archive should be created for a non-directory"
    );
}

#[test]
fn extract_tar_rejects_path_traversal() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.tar");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut builder = Builder::new(f);
        let content = b"evil";
        let mut header = data_header(content.len() as u64);
        // Builder::append_data refuses `..` paths, so write the malicious
        // name into the raw header bytes to bypass its validation.
        let name = b"../evil.txt";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        builder.append(&header, &content[..]).unwrap();
        builder.finish().unwrap();
    }

    let out = dir.path().join("out");
    let result = extract_tar(&archive, &out);
    assert!(result.is_err());
    // The entry must not have escaped the output directory.
    assert!(!dir.path().join("evil.txt").exists());
}
