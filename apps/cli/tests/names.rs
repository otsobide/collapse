//! What the CLI does with an archive holding entry names this machine cannot
//! write as files (issues #63 and #64).
//!
//! A command line cannot stop and ask, so it refuses instead, and the refusal
//! is the whole feature here: it has to name the entries, say what is wrong
//! with each one, and point at something that would get the user their files.
//!
//! Two kinds of test, and the split is deliberate. The ones that judge the
//! *message* build a `NameReport` against `NameRules::windows()`, so they read
//! the Windows refusal on the Mac this is written on and on the Linux CI leg;
//! the report is data, and asking for another platform's rules is a function
//! call. The ones that drive `run` end to end use the host's rules and a NUL
//! byte, which is the one character no filesystem anywhere will take, so they
//! assert the same thing on every platform. A test reachable only under
//! `#[cfg(windows)]` is a test this repository never runs.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use clap::Parser;
use collapse_cli::{run, Cli, CliError};
use collapse_core::{NameReport, NameRules};

fn run_err(args: &[&str]) -> CliError {
    run(Cli::try_parse_from(args).expect("args should parse")).expect_err("command should fail")
}

/// Write a zip holding exactly these entries, names included, with no opinion
/// about them.
///
/// Everything in core takes its entry names from files that exist, so nothing
/// there can spell a name the local filesystem refuses. Reaching the refusal
/// path at all means writing the archive by hand.
fn zip_named(path: &Path, entries: &[(&str, &[u8])]) {
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, body) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap();
}

/// The refusal a Windows machine would print for this listing, read from a
/// machine that is not one.
fn windows_refusal(archive: &str, names: &[&str]) -> String {
    CliError::UnwritableEntries {
        archive: PathBuf::from(archive),
        report: NameReport::of(names, NameRules::windows()),
    }
    .to_string()
}

// ------------------------------------------------------------- the refusal --

