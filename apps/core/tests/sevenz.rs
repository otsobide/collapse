//! Tests for the 7z backend (`compress_7z` / `extract_7z`).

use std::io::Write;
use std::path::Path;

use collapse_core::compression::{
    compress_7z, compress_7z_dir, compress_zip, extract_7z, extract_zip,
};
use collapse_core::CompressionError;
use sevenz_rust2::{SevenZArchiveEntry, SevenZWriter};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

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
    let out = dir.path().join("out");
    let err = extract_7z(&dir.path().join("nope.7z"), &out).unwrap_err();

    // `is_err()` alone passed whatever the message was, including the struct
    // dump 7z used to produce. Pin the variant and the sentence: this failure
    // never reaches the dependency at all (`File::open` refuses first), so it
    // must read like every other missing file in the crate.
    assert!(
        matches!(err, CompressionError::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound),
        "expected a NotFound Io, got {err:?}"
    );
    #[cfg(unix)]
    assert_eq!(
        err.to_string(),
        "IO error: No such file or directory (os error 2)"
    );
    // `extract_7z` creates the output directory before it opens the archive,
    // so a missing archive still leaves the directory behind; the point here
    // is only that it stays empty.
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
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
    let output = dir.path().join("out.7z");
    let err = compress_7z(&dir.path().join("nope.txt"), &output, "nope.txt", 1).unwrap_err();

    // `is_err()` alone could not tell this apart from a struct dump. The read
    // of the source happens before the writer exists, so this must be a plain
    // `Io` with std's own wording.
    assert!(
        matches!(err, CompressionError::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound),
        "expected a NotFound Io, got {err:?}"
    );
    #[cfg(unix)]
    assert_eq!(
        err.to_string(),
        "IO error: No such file or directory (os error 2)"
    );
    // And the observable effect: the source is read before the writer exists,
    // so a missing source leaves no stub archive behind. All three backends
    // agree on this now; `compress_zip` used to create its output first and
    // leave a zero-byte `.zip`, and `zip.rs` pins that it no longer does.
    assert!(!output.exists(), "a stub archive was left on disk");
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

    let err = compress_7z_dir(&file, &archive, 1).unwrap_err();

    // This one never reaches the dependency: `walk_tree` refuses first, so the
    // message is core's own and it does name the path (the caller passed it).
    // Pinned so the check cannot be silently downgraded to "some error".
    assert_eq!(
        err.to_string(),
        format!("Compression failed: Not a directory: {}", file.display())
    );
    assert!(!archive.exists(), "a stub archive was left on disk");
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

/// A one entry archive with a byte flipped inside the entry's own data.
///
/// Written with the COPY method on purpose: with LZMA2 a flipped byte usually
/// breaks the decoder before the checksum is ever compared, which reaches a
/// different arm of the mapping.
fn bitrotted_archive(dir: &Path) -> std::path::PathBuf {
    let archive = dir.join("bitrot.7z");
    {
        let mut writer = SevenZWriter::create(&archive).unwrap();
        writer.set_content_methods(vec![sevenz_rust2::SevenZMethodConfiguration::new(
            sevenz_rust2::SevenZMethod::COPY,
        )]);
        let entry = SevenZArchiveEntry {
            name: "sample.txt".to_string(),
            ..Default::default()
        };
        writer.push_archive_entry(entry, Some(SAMPLE)).unwrap();
        writer.finish().unwrap();
    }

    // The packed streams start straight after the 32 byte signature header, so
    // this lands in the entry's stored bytes and nowhere near a header.
    let mut bytes = std::fs::read(&archive).unwrap();
    bytes[40] ^= 0xFF;
    std::fs::write(&archive, &bytes).unwrap();
    archive
}

