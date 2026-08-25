//! Security tests: extraction must never write outside the output directory,
//! regardless of what entry names a malicious archive contains (path
//! traversal / "ZIP Slip"). Each test crafts an archive whose entry name tries
//! to escape, then asserts extraction is rejected AND nothing was written to
//! the parent directory.

use std::io::Write;
use std::path::Path;

use collapse_core::compression::{
    compress_7z_dir, compress_tar_dir, compress_zip_dir, extract_7z, extract_tar, extract_zip,
};
use collapse_core::{extract, Algorithm, Verify};
use sevenz_rust2::{SevenZArchiveEntry, SevenZWriter};
use tar::{Builder, EntryType, Header};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Normalize and sort an extracted listing so the expectations read the same
/// on a platform whose path separator is not `/`.
///
/// The entry names inside the archive are forward-slash separated everywhere;
/// the extractors rebuild each one as a `PathBuf` before stringifying it, so
/// the listing (and only the listing) comes back with `\` on Windows.
fn listing(paths: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = paths.iter().map(|p| p.replace('\\', "/")).collect();
    out.sort();
    out
}

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

// -- entry names that only Windows reads as an escape --
//
// Everything above spells traversal the Unix way. These four are the spellings
// a Windows machine resolves and a Unix machine does not, added rather than
// substituted so both platforms keep the coverage they had. What is asserted
// is deliberately not the same sentence on both:
//
// * Windows: `\` is a path separator and a drive or share is a path root, so
//   each name genuinely points out of the output directory and the backend
//   must refuse it (or, for tar, strip it back inside; see below).
// * Unix: none of these strings contains a separator, so each is one ordinary
//   (if ugly) file name that belongs *inside* the output directory. Refusing
//   them there would be a bug of its own, and asserting the contained result
//   is what stops these cases from silently testing nothing off Windows.
//
// The one sentence that holds everywhere is containment, which every case
// checks last: whatever the platform made of the name, the parent of the
// output directory gained nothing.

/// Traversal spelled with backslashes: `..` plus a name once Windows splits
/// them, one file name on Unix.
const BACKSLASH_TRAVERSAL_NAMES: [&str; 2] = [r"..\escape.txt", r"sub\..\..\escape.txt"];

/// Roots Unix has no notion of: a drive-relative name (which Windows resolves
/// against the current directory *of that drive*, so it is not even anchored
/// at the drive root) and a UNC share.
const WINDOWS_ROOTED_NAMES: [&str; 2] = [r"C:escape.txt", r"\\server\share\escape.txt"];

/// Both groups at once, for the backends whose guard refuses every one of
/// them (only tar splits them; see `tar_contains_windows_shaped_...`).
fn windows_shaped_names() -> impl Iterator<Item = &'static str> {
    BACKSLASH_TRAVERSAL_NAMES
        .into_iter()
        .chain(WINDOWS_ROOTED_NAMES)
}