/// Every offending entry is named, and every entry is told what is wrong with
/// it. Break any arm of the explanation and one of these disappears.
#[test]
fn the_refusal_names_every_bad_entry_and_says_what_is_wrong_with_each() {
    let message = windows_refusal(
        "report.zip",
        &[
            "summary.txt",
            "what?.txt",
            "notes.txt.",
            "CON",
            "logs/a:b.txt",
        ],
    );

    assert!(
        message.starts_with("cannot extract report.zip: 4 entry names cannot be written"),
        "the archive and the count come first: {message}"
    );
    for named in ["what?.txt", "notes.txt.", "CON", "logs/a:b.txt"] {
        assert!(
            message.contains(named),
            "{named:?} is unwritable and must be named: {message}"
        );
    }
    assert!(
        !message.contains("summary.txt"),
        "an entry that is perfectly writable is not the user's problem: {message}"
    );

    // One reason per kind of fault, each phrased for the person reading it.
    assert!(
        message.contains("'?' cannot appear in a file name here"),
        "the rejected character says so: {message}"
    );
    assert!(
        message.contains("':' is not read as part of a name here"),
        "the reinterpreted character is a different fault and reads differently: {message}"
    );
    assert!(
        message.contains("attached to another file as hidden data"),
        "and the difference that matters is that it fails silently: {message}"
    );
    assert!(
        message.contains(r#"the name ends in ".", which this system does not keep"#),
        "the trailing dot says which characters go: {message}"
    );
    assert!(
        message.contains(r#""CON" names a device"#),
        "the device name says it is a device: {message}"
    );
}

/// Nothing is half-written, and the user is told where to go. Drop the pointer
/// and the message becomes a dead end.
#[test]
fn the_refusal_says_nothing_was_written_and_where_to_go_next() {
    let message = windows_refusal("report.zip", &["what?.txt"]);

    assert!(
        message.contains("Nothing was extracted."),
        "the state of the output directory is the first thing a user wonders about: {message}"
    );
    assert!(
        message.contains("a replacement for '?' (1 entry)"),
        "what would unblock it, in the singular: {message}"
    );
    assert!(
        message.contains("desktop app"),
        "the pointer at the one front end that can ask: {message}"
    );
}

/// A UI puts one field per character, not one per file, and the message counts
/// the same way: the user needs to know how much of the archive one answer
/// buys.
#[test]
fn the_refusal_counts_the_entries_each_character_holds_up() {
    let message = windows_refusal(
        "report.zip",
        &["what?.txt", "why?.txt", "logs/a:b.txt", "clean.txt"],
    );

    assert!(
        message.contains("replacements for '?' (2 entries) and ':' (1 entry)"),
        "both characters, both counts, and a plural that matches: {message}"
    );
}

/// A trailing dot and a device name used to be adjusted rather than refused, on
/// the reasoning that they had one correct answer and nobody to ask. Core
/// adjusts nothing now, so `run_extract` refuses on the whole report instead of
/// on its `characters` half, and these three entries are exactly what that
/// widening is about: an archive whose only fault is a file called `aux.log` is
/// refused on Windows rather than arriving as `aux_.log`.
///
/// Judged through `NameReport` rather than by running the command, because the
/// faults are Windows-only and this suite runs on Linux.
#[test]
fn a_name_that_needs_no_answer_is_refused_like_any_other() {
    let windows = NameRules::windows();
    let report = NameReport::of(&["notes.txt.", "CON.txt", "aux.log", "fine.txt"], windows);

    // The half the CLI used to consult is empty: not one of these is a question
    // anybody could be asked. Under the old rule the archive extracted.
    assert!(report.characters.is_empty(), "none of these is a question");
    // The half it consults now is not, which is the whole of the change.
    assert!(!report.is_empty(), "the archive is refused all the same");

    let named: Vec<&str> = report.entries.iter().map(|e| e.entry.as_str()).collect();
    assert_eq!(
        named,
        ["notes.txt.", "CON.txt", "aux.log"],
        "every offending entry, and the writable one absent"
    );
}

/// The two kinds of fault now arrive at the same place, which is what stops the
/// refusal being half a list. A report mixing one of each names both, so a
/// user is not refused for `what?.txt`, fixed nothing, and then refused again
/// for `notes.txt.`.
#[test]
fn a_report_mixing_both_kinds_of_fault_names_all_of_them() {
    let windows = NameRules::windows();
    let report = NameReport::of(&["what?.txt", "notes.txt."], windows);

    assert!(!report.is_empty());
    let named: Vec<&str> = report.entries.iter().map(|e| e.entry.as_str()).collect();
    assert_eq!(named, ["what?.txt", "notes.txt."]);

    let message = windows_refusal("mixed.zip", &["what?.txt", "notes.txt."]);
    assert!(message.contains("what?.txt"), "{message}");
    assert!(message.contains("notes.txt."), "{message}");
}

// ------------------------------------------------------ end to end, on any host --

/// The refusal happens before anything is written, so the entries an archive
/// could have delivered are not left sitting in the output directory next to a
/// failure.
///
/// A NUL byte is the character every filesystem here refuses (`NameRules::unix`
/// lists it and nothing else, and Windows refuses every control character), so
/// this exercises the host path wherever it runs.
#[test]
fn extract_refuses_an_archive_this_host_cannot_name_and_writes_nothing() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("mixed.zip");
    zip_named(
        &archive,
        &[("keep.txt", b"kept"), ("no\u{0}pe.txt", b"impossible")],
    );
    let out = dir.path().join("out");

    let error = run_err(&[
        "collapse",
        "extract",
        archive.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ]);

    // Not the engine's own refusal (which names one entry and one character):
    // the CLI's, which surveys the whole listing first.
    assert!(
        matches!(&error, CliError::UnwritableEntries { report, .. } if report.entries.len() == 1),
        "expected the CLI's up-front refusal, got {error:?}"
    );
    let message = error.to_string();
    assert!(
        message.contains(r"no\0pe.txt"),
        "the entry is named, escaped so an unprintable character is visible: {message}"
    );

    assert!(
        !out.join("keep.txt").exists(),
        "the writable entry must not have been extracted: refusing halfway would leave the user \
         with a directory nobody can tell apart from a complete one"
    );
}

// ------------------------------------------------- an ordinary write failure --

/// Piece 1 of #64 at the surface a user sees: when a write fails for a reason
/// that has nothing to do with names (here a parent that is already a file,
/// which is how a read-only directory or a full disk arrives too), the message
/// says which entry it was.
///
/// Before this, everything above was `error: IO error: File exists (os error
/// 17)`: no entry, no archive, nothing to act on.
#[test]
fn a_failing_entry_names_itself_in_the_error() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("conflict.zip");
    // `x` is written as a file, and then `x/y.txt` needs `x` to be a directory.
    zip_named(&archive, &[("x", b"a file"), ("x/y.txt", b"under it")]);
    let out = dir.path().join("out");

    let message = run_err(&[
        "collapse",
        "extract",
        archive.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
    ])
    .to_string();

    assert!(
        message.contains("x/y.txt"),
        "the entry that could not be written is named: {message}"
    );
    // See the twin in apps/core/tests/names.rs: on Windows the message is built
    // from a canonicalized root, so compare against what was actually resolved.
    let resolved = out.canonicalize().unwrap_or_else(|_| out.clone());
    assert!(
        message.contains(&resolved.display().to_string()),
        "and where it was going: {message}"
    );
}