/// The dependency reports a header CRC mismatch as a variant of its own, but a
/// *data* CRC mismatch as an `io::Error` wrapping that variant, and its
/// `Display` is its `Debug`, so unwrapped it reached the user as
/// `IO error: ChecksumVerificationFailed`. Both spellings must arrive as the
/// same sentence, since the difference means nothing to whoever reads it.
#[test]
fn a_flipped_bit_in_the_data_is_described_in_words() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = bitrotted_archive(dir.path());

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
    // so it has to survive the round trip through the dependency unchanged.
    assert_eq!(
        err.to_string(),
        "Compression failed: Path traversal detected in archive entry: ../escape.txt"
    );

    // The test's name was a claim nothing checked: it pinned a literal and
    // never looked at zip, so zip could drift and only this comment would
    // notice. Build the same hostile entry as a zip and compare the two
    // messages, which is the property the `Other` passthrough exists for.
    let zip_archive = dir.path().join("evil.zip");
    {
        let f = std::fs::File::create(&zip_archive).unwrap();
        let mut w = ZipWriter::new(f);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        // The zip crate writes the name verbatim, so a traversal name survives.
        w.start_file("../escape.txt", opts).unwrap();
        w.write_all(b"pwned").unwrap();
        w.finish().unwrap();
    }
    let zip_err = extract_zip(&zip_archive, &dir.path().join("out_zip")).unwrap_err();
    assert_eq!(err.to_string(), zip_err.to_string());
}

#[test]
fn compressing_a_tree_into_a_missing_directory_reads_like_the_single_file_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = sample_tree(dir.path());
    let output = dir.path().join("does_not_exist").join("photos.7z");

    let err = compress_7z_dir(&root, &output, 3).unwrap_err();

    // `compress_7z_dir` has its own `SevenZWriter::create` call site, and no
    // test reached it: the whole-directory half of the backend could go back
    // to `e.to_string()` on its own and every other test here would still
    // pass. The message must match the single-file path exactly.
    assert!(
        matches!(err, CompressionError::Io(ref io) if io.kind() == std::io::ErrorKind::NotFound),
        "expected a NotFound Io, got {err:?}"
    );
    #[cfg(unix)]
    assert_eq!(
        err.to_string(),
        "IO error: No such file or directory (os error 2)"
    );
    assert!(
        !err.to_string().contains("photos.7z"),
        "the message leaks the output path: {err}"
    );
}

// -- crafted archives --
//
// The 7z writer can only produce well-formed archives, so the header-parsing
// failures below are unreachable without building the bytes by hand. Each one
// is gated behind a checksum, hence the CRC helper.

/// CRC-32 (IEEE), the checksum 7z uses for both of its headers.
///
/// Hand-rolled rather than pulled in as a dev-dependency: it is a dozen lines,
/// and the archives it seals are the only thing in the suite that needs it.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A 7z file whose 32-byte signature header is valid and points at `header`.
///
/// Layout: signature (6) + format version (2) + CRC of the start header (4) +
/// start header (20: next-header offset, size and CRC) = 32 bytes, then the
/// next header itself. `declared_crc` is what the file *claims* the next
/// header hashes to, so passing a wrong value is how the mismatch case is
/// reached; `None` seals it correctly and lets the reader go on to parse it.
fn archive_bytes(header: &[u8], declared_crc: Option<u32>) -> Vec<u8> {
    let mut start_header = Vec::with_capacity(20);
    start_header.extend_from_slice(&0u64.to_le_bytes()); // next header offset
    start_header.extend_from_slice(&(header.len() as u64).to_le_bytes());
    start_header.extend_from_slice(&declared_crc.unwrap_or_else(|| crc32(header)).to_le_bytes());

    let mut bytes = SEVENZ_SIGNATURE.to_vec();
    bytes.extend_from_slice(&[0, 4]); // format version 0.4
    bytes.extend_from_slice(&crc32(&start_header).to_le_bytes());
    bytes.extend_from_slice(&start_header);
    assert_eq!(bytes.len(), 32, "signature header must be 32 bytes");
    bytes.extend_from_slice(header);
    bytes
}

/// Write a crafted archive and hand back the failure extracting it produces.
fn extract_crafted(
    dir: &Path,
    name: &str,
    header: &[u8],
    declared_crc: Option<u32>,
) -> CompressionError {
    let archive = dir.join(name);
    std::fs::write(&archive, archive_bytes(header, declared_crc)).unwrap();
    extract_7z(&archive, &dir.join(format!("{name}.out"))).unwrap_err()
}

// Property ids from the 7z header grammar, named as the format spec names them.
const K_END: u8 = 0x00;
const K_HEADER: u8 = 0x01;
const K_ADDITIONAL_STREAMS_INFO: u8 = 0x03;
const K_MAIN_STREAMS_INFO: u8 = 0x04;
const K_UNPACK_INFO: u8 = 0x07;
const K_FOLDER: u8 = 0x0B;