/// Assert the attempt wrote nothing beside the output directory: after it, the
/// parent holds the archive and `out`, and nothing else. Every extractor
/// creates `out` before reading the first entry, so it is expected even when
/// the entry was refused.
fn assert_only_the_output_dir(parent: &Path, archive: &Path, out: &Path, what: &str) {
    let mut found: Vec<String> = std::fs::read_dir(parent)
        .expect("read the parent of the output dir")
        .map(|entry| {
            entry
                .expect("read a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    found.sort();
    let mut expected = vec![
        archive.file_name().unwrap().to_string_lossy().into_owned(),
        out.file_name().unwrap().to_string_lossy().into_owned(),
    ];
    expected.sort();
    assert_eq!(
        found, expected,
        "{what}: something was written beside the output directory"
    );
}

/// The Unix reading of a Windows-shaped name: one entry, extracted under that
/// exact name, inside the output directory.
fn assert_extracted_as_one_contained_file(
    result: Result<Vec<String>, impl std::fmt::Debug>,
    out: &Path,
    name: &str,
) {
    let files = result.expect("a name with no separator on this platform must extract");
    assert_eq!(
        files,
        vec![name.to_string()],
        "{name}: expected exactly this entry, reported as written"
    );
    assert!(
        out.join(name).is_file(),
        "{name}: the entry did not land inside the output directory"
    );
}

#[test]
fn zip_rejects_windows_shaped_traversal_and_rooted_names() {
    for name in windows_shaped_names() {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = dir.path().join("evil.zip");
        malicious_zip(&archive, name);

        let out = dir.path().join("out");
        let result = extract_zip(&archive, &out);
        if cfg!(windows) {
            assert!(
                result.is_err(),
                "{name}: Windows resolves this out of the output dir, got {result:?}"
            );
        } else {
            assert_extracted_as_one_contained_file(result, &out, name);
        }
        assert_only_the_output_dir(dir.path(), &archive, &out, name);
    }
}

#[test]
fn sevenz_rejects_windows_shaped_traversal_and_rooted_names() {
    for name in windows_shaped_names() {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = dir.path().join("evil.7z");
        malicious_7z(&archive, name);

        let out = dir.path().join("out");
        let result = extract_7z(&archive, &out);
        if cfg!(windows) {
            assert!(
                result.is_err(),
                "{name}: Windows resolves this out of the output dir, got {result:?}"
            );
        } else {
            assert_extracted_as_one_contained_file(result, &out, name);
        }
        assert_only_the_output_dir(dir.path(), &archive, &out, name);
    }
}

#[test]
fn tar_contains_windows_shaped_traversal_and_rooted_names() {
    // tar's guard is the tar crate's `unpack_in` rather than
    // `sanitize_entry_path`, and it treats the two groups differently: `..` is
    // refused, while a prefix or a root is *stripped* and the entry lands
    // inside the output dir. That is the same behaviour
    // `extract_tar_reports_absolute_entry_names_as_written` already pins for
    // `/abs.txt`, so the rooted group is asserted as contained, not refused.
    for name in BACKSLASH_TRAVERSAL_NAMES {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = dir.path().join("evil.tar");
        malicious_tar(&archive, name);

        let out = dir.path().join("out");
        let result = extract_tar(&archive, &out);
        if cfg!(windows) {
            assert!(
                result.is_err(),
                "{name}: Windows reads this as `..`, so it must be refused, got {result:?}"
            );
        } else {
            assert_extracted_as_one_contained_file(result, &out, name);
        }
        assert_only_the_output_dir(dir.path(), &archive, &out, name);
    }

    for name in WINDOWS_ROOTED_NAMES {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = dir.path().join("evil.tar");
        malicious_tar(&archive, name);

        let out = dir.path().join("out");
        let result = extract_tar(&archive, &out);
        if cfg!(windows) {
            let files = result.expect("a rooted name is stripped, not refused");
            assert_eq!(
                listing(files),
                vec!["escape.txt"],
                "{name}: the drive or share must be stripped off"
            );
            assert!(
                out.join("escape.txt").is_file(),
                "{name}: the stripped entry did not land inside the output directory"
            );
        } else {
            assert_extracted_as_one_contained_file(result, &out, name);
        }
        assert_only_the_output_dir(dir.path(), &archive, &out, name);
    }
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
        collapse_core::compress(
            &src,
            &archive,
            "nested/dir/input.txt",
            algo,
            1,
            Verify::Index,
        )
        .unwrap();

        let out = dir.path().join(format!("out_{}", algo.extension()));
        let files = extract(&archive, &out).unwrap();
        assert_eq!(listing(files), vec!["nested/dir/input.txt"], "{algo}");
        assert_eq!(
            std::fs::read(out.join("nested/dir/input.txt")).unwrap(),
            b"safe"
        );
    }
}

// ======================================================================
// Directory / multi-entry archives
// ======================================================================

// -- extraction: a *directory* entry whose name traverses --

#[test]
fn zip_rejects_directory_entry_traversal() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.zip");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut w = ZipWriter::new(f);
        w.add_directory("../evildir", SimpleFileOptions::default())
            .unwrap();
        w.finish().unwrap();
    }
    let out = dir.path().join("out");
    assert_contained(extract_zip(&archive, &out), &dir.path().join("evildir"));
}

#[test]
fn sevenz_rejects_directory_entry_traversal() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.7z");
    {
        let mut w = SevenZWriter::create(&archive).unwrap();
        let mut e = SevenZArchiveEntry::default();
        e.name = "../evildir".to_string();
        e.is_directory = true;
        e.has_stream = false;
        w.push_archive_entry::<&[u8]>(e, None).unwrap();
        w.finish().unwrap();
    }
    let out = dir.path().join("out");
    assert_contained(extract_7z(&archive, &out), &dir.path().join("evildir"));
}

// -- extraction: symlink-based escape (create a link out of the tree, then
//    write a file through it) --

/// A symlink out of the tree followed by a file "through" it must not let the
/// file escape. We neutralize the symlink (never create it), so the follow-up
/// file lands safely inside the output dir instead of in the parent.
#[test]
fn tar_symlink_write_through_does_not_escape() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("evil.tar");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut builder = Builder::new(f);
        // "sneak" -> ".." (the parent of the output dir)
        let mut link = Header::new_gnu();
        link.set_entry_type(EntryType::Symlink);
        link.set_size(0);
        link.set_mode(0o777);
        builder.append_link(&mut link, "sneak", "..").unwrap();
        // a file written through the link would land in the parent
        let content = b"pwned";
        let mut file = Header::new_gnu();
        file.set_entry_type(EntryType::Regular);
        file.set_size(content.len() as u64);
        file.set_mode(0o644);
        builder
            .append_data(&mut file, "sneak/pwned.txt", &content[..])
            .unwrap();
        builder.finish().unwrap();
    }
    let out = dir.path().join("out");
    extract_tar(&archive, &out).unwrap();

    // Nothing landed in the parent of out/, and no outbound symlink was created.
    assert!(
        !dir.path().join("pwned.txt").exists(),
        "a file escaped the output directory"
    );
    assert!(
        out.join("sneak")
            .symlink_metadata()
            .map(|m| !m.file_type().is_symlink())
            .unwrap_or(true),
        "an outbound symlink was materialized"
    );
}

