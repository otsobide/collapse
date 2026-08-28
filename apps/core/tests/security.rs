//! Security tests: extraction must never write outside the output directory,
//! regardless of what entry names a malicious archive contains (path
//! traversal / "ZIP Slip"). Each test crafts an archive whose entry name tries
//! to escape, then asserts extraction is rejected AND nothing was written to
//! the parent directory.

use std::io::Write;
use std::path::Path;

use collapse_core::compression::{
    compress_7z_dir, compress_tar_dir, compress_zip_dir, extract_7z, extract_tar, extract_zip,
    NameRules, Substitutions,
};
use collapse_core::{extract, extract_with, Algorithm, CompressionError, ExtractOptions, Verify};
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

// -- entry names Windows accepts and reads as something other than a file --

/// Names whose colon Win32 reads as alternate data stream syntax rather than as
/// part of a file name: the first attaches a payload to a file an archive can
/// extract legitimately alongside it (`Zone.Identifier` among the streams it
/// could overwrite), the second attaches one to the output directory itself.
///
/// Both are ordinary, if odd, file names on Unix, so the assertions split by
/// platform: refusing them there would be a bug of its own.
const STREAM_SHAPED_NAMES: [&str; 2] = ["notes.txt:hidden", ":hidden"];

#[test]
fn no_format_writes_an_entry_name_with_a_colon_as_a_stream() {
    for (ext, build) in [
        ("zip", malicious_zip as fn(&Path, &str)),
        ("7z", malicious_7z),
        ("tar", malicious_tar),
    ] {
        for name in STREAM_SHAPED_NAMES {
            let dir = tempfile::TempDir::new().unwrap();
            let archive = dir.path().join(format!("stream.{ext}"));
            build(&archive, name);
            let out = dir.path().join("out");

            let result = extract(&archive, &out);

            if cfg!(windows) {
                // The host would take this name and write the bytes somewhere
                // no listing shows, so extraction stops and says which entry
                // and which character (issue #63). Nothing is written at all:
                // the naming pass runs before the output directory exists.
                let message = match result {
                    Err(err) => err.to_string(),
                    Ok(files) => panic!("{ext}/{name}: must be refused, wrote {files:?}"),
                };
                assert!(message.contains(name), "{ext}/{name}: {message}");
                assert!(message.contains(':'), "{ext}/{name}: {message}");
                assert!(
                    !out.exists() || std::fs::read_dir(&out).unwrap().next().is_none(),
                    "{ext}/{name}: something was written before the refusal"
                );
            } else {
                // One contained file, named exactly as the archive spells it.
                let files =
                    result.unwrap_or_else(|e| panic!("{ext}/{name}: a legal Unix name, got {e}"));
                assert_eq!(listing(files), vec![name.to_string()], "{ext}/{name}");
                assert!(out.join(name).is_file(), "{ext}/{name}");
            }
            // Whatever the platform made of it, nothing landed beside `out`,
            // and in particular no carrier file appeared next to the archive.
            assert!(
                !dir.path().join("notes.txt").exists(),
                "{ext}/{name}: a file was written outside the output directory"
            );
        }
    }
}