#[test]
fn a_header_that_fails_its_own_checksum_is_described_in_words() {
    let dir = tempfile::TempDir::new().unwrap();

    // Two different checksums guard a 7z: the one over the start header (the
    // `a_corrupt_header_...` test above trips that one) and the one the start
    // header declares for the next header. They map to two different
    // sentences, so collapsing the arms into one would go unnoticed without
    // this: here the start header is valid and only its claim about the next
    // header is wrong.
    let err = extract_crafted(
        dir.path(),
        "lying.7z",
        &[K_HEADER, K_END],
        Some(0xDEAD_BEEF),
    );

    assert_eq!(
        err.to_string(),
        "Compression failed: the 7z archive is corrupt: its header failed the checksum"
    );
}

#[test]
fn a_malformed_header_section_is_described_in_words() {
    let dir = tempfile::TempDir::new().unwrap();

    // The five `BadTerminated*` variants share one arm and one sentence, and
    // nothing reached any of them. `0x42` is not a property id the parser
    // expects at either position, which is what the variant carries; the
    // sentence must not repeat it, because a raw property id tells a user
    // nothing.
    for (name, header) in [
        ("bent_header.7z", vec![K_HEADER, 0x42]),
        ("bent_streams.7z", vec![K_HEADER, K_MAIN_STREAMS_INFO, 0x42]),
    ] {
        let message = extract_crafted(dir.path(), name, &header, None).to_string();
        assert_eq!(
            message, "Compression failed: the 7z archive is corrupt: its header is malformed",
            "{name}"
        );
        assert!(
            !message.contains("66") && !message.contains("0x42"),
            "{name}: the message repeats the raw property id: {message}"
        );
    }
}

#[test]
fn an_archive_with_an_external_file_list_is_described_in_words() {
    let dir = tempfile::TempDir::new().unwrap();

    // Header, main streams info, unpack info, folder, zero folders, and then a
    // non-zero "external" flag: the byte that means the file list lives in a
    // stream of its own. It has its own hand-written sentence and its own arm,
    // neither of which anything exercised.
    let header = [
        K_HEADER,
        K_MAIN_STREAMS_INFO,
        K_UNPACK_INFO,
        K_FOLDER,
        0x00, // number of folders
        0x01, // external != 0
    ];
    let err = extract_crafted(dir.path(), "external.7z", &header, None);

    assert_eq!(
        err.to_string(),
        "Compression failed: this 7z archive keeps its file list in an external stream, \
         which is not supported"
    );
}

#[test]
fn a_sentence_the_dependency_wrote_travels_through_unchanged() {
    let dir = tempfile::TempDir::new().unwrap();

    // The `Other` arm passes its payload through, which is what keeps our own
    // traversal message intact. The other half of that arm is the dependency's
    // own prose, and this pins it: it must arrive as a sentence, not wrapped
    // in `Other(...)` the way `to_string()` used to render it.
    let err = extract_crafted(
        dir.path(),
        "extra_streams.7z",
        &[K_HEADER, K_ADDITIONAL_STREAMS_INFO],
        None,
    );

    assert_eq!(
        err.to_string(),
        "Compression failed: Additional streams unsupported"
    );
}

// -- the property, across every failure the suite can provoke --

