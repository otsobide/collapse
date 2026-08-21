//! Tests for the collapse-core public API: the `Algorithm` enum and the
//! `compress`/`extract` dispatchers.

use std::path::Path;

use collapse_core::{compress, compress_dir, extract, Algorithm, CompressionError};

#[test]
fn algorithm_display() {
    assert_eq!(Algorithm::SevenZ.to_string(), "7z");
    assert_eq!(Algorithm::Tar.to_string(), "tar");
    assert_eq!(Algorithm::Zip.to_string(), "zip");
}

#[test]
fn algorithm_from_str() {
    assert_eq!("7z".parse::<Algorithm>().unwrap(), Algorithm::SevenZ);
    assert_eq!("tar".parse::<Algorithm>().unwrap(), Algorithm::Tar);
    assert_eq!("zip".parse::<Algorithm>().unwrap(), Algorithm::Zip);
    assert!("invalid".parse::<Algorithm>().is_err());
}

#[test]
fn algorithm_extension() {
    assert_eq!(Algorithm::SevenZ.extension(), "7z");
    assert_eq!(Algorithm::Tar.extension(), "tar");
    assert_eq!(Algorithm::Zip.extension(), "zip");
}

#[test]
fn algorithm_media_type() {
    assert_eq!(
        Algorithm::SevenZ.media_type(),
        "application/x-7z-compressed"
    );
    assert_eq!(Algorithm::Tar.media_type(), "application/x-tar");
    assert_eq!(Algorithm::Zip.media_type(), "application/zip");
}

#[test]
fn algorithm_serde_roundtrip() {
    let json = serde_json::to_string(&Algorithm::SevenZ).unwrap();
    assert_eq!(json, "\"7z\"");
    let parsed: Algorithm = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, Algorithm::SevenZ);

    let json = serde_json::to_string(&Algorithm::Tar).unwrap();
    assert_eq!(json, "\"tar\"");
    let parsed: Algorithm = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, Algorithm::Tar);

    let json = serde_json::to_string(&Algorithm::Zip).unwrap();
    assert_eq!(json, "\"zip\"");
    let parsed: Algorithm = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, Algorithm::Zip);
}

#[test]
fn compress_invalid_level_zero() {
    let result = compress(Path::new("/x"), Path::new("/y"), "f", Algorithm::Zip, 0);
    assert!(matches!(result, Err(CompressionError::InvalidLevel(0))));
}

#[test]
fn compress_invalid_level_six() {
    let result = compress(Path::new("/x"), Path::new("/y"), "f", Algorithm::Zip, 6);
    assert!(matches!(result, Err(CompressionError::InvalidLevel(6))));
}

// -- Algorithm::from_extension tests --

#[test]
fn from_extension_zip() {
    assert_eq!(Algorithm::from_extension("zip"), Some(Algorithm::Zip));
}

#[test]
fn from_extension_7z() {
    assert_eq!(Algorithm::from_extension("7z"), Some(Algorithm::SevenZ));
}

#[test]
fn from_extension_tar() {
    assert_eq!(Algorithm::from_extension("tar"), Some(Algorithm::Tar));
}

#[test]
fn from_extension_unknown() {
    assert_eq!(Algorithm::from_extension("rar"), None);
    assert_eq!(Algorithm::from_extension("gz"), None);
    assert_eq!(Algorithm::from_extension(""), None);
}

// -- extract dispatcher tests --

#[test]
fn extract_dispatches_zip() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.txt");
    std::fs::write(&src, b"dispatch zip").unwrap();

    let archive = dir.path().join("out.zip");
    compress(&src, &archive, "input.txt", Algorithm::Zip, 1).unwrap();

    let out = dir.path().join("extracted");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(files, vec!["input.txt"]);
    assert_eq!(
        std::fs::read(out.join("input.txt")).unwrap(),
        b"dispatch zip"
    );
}

#[test]
fn extract_dispatches_7z() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.txt");
    std::fs::write(&src, b"dispatch 7z").unwrap();

    let archive = dir.path().join("out.7z");
    compress(&src, &archive, "input.txt", Algorithm::SevenZ, 1).unwrap();

    let out = dir.path().join("extracted");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(files, vec!["input.txt"]);
    assert_eq!(
        std::fs::read(out.join("input.txt")).unwrap(),
        b"dispatch 7z"
    );
}