/// A zip symlink entry must be materialized as a regular file, never as an
/// actual symlink — otherwise a later entry could be written through it.
#[test]
fn zip_symlink_entry_is_not_materialized_as_symlink() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("link.zip");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut w = ZipWriter::new(f);
        w.add_symlink("link", "/etc", SimpleFileOptions::default())
            .unwrap();
        w.finish().unwrap();
    }
    let out = dir.path().join("out");
    extract_zip(&archive, &out).unwrap();
    let meta = out.join("link").symlink_metadata().unwrap();
    assert!(
        !meta.file_type().is_symlink(),
        "a symlink entry was materialized as a real symlink"
    );
}

/// A tar symlink entry must not be materialized as a symlink on disk; the
/// regular entries around it still extract. Keeps the "no links created"
/// guarantee uniform with zip/7z.
#[test]
fn tar_symlink_entry_is_not_materialized() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("mixed.tar");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut builder = Builder::new(f);
        let mut link = Header::new_gnu();
        link.set_entry_type(EntryType::Symlink);
        link.set_size(0);
        link.set_mode(0o777);
        builder
            .append_link(&mut link, "evil", "/etc/passwd")
            .unwrap();
        let content = b"ok";
        let mut file = Header::new_gnu();
        file.set_entry_type(EntryType::Regular);
        file.set_size(content.len() as u64);
        file.set_mode(0o644);
        builder
            .append_data(&mut file, "ok.txt", &content[..])
            .unwrap();
        builder.finish().unwrap();
    }
    let out = dir.path().join("out");
    let files = extract_tar(&archive, &out).unwrap();
    assert_eq!(listing(files), vec!["ok.txt"]);
    assert!(
        out.join("evil").symlink_metadata().is_err(),
        "a tar symlink entry was materialized on disk"
    );
}

// -- extraction: a malicious entry AFTER benign ones must still abort the
//    whole extraction with nothing escaping --

#[test]
fn zip_rejects_malicious_entry_after_benign_ones() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("mixed.zip");
    {
        let f = std::fs::File::create(&archive).unwrap();
        let mut w = ZipWriter::new(f);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        w.start_file("ok.txt", opts).unwrap();
        w.write_all(b"fine").unwrap();
        w.start_file("../escape.txt", opts).unwrap();
        w.write_all(b"pwned").unwrap();
        w.finish().unwrap();
    }
    let out = dir.path().join("out");
    assert_contained(extract_zip(&archive, &out), &dir.path().join("escape.txt"));
}

#[test]
fn sevenz_rejects_malicious_entry_after_benign_ones() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("mixed.7z");
    {
        let mut w = SevenZWriter::create(&archive).unwrap();
        let mut ok = SevenZArchiveEntry::default();
        ok.name = "ok.txt".to_string();
        w.push_archive_entry(ok, Some(b"fine".as_slice())).unwrap();
        let mut evil = SevenZArchiveEntry::default();
        evil.name = "../escape.txt".to_string();
        w.push_archive_entry(evil, Some(b"pwned".as_slice()))
            .unwrap();
        w.finish().unwrap();
    }
    let out = dir.path().join("out");
    assert_contained(extract_7z(&archive, &out), &dir.path().join("escape.txt"));
}

// -- compression: archiving a directory must never follow a symlink out of
//    the tree (all three formats skip symlinks) --

#[cfg(unix)]
#[test]
fn compress_dir_skips_symlinks_for_every_format() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::TempDir::new().unwrap();
    let secret = dir.path().join("secret.txt");
    std::fs::write(&secret, b"TOP-SECRET").unwrap();

    let root = dir.path().join("photos");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("ok.txt"), b"ok").unwrap();
    symlink(&secret, root.join("leak.txt")).unwrap();

    // (extension, compress result)
    compress_tar_dir(&root, &dir.path().join("a.tar")).unwrap();
    compress_zip_dir(&root, &dir.path().join("a.zip"), 1).unwrap();
    compress_7z_dir(&root, &dir.path().join("a.7z"), 1).unwrap();

    for ext in ["tar", "zip", "7z"] {
        let archive = dir.path().join(format!("a.{ext}"));
        let out = dir.path().join(format!("out_{ext}"));
        let files = extract(&archive, &out).unwrap();
        assert_eq!(
            listing(files),
            vec!["photos/ok.txt"],
            "{ext}: unexpected entries"
        );
        assert!(
            out.join("photos/leak.txt").symlink_metadata().is_err(),
            "{ext}: the symlink leaked into the archive"
        );
    }
}