/// Every 7z failure these tests know how to cause, paired with a description
/// of what a user did to cause it.
///
/// A per-variant test proves the mapping for the variant it names. This table
/// is the other half: the change's real promise is that *no* 7z failure can
/// show a `Debug` form or a path, and a promise about all of them needs a test
/// over all of them, so a newly mapped arm that stringifies is caught even
/// before anyone writes its own test.
///
/// Only failures that go through the 7z backend belong here. `walk_tree`'s
/// "Not a directory: <path>" is deliberately excluded: it never reaches the
/// dependency, and it names the path on purpose (it is echoing the argument
/// the caller just passed). Its own test pins it.
fn every_provokable_failure(dir: &Path) -> Vec<(&'static str, CompressionError)> {
    let src = source_file(dir);
    let tree = sample_tree(dir);
    let missing = dir.join("does_not_exist");
    let occupied = dir.join("already_a_file");
    std::fs::write(&occupied, b"in the way").unwrap();

    let not_an_archive = dir.join("plain.7z");
    std::fs::write(&not_an_archive, b"this is plain text, not an archive").unwrap();

    let from_the_future = dir.join("future.7z");
    let mut future_bytes = SEVENZ_SIGNATURE.to_vec();
    future_bytes.extend_from_slice(&[9, 4]);
    std::fs::write(&from_the_future, &future_bytes).unwrap();

    let bad_start_header = dir.join("bent_start.7z");
    let mut bent = SEVENZ_SIGNATURE.to_vec();
    bent.extend_from_slice(&[0, 0]);
    bent.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    bent.extend_from_slice(&[7u8; 20]);
    std::fs::write(&bad_start_header, &bent).unwrap();

    let half = dir.join("half.7z");
    compress_7z(&src, &half, "sample.txt", 3).unwrap();
    let whole = std::fs::read(&half).unwrap();
    std::fs::write(&half, &whole[..whole.len() / 2]).unwrap();

    let hostile = dir.join("hostile.7z");
    {
        let mut writer = SevenZWriter::create(&hostile).unwrap();
        let entry = SevenZArchiveEntry {
            name: "../escape.txt".to_string(),
            ..Default::default()
        };
        writer
            .push_archive_entry(entry, Some(b"pwned".as_slice()))
            .unwrap();
        writer.finish().unwrap();
    }

    // Not `mut` on a platform without /dev/full; see the push below.
    #[allow(unused_mut)]
    let mut failures = vec![
        (
            "compressing a file into a directory that is not there",
            compress_7z(&src, &missing.join("out.7z"), "sample.txt", 3).unwrap_err(),
        ),
        (
            "compressing a tree into a directory that is not there",
            compress_7z_dir(&tree, &missing.join("photos.7z"), 3).unwrap_err(),
        ),
        (
            "compressing a file that is not there",
            compress_7z(&missing, &dir.join("out.7z"), "sample.txt", 3).unwrap_err(),
        ),
        (
            "compressing onto a path that is already a directory",
            compress_7z(&src, &tree, "sample.txt", 3).unwrap_err(),
        ),
        (
            "extracting an archive that is not there",
            extract_7z(&missing.join("nope.7z"), &dir.join("out_missing")).unwrap_err(),
        ),
        (
            "extracting into a path that is already a file",
            extract_7z(&half, &occupied).unwrap_err(),
        ),
        (
            "extracting a file that is not an archive",
            extract_7z(&not_an_archive, &dir.join("out_plain")).unwrap_err(),
        ),
        (
            "extracting an archive from a newer format version",
            extract_7z(&from_the_future, &dir.join("out_future")).unwrap_err(),
        ),
        (
            "extracting an archive whose start header is corrupt",
            extract_7z(&bad_start_header, &dir.join("out_bent")).unwrap_err(),
        ),
        (
            "extracting an archive that lies about its header checksum",
            extract_crafted(dir, "lying.7z", &[K_HEADER, K_END], Some(0xDEAD_BEEF)),
        ),
        (
            "extracting an archive whose header section is malformed",
            extract_crafted(dir, "bent_header.7z", &[K_HEADER, 0x42], None),
        ),
        (
            "extracting an archive whose streams section is malformed",
            extract_crafted(
                dir,
                "bent_streams.7z",
                &[K_HEADER, K_MAIN_STREAMS_INFO, 0x42],
                None,
            ),
        ),
        (
            "extracting an archive with no header at all",
            extract_crafted(dir, "headerless.7z", &[0x42], None),
        ),
        (
            "extracting an archive with additional streams",
            extract_crafted(
                dir,
                "extra_streams.7z",
                &[K_HEADER, K_ADDITIONAL_STREAMS_INFO],
                None,
            ),
        ),
        (
            "extracting an archive with an external file list",
            extract_crafted(
                dir,
                "external.7z",
                &[
                    K_HEADER,
                    K_MAIN_STREAMS_INFO,
                    K_UNPACK_INFO,
                    K_FOLDER,
                    0x00,
                    0x01,
                ],
                None,
            ),
        ),
        (
            "extracting an archive that stops halfway",
            extract_7z(&half, &dir.join("out_half")).unwrap_err(),
        ),
        (
            "extracting an archive whose entry name escapes the output",
            extract_7z(&hostile, &dir.join("out_hostile")).unwrap_err(),
        ),
        (
            "extracting an archive with a flipped bit in an entry's data",
            extract_7z(&bitrotted_archive(dir), &dir.join("out_bitrot")).unwrap_err(),
        ),
    ];

    // The only failure that reaches `push_archive_entry`'s mapping, where the
    // packed stream fails to write. Provoking it needs a device that is
    // permanently out of space, and only Linux has one, so this entry is
    // skipped elsewhere rather than faked; CI runs on Linux, so the call site
    // is still covered there.
    #[cfg(target_os = "linux")]
    {
        let always_full = Path::new("/dev/full");
        if always_full.exists() {
            failures.push((
                "compressing onto a device with no space left",
                compress_7z(&src, always_full, "sample.txt", 3).unwrap_err(),
            ));
        }
    }

    failures
}