#[test]
fn a_replacement_cannot_carry_an_entry_out_of_the_output_directory() {
    // The answer the user gave used to be pushed inside a name that containment
    // had already cleared, so an unchecked replacement was a traversal by the
    // back door: `?` answered with `../..` would write above the output
    // directory with nothing left to notice it. `check_replacements` stood in
    // front of that.
    //
    // The hole is now closed a layer earlier and by construction rather than by
    // a check: an answer is never applied to anything, so it cannot reach a
    // path at all, and the entry that would have carried it is refused for its
    // own name. The hostile answers are still offered here, and must still
    // change nothing.
    for (ext, build) in [
        ("zip", malicious_zip as fn(&Path, &str)),
        ("7z", malicious_7z),
        ("tar", malicious_tar),
    ] {
        for replacement in ["../../escape", "/escape", r"..\..\escape"] {
            let dir = tempfile::TempDir::new().unwrap();
            let archive = dir.path().join(format!("q.{ext}"));
            build(&archive, "sub/a?b.txt");
            let out = dir.path().join("out");

            let options = ExtractOptions::new()
                .with_rules(NameRules::windows())
                .with_replacements(Substitutions::new().with('?', replacement));
            let result = extract_with(&archive, &out, &options);

            assert!(
                result.is_err(),
                "{ext}: {replacement:?} was accepted as a replacement"
            );
            assert!(
                !dir.path().join("escape").exists()
                    && !dir.path().join("..").join("escape").exists(),
                "{ext}: {replacement:?} wrote outside the output directory"
            );
        }
    }
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

// -- writing through something already in the output directory --

/// A symlink the extractor did not create, sitting in the output directory
/// before extraction begins, is the one traversal a name-only guard cannot see.
///
/// It was reachable and silent: with `link` a symlink in the output directory,
/// an archive holding `link/evil.txt` wrote straight through it and returned
/// `Ok`. tar was immune, because `unpack_in` resolves the directory it is about
/// to write into; zip and 7z joined a sanitized name and wrote. Extracting into
/// a directory that already holds a symlink is ordinary, and this predates the
/// naming work rather than arriving with it.
#[cfg(unix)]
#[test]
fn no_format_writes_through_a_symlink_already_in_the_output() {
    for ext in ["zip", "7z", "tar"] {
        let dir = tempfile::TempDir::new().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(&outside, out.join("link")).unwrap();

        let archive = dir.path().join(format!("a.{ext}"));
        match ext {
            "zip" => malicious_zip(&archive, "link/evil.txt"),
            "7z" => malicious_7z(&archive, "link/evil.txt"),
            _ => malicious_tar(&archive, "link/evil.txt"),
        }

        let escaped = outside.join("evil.txt");
        assert_contained(extract(&archive, &out), &escaped);
    }
}

/// The same guard, reached through the naming layer rather than around it: a
/// planned rename must not be able to land outside either.
#[cfg(unix)]
#[test]
fn an_entry_that_cannot_be_named_is_refused_before_any_symlink_is_followed() {
    // This entry used to take a second write path. Windows rules made the `?` a
    // question, the answer renamed it, and the renamed branch wrote through a
    // route that `unpack_in`'s own containment check never saw — so it needed
    // its own proof that a symlink already sitting in the output could not be
    // followed.
    //
    // That branch is unreachable now: the name is refused and nothing is
    // written by any path. The test stays because the thing it guards is the
    // file outside the output directory, and that assertion does not care which
    // of the two reasons kept it safe.
    for ext in ["zip", "7z", "tar"] {
        let dir = tempfile::TempDir::new().unwrap();
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(&outside, out.join("link")).unwrap();

        let archive = dir.path().join(format!("a.{ext}"));
        match ext {
            "zip" => malicious_zip(&archive, "link/ev?l.txt"),
            "7z" => malicious_7z(&archive, "link/ev?l.txt"),
            _ => malicious_tar(&archive, "link/ev?l.txt"),
        }

        let options = ExtractOptions::new().with_rules(NameRules::windows());
        let escaped = outside.join("ev?l.txt");
        assert_contained(extract_with(&archive, &out, &options), &escaped);
    }
}

/// `PathBuf::push` replaces what it holds when handed a path carrying a prefix,
/// and Windows reads `c:` at the head of a component as a drive. Prefixes are
/// parsed only at the head of a whole path, so a colon in a *later* component
/// gives `sanitize_entry_path` nothing to reject while still clearing the
/// buffer it is building, which would drop `docs` and leave a drive-relative
/// path resolving against the current directory of C:.
///
/// On Unix a colon is an ordinary character and this is simply a nested file,
/// which is the half this can assert here. Either way the bytes must land under
/// the output directory and nowhere else.
#[test]
fn a_colon_in_a_later_component_cannot_clear_the_path_being_built() {
    for ext in ["zip", "7z", "tar"] {
        let dir = tempfile::TempDir::new().unwrap();
        let out = dir.path().join("out");
        let archive = dir.path().join(format!("a.{ext}"));
        match ext {
            "zip" => malicious_zip(&archive, "docs/c:evil.txt"),
            "7z" => malicious_7z(&archive, "docs/c:evil.txt"),
            _ => malicious_tar(&archive, "docs/c:evil.txt"),
        }

        match extract(&archive, &out) {
            // Unix: written, and written inside.
            Ok(files) => {
                assert_eq!(listing(files), vec!["docs/c:evil.txt"], "{ext}");
                assert!(out.join("docs").join("c:evil.txt").exists(), "{ext}");
            }
            // Windows: refused, and nothing written anywhere.
            Err(_) => assert!(
                !out.join("c:evil.txt").exists(),
                "{ext}: a drive-relative path was written"
            ),
        }
    }
}

// -------------------------------------- writing over the archive being read --

/// Issue #96. An archive holding an entry with its own name, extracted into its
/// own directory, used to overwrite itself and report success.
///
/// Measured before this guard, on all three formats: `Ok`, "Extracted 1
/// file(s)", and the archive replaced by the 12 bytes it contained.
///
/// The asymmetry is what made it indefensible rather than merely unfortunate:
/// compression has always refused to write an archive over its own source, and
/// refuses it even with `--force`, while extraction had no equivalent at all.
#[test]
fn no_format_writes_an_entry_over_the_archive_it_is_reading() {
    for ext in ["zip", "7z", "tar"] {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = dir.path().join(format!("victim.{ext}"));

        // An archive whose single entry is named after the archive itself.
        let entry = format!("victim.{ext}");
        match ext {
            "zip" => malicious_zip(&archive, &entry),
            "7z" => malicious_7z(&archive, &entry),
            _ => malicious_tar(&archive, &entry),
        }
        let before = std::fs::read(&archive).unwrap();

        let err = extract(&archive, dir.path())
            .expect_err("extracting into its own directory must be refused");

        assert!(
            err.to_string().contains("over the archive itself"),
            "{ext}: {err}"
        );
        assert_eq!(
            std::fs::read(&archive).unwrap(),
            before,
            "{ext}: the archive was modified"
        );
    }
}

/// The same guard, reached through a second name for the same file.
///
/// A hardlink never resolves to the same path, so a string comparison would
/// wave this through. That is not hypothetical: it is exactly how `--force`
/// used to be able to overwrite its own source on the compression side, which
/// is why `paths::same_file` exists and why this uses it.
#[cfg(unix)]
#[test]
fn a_hardlink_to_the_archive_is_not_a_way_around_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("real.zip");
    malicious_zip(&archive, "alias.zip");

    std::fs::hard_link(&archive, dir.path().join("alias.zip")).unwrap();
    let before = std::fs::read(&archive).unwrap();

    let err = extract(&archive, dir.path()).expect_err("a second name is still the same file");
    assert!(err.to_string().contains("over the archive itself"), "{err}");
    assert_eq!(std::fs::read(&archive).unwrap(), before);
}

