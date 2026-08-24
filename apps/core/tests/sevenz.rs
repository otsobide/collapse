//! Tests for the 7z backend (`compress_7z` / `extract_7z`).

use collapse_core::compression::{compress_7z, compress_7z_dir, compress_zip, extract_7z};
use collapse_core::CompressionError;
use sevenz_rust2::{SevenZArchiveEntry, SevenZWriter};

const SAMPLE: &[u8] = b"Hello, Collapse! Hello, Collapse! Hello, Collapse! ";

fn source_file(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("sample.txt");
    std::fs::write(&p, SAMPLE).unwrap();
    p
}

/// Normalize and sort an extracted listing so the expectations read the same
/// on a platform whose path separator is not `/`.
///
/// The entry names inside the archive are forward-slash separated everywhere;
/// `extract_7z` rebuilds each one as a `PathBuf` before stringifying it, so
/// the listing (and only the listing) comes back with `\` on Windows.
fn listing(paths: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = paths.iter().map(|p| p.replace('\\', "/")).collect();
    out.sort();
    out
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
    assert_eq!(listing(files), vec!["sample.txt"]);
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
    assert_eq!(listing(files), vec!["renamed.dat"]);
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
    let files = extract_7z(&archive, &out).unwrap();
    assert_eq!(listing(files), vec!["a/b/deep.txt", "a/mid.txt", "top.txt"]);
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

// -- compress_7z_dir (whole-directory archiving) --

/// Build a small tree under `<parent>/photos` and return the `photos` dir.
fn sample_tree(parent: &std::path::Path) -> std::path::PathBuf {
    let root = parent.join("photos");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("top.txt"), b"top").unwrap();
    std::fs::write(root.join("sub/inner.txt"), b"inner").unwrap();
    root
}

#[test]
fn compress_7z_dir_round_trips_tree() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = sample_tree(dir.path());
    let archive = dir.path().join("photos.7z");

    compress_7z_dir(&root, &archive, 3).unwrap();

    let out = dir.path().join("out");
    let files = extract_7z(&archive, &out).unwrap();
    assert_eq!(
        listing(files),
        vec!["photos/sub/inner.txt", "photos/top.txt"]
    );
    assert_eq!(std::fs::read(out.join("photos/top.txt")).unwrap(), b"top");
    assert_eq!(
        std::fs::read(out.join("photos/sub/inner.txt")).unwrap(),
        b"inner"
    );
}

#[test]
fn compress_7z_dir_preserves_empty_subdir() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::write(root.join("file.txt"), b"x").unwrap();
    let archive = dir.path().join("photos.7z");

    compress_7z_dir(&root, &archive, 1).unwrap();

    let out = dir.path().join("out");
    extract_7z(&archive, &out).unwrap();
    assert!(
        out.join("photos/empty").is_dir(),
        "empty subdir was not preserved"
    );
}

#[test]
fn compress_7z_dir_rejects_non_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let file = source_file(dir.path());
    let archive = dir.path().join("out.7z");

    let result = compress_7z_dir(&file, &archive, 1);
    assert!(result.is_err());
}

// -- error reporting --
//
// `sevenz_rust2::Error` implements `Display` as `Debug`, so the backend must
// translate it rather than stringify it. Every test below fails the moment a
// `map_err(from_sevenz)` in `compression/sevenz.rs` goes back to
// `CompressionError::Failed(e.to_string())`: they pin the whole message, and
// the Debug spelling is never the same string.

/// The six bytes every 7z file starts with, so a header can be hand-built far
/// enough to reach the check under test.
const SEVENZ_SIGNATURE: [u8; 6] = [b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C];