#[test]
fn no_seven_z_failure_shows_a_debug_dump_or_a_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let failures = every_provokable_failure(dir.path());
    assert!(
        failures.len() >= 17,
        "the table was gutted; this test only means something if it is broad"
    );

    // Everything the tests touch lives under the temp dir, so its own path is
    // the exact string a leaked path would contain. On macOS `TempDir` hands
    // back `/var/...` while the dependency would report the resolved
    // `/private/var/...`, so both spellings are checked.
    let here = dir.path().to_string_lossy().to_string();
    let resolved = dir
        .path()
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .to_string();

    for (what, err) in failures {
        let message = err.to_string();

        // `sevenz_rust2::Error` implements `Display` as `Debug`, so these are
        // the fingerprints of the dump the mapping replaced. `Io(` alone
        // catches the old `Failed("Io(Os { .. }, \"/path\")")` spelling.
        for fingerprint in [
            "Os {", "Error {", "Custom {", "kind:", "code:", "Error::", "Io(", "Other(", "\"",
        ] {
            assert!(
                !message.contains(fingerprint),
                "{what}: the message contains `{fingerprint}`, so it is a struct dump: {message}"
            );
        }

        assert!(
            !message.contains(&here) && !message.contains(&resolved),
            "{what}: the message names a path on this machine: {message}"
        );
        assert!(
            !message.contains(".7z"),
            "{what}: the message names a file, and the only file names here are ours: {message}"
        );
        assert!(
            !message.contains('\n'),
            "{what}: the message spans several lines: {message}"
        );

        // Only two renderings are acceptable, and neither may be a bare
        // prefix with nothing after it.
        let body = message
            .strip_prefix("IO error: ")
            .or_else(|| message.strip_prefix("Compression failed: "))
            .unwrap_or_else(|| panic!("{what}: unexpected rendering: {message}"));
        assert!(!body.trim().is_empty(), "{what}: empty message");
    }
}

#[test]
fn the_mapping_keeps_the_two_kinds_apart() {
    let dir = tempfile::TempDir::new().unwrap();
    let failures = every_provokable_failure(dir.path());

    // A mapping that answered `Failed` to everything would still satisfy the
    // "no dump" property above while throwing away the distinction the commit
    // introduced, so check both kinds actually occur, and that the ones that
    // land in `Io` really are IO problems rather than parse problems dressed
    // up as one.
    let (io, failed): (Vec<_>, Vec<_>) = failures
        .iter()
        .partition(|(_, err)| matches!(err, CompressionError::Io(_)));
    assert!(io.len() >= 7, "failures stopped reaching Io: {io:?}");
    assert!(
        failed.len() >= 10,
        "failures stopped reaching Failed: {failed:?}"
    );

    for (what, err) in &io {
        let CompressionError::Io(inner) = err else {
            unreachable!()
        };
        assert!(
            inner.raw_os_error().is_some() || inner.kind() == std::io::ErrorKind::UnexpectedEof,
            "{what}: `Io` should carry a real OS failure, got {inner:?}"
        );
    }

    // Known defect, pinned rather than fixed: a truncated archive is a corrupt
    // archive, but the dependency reports the short read as its `Io` variant
    // and the mapping cannot tell that apart from a failing disk, so the user
    // is told "IO error: failed to fill whole buffer". Layer 2 of issue #66
    // (a `#[source]` on `CompressionError`) is where that gets resolved.
    let truncated = io
        .iter()
        .find(|(what, _)| what.contains("stops halfway"))
        .expect("the truncated archive must still be in the table");
    assert_eq!(
        truncated.1.to_string(),
        "IO error: failed to fill whole buffer"
    );
}