/// The guard must not cost anyone an extraction that was never dangerous.
///
/// Same archive, same entry name, a different output directory: nothing to
/// refuse. Without this the fix could be "refuse everything that looks vaguely
/// like the archive" and still pass the two tests above.
#[test]
fn the_same_archive_extracts_normally_somewhere_else() {
    for ext in ["zip", "7z", "tar"] {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = dir.path().join(format!("victim.{ext}"));
        let entry = format!("victim.{ext}");
        match ext {
            "zip" => malicious_zip(&archive, &entry),
            "7z" => malicious_7z(&archive, &entry),
            _ => malicious_tar(&archive, &entry),
        }

        let out = dir.path().join("elsewhere");
        let files = extract(&archive, &out).unwrap_or_else(|e| panic!("{ext}: {e}"));
        assert_eq!(listing(files), vec![entry.clone()], "{ext}");
        assert!(out.join(&entry).exists(), "{ext}");
    }
}

/// A renamed entry must be checked at the name it will actually be written
/// under, not the one the archive spells.
///
/// The archive is `v_.zip` and its entry is `v?.zip`, which is nothing special
/// on Unix; under Windows rules the `?` is answered with `_`, so the entry
/// lands exactly on the archive. Checking the archive's own spelling would
/// miss it.
#[test]
fn an_entry_that_cannot_be_named_never_reaches_the_archive_it_would_overwrite() {
    // This used to be the case for following the *planned* name: `v?.zip`
    // answered with `_` became `v_.zip`, which is the archive being read, and
    // the overwrite guard had to see that even though the archive's own name
    // matched no entry.
    //
    // Nothing is renamed any more, so that arrangement cannot be built. What
    // is left is the ordering, and it is worth pinning: the naming refusal
    // comes first, so the archive is never opened for writing at all. The
    // guarantee a person cares about is unchanged and is the last assertion —
    // the archive is still there, byte for byte.
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("v_.zip");
    malicious_zip(&archive, "v?.zip");
    let before = std::fs::read(&archive).unwrap();

    let options = ExtractOptions::new().with_rules(NameRules::windows());

    let err = extract_with(&archive, dir.path(), &options)
        .expect_err("a name this host cannot write is refused");
    assert!(
        matches!(err, CompressionError::Name(_)),
        "refused for the naming reason, before the overwrite check: {err}"
    );
    assert_eq!(std::fs::read(&archive).unwrap(), before);
}
