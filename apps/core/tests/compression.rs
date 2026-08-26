//! Tests for the collapse-core public API: the `Algorithm` enum and the
//! `compress`/`extract` dispatchers.

use std::path::Path;

use collapse_core::{compress, compress_dir, extract, Algorithm, CompressionError, Verify};

/// Normalize and sort an extracted listing so the expectations read the same
/// on a platform whose path separator is not `/`.
///
/// The archives are identical everywhere (every backend writes forward-slash
/// entry names), but `extract` rebuilds each entry as a `PathBuf` and
/// stringifies it, so the same archive answers `data/a.txt` on Unix and
/// `data\a.txt` on Windows. Only the returned listing differs, never what
/// lands on disk, so the separator is normalized here rather than in the
/// product.
fn listing(paths: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = paths.iter().map(|p| p.replace('\\', "/")).collect();
    out.sort();
    out
}

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
    let result = compress(
        Path::new("/x"),
        Path::new("/y"),
        "f",
        Algorithm::Zip,
        0,
        Verify::Index,
    );
    assert!(matches!(result, Err(CompressionError::InvalidLevel(0))));
}

#[test]
fn compress_invalid_level_six() {
    let result = compress(
        Path::new("/x"),
        Path::new("/y"),
        "f",
        Algorithm::Zip,
        6,
        Verify::Index,
    );
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

/// An extension is a file name, not a wire value. Windows and macOS fold case
/// in the filesystem and plenty of tools write `.ZIP`, so a valid archive was
/// being refused as an unknown format for the spelling of its name alone.
#[test]
fn from_extension_is_case_insensitive() {
    for (spelling, expected) in [
        ("ZIP", Algorithm::Zip),
        ("Zip", Algorithm::Zip),
        ("zIp", Algorithm::Zip),
        ("7Z", Algorithm::SevenZ),
        ("TAR", Algorithm::Tar),
        ("Tar", Algorithm::Tar),
    ] {
        assert_eq!(
            Algorithm::from_extension(spelling),
            Some(expected),
            "{spelling} names the same format as its lowercase spelling"
        );
    }
}

/// Lenient about spelling, not about formats: a case-insensitive match must not
/// become a match that accepts anything.
#[test]
fn from_extension_still_refuses_a_format_it_does_not_have() {
    for unknown in ["RAR", "Gz", "TAR.GZ", "ZIPX", " zip", "zip "] {
        assert_eq!(
            Algorithm::from_extension(unknown),
            None,
            "{unknown:?} is not one of the three"
        );
    }
}

/// The two parsers are deliberately different rules, and this is the one that
/// must NOT follow: `FromStr` reads the `algorithm=` query parameter of
/// `POST /compress` and the CLI's `--format`, both wire values with a
/// documented enum. Loosening it here would silently widen the API.
#[test]
fn from_str_stays_strict_about_case() {
    for shouted in ["ZIP", "Zip", "7Z", "TAR"] {
        assert!(
            shouted.parse::<Algorithm>().is_err(),
            "{shouted} is not a wire value, whatever from_extension says about file names"
        );
    }
}

/// What names the files this toolkit writes stays lowercase, so making the
/// reader case insensitive cannot start producing `photos.ZIP`.
#[test]
fn extension_is_always_written_lowercase() {
    for algorithm in [Algorithm::Zip, Algorithm::SevenZ, Algorithm::Tar] {
        let ext = algorithm.extension();
        assert_eq!(
            ext,
            ext.to_ascii_lowercase(),
            "{algorithm} names output files"
        );
    }
}

// -- extract dispatcher tests --

/// The dispatcher end to end, not just the parser: a real archive written by
/// this toolkit, renamed the way a user or another tool would, still opens.
/// The parser test above would keep passing if `extract` stopped calling it.
#[test]
fn extract_opens_an_archive_whatever_the_case_of_its_name() {
    for (algorithm, shouted) in [
        (Algorithm::Zip, "OUT.ZIP"),
        (Algorithm::SevenZ, "Out.7Z"),
        (Algorithm::Tar, "OUT.Tar"),
    ] {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("input.txt");
        std::fs::write(&src, b"shouted name").unwrap();

        let archive = dir.path().join(shouted);
        compress(&src, &archive, "input.txt", algorithm, 1, Verify::Index).unwrap();

        let out = dir.path().join("extracted");
        let files = extract(&archive, &out).expect("the name is understood");
        assert_eq!(listing(files), vec!["input.txt"], "{shouted}");
        assert_eq!(
            std::fs::read(out.join("input.txt")).unwrap(),
            b"shouted name",
            "{shouted}"
        );
    }
}

#[test]
fn extract_dispatches_zip() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("input.txt");
    std::fs::write(&src, b"dispatch zip").unwrap();

    let archive = dir.path().join("out.zip");
    compress(
        &src,
        &archive,
        "input.txt",
        Algorithm::Zip,
        1,
        Verify::Index,
    )
    .unwrap();

    let out = dir.path().join("extracted");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(listing(files), vec!["input.txt"]);
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
    compress(
        &src,
        &archive,
        "input.txt",
        Algorithm::SevenZ,
        1,
        Verify::Index,
    )
    .unwrap();

    let out = dir.path().join("extracted");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(listing(files), vec!["input.txt"]);
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
    compress(
        &src,
        &archive,
        "input.txt",
        Algorithm::Tar,
        1,
        Verify::Index,
    )
    .unwrap();

    let out = dir.path().join("extracted");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(listing(files), vec!["input.txt"]);
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
    compress(
        &src,
        &reference,
        "input.txt",
        Algorithm::Tar,
        1,
        Verify::Index,
    )
    .unwrap();
    let reference_bytes = std::fs::read(&reference).unwrap();

    for level in 2..=5 {
        let archive = dir.path().join(format!("out_l{level}.tar"));
        compress(
            &src,
            &archive,
            "input.txt",
            Algorithm::Tar,
            level,
            Verify::Index,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&archive).unwrap(),
            reference_bytes,
            "level {level} produced different output"
        );
    }
}