#[test]
fn a_compression_failure_reads_the_same_whether_it_is_a_file_or_a_tree() {
    let dir = tempfile::TempDir::new().unwrap();
    let src = source_file(dir.path());
    let tree = sample_tree(dir.path());
    let occupied = dir.path().join("occupied");
    std::fs::create_dir(&occupied).unwrap();

    // Writing onto a directory is the compression-side failure that reaches
    // the dependency with a path in hand (`SevenZWriter::create` puts the
    // whole output path in its error). Both entry points must drop it, and
    // both must agree with each other.
    let file_err = compress_7z(&src, &occupied, "sample.txt", 3).unwrap_err();
    let tree_err = compress_7z_dir(&tree, &occupied, 3).unwrap_err();

    assert!(matches!(file_err, CompressionError::Io(_)), "{file_err:?}");
    assert!(matches!(tree_err, CompressionError::Io(_)), "{tree_err:?}");
    assert_eq!(file_err.to_string(), tree_err.to_string());
    for message in [file_err.to_string(), tree_err.to_string()] {
        assert!(
            !message.contains("occupied"),
            "the message names the path the user picked: {message}"
        );
    }
    #[cfg(unix)]
    assert_eq!(
        file_err.to_string(),
        "IO error: Is a directory (os error 21)"
    );
}

/// The `finish()` calls that lost their `map_err` are the one part of the
/// change no test here reaches, and this records why rather than pretending.
///
/// `SevenZWriter::finish` only fails when a write or a seek on the output
/// fails, and by then the file has been created, seeked and (for any entry
/// with content) written to successfully. Filling a real disk is the only way
/// in, and the obvious stand-in does not work: on `/dev/full` the dependency
/// panics inside `finish` (`writer.rs:388`, `position - SIGNATURE_HEADER_SIZE`
/// underflows because that device's `lseek` always answers 0) before it ever
/// reaches a failing write. So this test pins the successful path instead: an
/// archive of nothing but directories writes no entry bytes at all, which
/// makes `finish` the only thing that touches the output.
#[test]
fn finishing_an_archive_of_only_directories_writes_the_whole_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("a").join("b")).unwrap();
    let archive = dir.path().join("photos.7z");

    compress_7z_dir(&root, &archive, 3).unwrap();

    let written = std::fs::read(&archive).unwrap();
    assert!(
        written.starts_with(&SEVENZ_SIGNATURE),
        "finish never rewrote the signature header"
    );
    let out = dir.path().join("out");
    assert!(extract_7z(&archive, &out).unwrap().is_empty());
    assert!(out.join("photos/a/b").is_dir());
}

/// Writing an entry's packed stream is the second compression-side call into
/// the mapping, and a device that is permanently out of space is the only
/// portable way to make it fail. Linux only: macOS has no `/dev/full`, so this
/// runs in CI (Linux runners) and is skipped on a developer's Mac.
#[cfg(target_os = "linux")]
#[test]
fn a_write_that_fails_mid_entry_is_reported_as_an_io_error() {
    let always_full = Path::new("/dev/full");
    if !always_full.exists() {
        return;
    }
    let dir = tempfile::TempDir::new().unwrap();
    let src = dir.path().join("big.txt");
    std::fs::write(&src, vec![b'x'; 100_000]).unwrap();

    let err = compress_7z(&src, always_full, "big.txt", 3).unwrap_err();

    // The dependency wraps this one as `Io(err, "Encode entry:big.txt")`, so
    // `to_string()` used to render the whole struct, entry name included.
    assert!(
        matches!(err, CompressionError::Io(ref io) if io.raw_os_error() == Some(28)),
        "expected an ENOSPC Io, got {err:?}"
    );
    assert_eq!(
        err.to_string(),
        "IO error: No space left on device (os error 28)"
    );
    assert!(!err.to_string().contains("big.txt"), "{err}");
}
