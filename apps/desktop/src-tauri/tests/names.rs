//! Entry names this computer cannot write, at the desktop's own boundary.
//!
//! This file used to drive a conversation: `unwritable_names` reported what an
//! archive held, a dialog asked the user for a character to put in its place,
//! and `extract_archive` took the answers. None of that exists any more —
//! extraction refuses a name it cannot write rather than negotiating one — so
//! what is left to guard is narrower and, being narrower, worth stating
//! exactly.
//!
//! **The refusal has to reach the user with its reason attached.** The command
//! surface is the last place that can be lost: core builds a message naming the
//! entry, the component and the fault, and if this layer flattened it to
//! "extraction failed" the user would be told an archive is broken when it is
//! merely foreign. On Unix the whole alphabet of such names is one character,
//! the NUL byte, which is why the cases below are `#[cfg(unix)]` — their
//! fixture is Unix-specific, not their subject. `apps/core/tests/names.rs`
//! covers the Windows rules from any machine, since the rules are data.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use collapse_desktop::commands::extract_archive;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

fn zip_with(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let archive = dir.join("input.zip");
    let file = fs::File::create(&archive).unwrap();
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, content) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap();
    archive
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// An entry name this machine genuinely refuses. The NUL byte cannot cross the
/// libc boundary, so no Unix filesystem can hold one.
#[cfg(unix)]
const UNWRITABLE_HERE: &str = "bad\u{0}name.txt";

#[test]
fn an_ordinary_archive_extracts_and_lists_what_it_wrote() {
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[("notes.txt", b"hello")]);
    let out = dir.path().join("out");

    let files = extract_archive(text(&archive), text(&out)).expect("an ordinary archive extracts");

    assert_eq!(files, ["notes.txt"]);
    assert_eq!(fs::read(out.join("notes.txt")).unwrap(), b"hello");
}

#[cfg(unix)]
#[test]
fn a_name_this_computer_cannot_write_is_refused_with_its_reason() {
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[(UNWRITABLE_HERE, b"hello")]);
    let out = dir.path().join("out");

    let message = extract_archive(text(&archive), text(&out)).expect_err("the name is refused");

    assert!(
        message.contains("cannot be written on this system"),
        "{message}"
    );
    assert!(message.contains("bad"), "the entry names itself: {message}");
    assert!(
        message.contains("refuses in a file name"),
        "and the reason travels, which matters most here: a NUL prints as \
         nothing, so an entry named without its fault looks fine: {message}"
    );
}

#[cfg(unix)]
#[test]
fn the_writable_entry_beside_it_is_not_written_either() {
    // All of the archive or none of it, seen from the surface the app calls.
    // `fine.txt` is ordinary and must still not appear: a user who is told an
    // extraction failed should not find half of one on disk.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(
        dir.path(),
        &[("fine.txt", b"hello"), (UNWRITABLE_HERE, b"hello")],
    );
    let out = dir.path().join("out");

    extract_archive(text(&archive), text(&out)).expect_err("the archive is refused");

    assert!(!out.exists(), "nothing may be written before the refusal");
}

#[test]
fn a_missing_archive_is_reported_by_path_rather_than_by_the_extractor() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("nowhere.zip");

    let message = extract_archive(text(&missing), text(&dir.path().join("out")))
        .expect_err("a missing archive fails");

    assert!(message.contains("Not found"), "{message}");
}