#[test]
fn tar_out_of_range_level_is_still_rejected() {
    let result = compress(
        Path::new("/x"),
        Path::new("/y"),
        "f",
        Algorithm::Tar,
        0,
        Verify::Index,
    );
    assert!(matches!(result, Err(CompressionError::InvalidLevel(0))));
    let result = compress(
        Path::new("/x"),
        Path::new("/y"),
        "f",
        Algorithm::Tar,
        6,
        Verify::Index,
    );
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
        Verify::Index,
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
    compress_dir(&root, &archive, Algorithm::Tar, 1, Verify::Index).unwrap();

    let out = dir.path().join("out");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(listing(files), vec!["data/a.txt"]);
    assert_eq!(std::fs::read(out.join("data/a.txt")).unwrap(), b"alpha");
}

#[test]
fn compress_dir_dispatches_zip() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();

    let archive = dir.path().join("data.zip");
    compress_dir(&root, &archive, Algorithm::Zip, 3, Verify::Index).unwrap();

    let out = dir.path().join("out");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(listing(files), vec!["data/a.txt"]);
    assert_eq!(std::fs::read(out.join("data/a.txt")).unwrap(), b"alpha");
}

#[test]
fn compress_dir_dispatches_7z() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();

    let archive = dir.path().join("data.7z");
    compress_dir(&root, &archive, Algorithm::SevenZ, 3, Verify::Index).unwrap();

    let out = dir.path().join("out");
    let files = extract(&archive, &out).unwrap();
    assert_eq!(listing(files), vec!["data/a.txt"]);
    assert_eq!(std::fs::read(out.join("data/a.txt")).unwrap(), b"alpha");
}

#[test]
fn compress_dir_invalid_level_is_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("data");
    std::fs::create_dir_all(&root).unwrap();

    let archive = dir.path().join("data.tar");
    assert!(matches!(
        compress_dir(&root, &archive, Algorithm::Tar, 0, Verify::Index),
        Err(CompressionError::InvalidLevel(0))
    ));
    assert!(matches!(
        compress_dir(&root, &archive, Algorithm::Tar, 6, Verify::Index),
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