#[test]
fn extract_dispatches_tar() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.txt");
    std::fs::write(&src, b"dispatch tar").unwrap();

    let archive = dir.path().join("out.tar");
    compress(&src, &archive, "input.txt", Algorithm::Tar, 1).unwrap();

    let out = dir.path().join("extracted");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(files, vec!["input.txt"]);
    assert_eq!(
        std::fs::read(out.join("input.txt")).unwrap(),
        b"dispatch tar"
    );
}

#[test]
fn tar_level_is_ignored() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.txt");
    std::fs::write(&src, b"same bytes at every level").unwrap();

    // All valid levels must produce byte-identical tar archives.
    let reference = dir.path().join("out_l1.tar");
    compress(&src, &reference, "input.txt", Algorithm::Tar, 1).unwrap();
    let reference_bytes = std::fs::read(&reference).unwrap();

    for level in 2..=5 {
        let archive = dir.path().join(format!("out_l{level}.tar"));
        compress(&src, &archive, "input.txt", Algorithm::Tar, level).unwrap();
        assert_eq!(
            std::fs::read(&archive).unwrap(),
            reference_bytes,
            "level {level} produced different output"
        );
    }
}

#[test]
fn tar_out_of_range_level_is_still_rejected() {
    let result = compress(Path::new("/x"), Path::new("/y"), "f", Algorithm::Tar, 0);
    assert!(matches!(result, Err(CompressionError::InvalidLevel(0))));
    let result = compress(Path::new("/x"), Path::new("/y"), "f", Algorithm::Tar, 6);
    assert!(matches!(result, Err(CompressionError::InvalidLevel(6))));
}

#[test]
fn extract_unknown_extension_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let fake = dir.path().join("archive.rar");
    std::fs::write(&fake, b"not an archive").unwrap();

    let result = extract(&fake, &dir.path().join("out"));
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Unknown archive extension"));
}

#[test]
fn compress_nonexistent_source_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let result = compress(
        &dir.path().join("ghost.txt"),
        &dir.path().join("out.zip"),
        "ghost.txt",
        Algorithm::Zip,
        1,
    );
    assert!(result.is_err());
}

#[test]
fn compression_error_display_invalid_level() {
    let err = CompressionError::InvalidLevel(99);
    assert!(err.to_string().contains("99"));
    assert!(err.to_string().contains("between 1 and 5"));
}

#[test]
fn compression_error_display_failed() {
    let err = CompressionError::Failed("boom".into());
    assert!(err.to_string().contains("boom"));
}

#[test]
fn compression_error_display_io() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
    let err = CompressionError::from(io_err);
    assert!(err.to_string().contains("not found"));
}

// -- compress_dir dispatcher --

#[test]
fn compress_dir_dispatches_tar() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();

    let archive = dir.path().join("data.tar");
    compress_dir(&root, &archive, Algorithm::Tar, 1).unwrap();

    let out = dir.path().join("out");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(files, vec!["data/a.txt"]);
    assert_eq!(std::fs::read(out.join("data/a.txt")).unwrap(), b"alpha");
}

#[test]
fn compress_dir_dispatches_zip() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();

    let archive = dir.path().join("data.zip");
    compress_dir(&root, &archive, Algorithm::Zip, 3).unwrap();

    let out = dir.path().join("out");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(files, vec!["data/a.txt"]);
    assert_eq!(std::fs::read(out.join("data/a.txt")).unwrap(), b"alpha");
}

#[test]
fn compress_dir_dispatches_7z() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();

    let archive = dir.path().join("data.7z");
    compress_dir(&root, &archive, Algorithm::SevenZ, 3).unwrap();

    let out = dir.path().join("out");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(files, vec!["data/a.txt"]);
    assert_eq!(std::fs::read(out.join("data/a.txt")).unwrap(), b"alpha");
}

#[test]
fn compress_dir_invalid_level_is_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).unwrap();

    let archive = dir.path().join("data.tar");
    assert!(matches!(
        compress_dir(&root, &archive, Algorithm::Tar, 0),
        Err(CompressionError::InvalidLevel(0))
    ));
    assert!(matches!(
        compress_dir(&root, &archive, Algorithm::Tar, 6),
        Err(CompressionError::InvalidLevel(6))
    ));
}

#[test]
fn extract_no_extension_errors() {
    let dir = tempfile::TempDir::new().unwrap();
    let fake = dir.path().join("noext");
    std::fs::write(&fake, b"no extension").unwrap();

    let result = extract(&fake, &dir.path().join("out"));
    assert!(result.is_err());
}