#[test]
fn a_missing_output_directory_reads_exactly_like_zip() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let missing = dir.path().join("does_not_exist");

    let sevenz_err = compress_7z(&src, &missing.join("out.7z"), "sample.txt", 3).unwrap_err();
    let zip_err = compress_zip(&src, &missing.join("out.zip"), "sample.txt", 3).unwrap_err();

    // The point of the mapping: the same mistake now reaches the same variant
    // through all three backends, not just the same prose.
    assert!(
        matches!(sevenz_err, CompressionError::Io(_)),
        "7z should report ENOENT as Io, got {sevenz_err:?}"
    );
    assert_eq!(sevenz_err.to_string(), zip_err.to_string());
    #[cfg(unix)]
    assert_eq!(
        sevenz_err.to_string(),
        "IO error: No such file or directory (os error 2)"
    );
    // The dependency puts the absolute output path in the error it hands back;
    // dropping it is half of why the mapping exists (the server forwards this
    // string to unauthenticated clients).
    assert!(
        !sevenz_err.to_string().contains("out.7z"),
        "the message leaks the output path: {sevenz_err}"
    );
}

#[test]
fn a_file_that_is_not_an_archive_is_described_in_words() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("corrupt.7z");
    std::fs::write(&archive, b"this is plain text, not an archive").unwrap();

    let err = extract_7z(&archive, &dir.path().join("out")).unwrap_err();

    assert_eq!(
        err.to_string(),
        "Compression failed: not a 7z archive: the file does not start with a 7z signature"
    );
    // The old spelling was `BadSignature([116, 104, 105, 115, 32, 105])`: the
    // first six bytes of the user's file, printed as a byte array.
    assert!(
        !err.to_string().contains("116"),
        "the message still echoes the file's bytes: {err}"
    );
}

#[test]
fn an_unreadable_format_version_is_described_in_words() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("future.7z");
    let mut bytes = SEVENZ_SIGNATURE.to_vec();
    // Major/minor, read straight after the signature. Major 0 is the only one
    // the format defines, so anything else stops the reader right here.
    bytes.extend_from_slice(&[9, 4]);
    std::fs::write(&archive, &bytes).unwrap();

    let err = extract_7z(&archive, &dir.path().join("out")).unwrap_err();

    assert_eq!(
        err.to_string(),
        "Compression failed: unsupported 7z format version 9.4"
    );
}

#[test]
fn a_corrupt_header_is_described_in_words() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("bent.7z");
    let mut bytes = SEVENZ_SIGNATURE.to_vec();
    bytes.extend_from_slice(&[0, 0]); // version 0.0
    bytes.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes()); // claimed header CRC
    bytes.extend_from_slice(&[7u8; 20]); // 20 header bytes that do not hash to it
    std::fs::write(&archive, &bytes).unwrap();

    let err = extract_7z(&archive, &dir.path().join("out")).unwrap_err();

    assert_eq!(
        err.to_string(),
        "Compression failed: the 7z archive is corrupt: a checksum did not match"
    );
}

#[test]
fn a_truncated_archive_is_reported_as_an_io_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let archive = dir.path().join("half.7z");
    compress_7z(&src, &archive, "sample.txt", 3).unwrap();
    let whole = std::fs::read(&archive).unwrap();
    std::fs::write(&archive, &whole[..whole.len() / 2]).unwrap();

    let err = extract_7z(&archive, &dir.path().join("out")).unwrap_err();

    // The dependency reports the short read as its `Io` variant, so the
    // mapping hands back the `io::Error` verbatim rather than inventing a
    // "truncated" sentence it cannot actually distinguish from a bad disk.
    assert!(
        matches!(err, CompressionError::Io(ref io) if io.kind() == std::io::ErrorKind::UnexpectedEof),
        "expected an UnexpectedEof Io, got {err:?}"
    );
    assert_eq!(err.to_string(), "IO error: failed to fill whole buffer");
}

#[test]
fn a_rejected_entry_name_reads_exactly_like_zip() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.7z");
    {
        let mut writer = SevenZWriter::create(&archive).unwrap();
        let mut entry = SevenZArchiveEntry::default();
        entry.name = "../escape.txt".to_string();
        writer
            .push_archive_entry(entry, Some(b"pwned".as_slice()))
            .unwrap();
        writer.finish().unwrap();
    }

    let err = extract_7z(&archive, &dir.path().join("out")).unwrap_err();

    // `extract_7z` builds this one itself, as a `sevenz_rust2::Error::Other`,
    // so it has to survive the round trip through the dependency unchanged:
    // byte for byte what zip and tar produce for the same entry name.
    assert_eq!(
        err.to_string(),
        "Compression failed: Path traversal detected in archive entry: ../escape.txt"
    );
}
