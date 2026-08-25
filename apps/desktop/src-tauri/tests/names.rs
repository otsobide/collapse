//! The naming exchange behind the extract dialog: `unwritable_names`, the
//! answers `extract_archive` takes, and the JSON both of them cross the IPC
//! boundary as.
//!
//! Two halves, and the split is deliberate.
//!
//! The **pure** half runs the Windows ruleset from wherever the suite runs, so
//! the dialog's data (and the exact JSON the webview parses) is verified on a
//! Mac and on the Linux CI leg, not only on the Windows leg that runs on the
//! release path. That is the whole reason `NameRules` is data rather than
//! `#[cfg]`, and this file would be worth very little without it.
//!
//! The **host** half needs an entry name this machine genuinely refuses, which
//! on Unix means exactly one character: the NUL byte, which cannot cross the
//! libc boundary. Those cases are `#[cfg(unix)]` because their fixture is, not
//! because the behaviour is: the same code path runs on Windows for a much
//! larger alphabet, and core's `tests/names.rs` covers that alphabet with the
//! Windows rules from anywhere.
//!
//! The fixtures are crafted with the `zip` crate directly. Nothing that
//! compresses a real directory could produce them, because the filesystem
//! would have refused to hold the file in the first place.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use collapse_core::{NameReport, NameRules};
use collapse_desktop::commands::{extract_archive, unwritable_names, Extraction};
use collapse_desktop::names::{substitutions_from, NameInspection};
use serde_json::json;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

// ---------------------------------------------------------------- fixtures --

/// A zip whose entries are named exactly as given, however hostile the name.
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

fn inspect(archive: &Path) -> Result<NameInspection, String> {
    unwritable_names(text(archive))
}

/// Extract with the answers a user would have typed into the dialog.
fn extract_answering(
    archive: &Path,
    into: &Path,
    answers: &[(&str, &str)],
) -> Result<Extraction, String> {
    let replacements: BTreeMap<String, String> = answers
        .iter()
        .map(|(character, replacement)| (character.to_string(), replacement.to_string()))
        .collect();
    extract_archive(text(archive), text(into), replacements)
}

/// Every file under `dir`, relative, sorted and forward-slashed.
fn files_under(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(children) = fs::read_dir(&current) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(
                    path.strip_prefix(dir)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    found.sort();
    found
}

/// The message of an outcome that refused to write anything, or a panic naming
/// what came back instead.
fn refusal(outcome: Extraction) -> String {
    match outcome {
        Extraction::NameProblem { message } => message,
        Extraction::Extracted { files } => {
            panic!("expected a naming question, but the archive extracted {files:?}")
        }
    }
}

fn written(outcome: Extraction) -> Vec<String> {
    match outcome {
        Extraction::Extracted { mut files } => {
            for name in &mut files {
                *name = name.replace('\\', "/");
            }
            files.sort();
            files
        }
        Extraction::NameProblem { message } => {
            panic!("expected an extraction, but it asked: {message}")
        }
    }
}

// ------------------------------------------------- the shape on the wire --

#[test]
fn the_dialog_is_handed_exactly_the_json_it_reads() {
    // The webview builds the whole dialog out of this object and nothing type
    // checks the crossing, so the shape is pinned here, from the Windows rules,
    // on whatever machine runs the suite. `apps/desktop/tests/App.test.js`
    // stubs this same shape by hand; if serde's tags move (the `kind` tag, the
    // camelCase variant names, the lowercase faults) this fails here and the
    // Vitest suite goes on passing against a shape that no longer exists.
    let names = ["logs/what?.txt", "when?.txt", "notes.txt.", "CON.txt"];
    let rules = NameRules::windows();
    let inspection = NameInspection::new(NameReport::of(&names, rules), rules);

    let wire = serde_json::to_value(&inspection).unwrap();
    assert_eq!(
        wire["entries"],
        json!([
            {
                "entry": "logs/what?.txt",
                "problems": [{ "kind": "character", "character": "?", "fault": "rejected" }],
            },
            {
                "entry": "when?.txt",
                "problems": [{ "kind": "character", "character": "?", "fault": "rejected" }],
            },
            {
                "entry": "notes.txt.",
                "problems": [{ "kind": "trailingCharacters", "removed": "." }],
            },
            {
                "entry": "CON.txt",
                "problems": [{ "kind": "reservedDevice", "device": "CON" }],
            },
        ])
    );
    // One question per character, not per entry: two files carry the `?`, and
    // the dialog puts one text field on screen for both.
    assert_eq!(
        wire["characters"],
        json!([{ "character": "?", "fault": "rejected", "entries": 2 }])
    );
    // Flattened, so the webview reads one object rather than reaching through
    // a wrapper for the half it needs.
    assert!(
        wire.get("report").is_none(),
        "the report must be flattened into the inspection: {wire}"
    );
}

#[test]
fn a_colon_is_offered_as_the_fault_that_it_is() {
    // The colon is the one Windows ACCEPTS: `notes.txt:hidden` is the `hidden`
    // stream of `notes.txt`, the write succeeds and the file exists under no
    // name (issue #63). The dialog says something quite different for it than
    // for a `?`, which it can only do if the fault survives serialization.
    let rules = NameRules::windows();
    let inspection = NameInspection::new(NameReport::of(&["notes.txt:hidden"], rules), rules);

    let wire = serde_json::to_value(&inspection).unwrap();
    assert_eq!(
        wire["characters"],
        json!([{ "character": ":", "fault": "reinterpreted", "entries": 1 }])
    );
}

#[test]
fn the_dialog_is_told_every_character_an_answer_may_not_contain() {
    // Sent as data so the dialog can refuse a bad answer as it is typed without
    // holding a copy of the rules in JavaScript. Two rulesets, both asked for
    // by name, so this runs everywhere.
    let windows: String = (0u8..=0x1f)
        .map(char::from)
        .chain("\"*/:<>?\\|".chars())
        .collect();
    assert_eq!(
        NameInspection::new(NameReport::default(), NameRules::windows()).rejected_in_replacement,
        windows
    );
    // Unix refuses one character, and the two separators are added to both:
    // answering `?` with `../` would move the entry to another directory rather
    // than rename it, which is why core refuses it whatever the ruleset.
    assert_eq!(
        NameInspection::new(NameReport::default(), NameRules::unix()).rejected_in_replacement,
        "\u{0}/\\"
    );
}

#[test]
fn an_archive_with_nothing_wrong_still_says_what_an_answer_may_not_contain() {
    // The empty case is not the null case: `rejectedInReplacement` describes
    // the host, not the archive, and a UI that only received it alongside a
    // complaint could not validate anything.
    let inspection = NameInspection::new(NameReport::default(), NameRules::windows());
    assert!(inspection.is_empty());
    assert!(inspection.rejected_in_replacement.contains('?'));
}

// ------------------------------------------------------------- the answers --

#[test]
fn the_answers_arrive_as_strings_and_become_characters() {
    let answers = BTreeMap::from([("?".to_string(), "-".to_string())]);
    let substitutions = substitutions_from(&answers).unwrap();
    assert_eq!(substitutions.get('?'), Some("-"));
}

#[test]
fn an_answer_keyed_by_more_than_one_character_is_refused_by_name() {
    // A JSON object has no `char` keys, so this is the one thing that can go
    // wrong in the translation, and it has to name the key it choked on.
    let answers = BTreeMap::from([("??".to_string(), "-".to_string())]);
    let problem = substitutions_from(&answers).unwrap_err().to_string();
    assert!(problem.contains("\"??\""), "{problem}");
    assert!(problem.contains("not a single character"), "{problem}");
}

// -------------------------------------------------------------- inspecting --

#[test]
fn inspecting_a_missing_archive_reports_it_by_path() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("nope.zip");

    assert_eq!(
        inspect(&missing).unwrap_err(),
        format!("Not found: {}", missing.to_string_lossy())
    );
}

#[test]
fn an_archive_this_computer_can_write_asks_nothing() {
    let dir = TempDir::new().unwrap();
    let archive = zip_with(
        dir.path(),
        &[("notes.txt", b"hello"), ("sub/deep.txt", b"hi")],
    );

    let inspection = inspect(&archive).unwrap();

    assert!(inspection.is_empty());
    assert!(inspection.report.entries.is_empty());
    assert!(inspection.report.characters.is_empty());
}

#[test]
fn an_archive_that_cannot_be_read_asks_nothing_and_leaves_the_complaining_to_the_extractor() {
    // Deliberate: the extractor is about to open the same file and fail on it
    // in its own words ("Could not find EOCD"), which is the message a user can
    // act on. Answering first would replace it with a worse one, and would put
    // a dialog in the way of an archive that has no naming question at all.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[("notes.txt", b"hello")]);
    let whole = fs::read(&archive).unwrap();
    fs::write(&archive, &whole[..whole.len() / 2]).unwrap();

    assert!(inspect(&archive).unwrap().is_empty());

    let out = dir.path().join("out");
    let complaint = extract_answering(&archive, &out, &[]).unwrap_err();
    assert!(complaint.contains("Zip"), "{complaint}");

    // Same for a name no backend claims: refused by the extractor, not here.
    let foreign = dir.path().join("photos.rar");
    fs::write(&foreign, b"not an archive").unwrap();
    assert!(inspect(&foreign).unwrap().is_empty());
    assert_eq!(
        extract_answering(&foreign, &out, &[]).unwrap_err(),
        "Compression failed: Unknown archive extension: .rar"
    );
}

// -------------------------------------------------------------- extracting --

#[test]
fn an_ordinary_archive_extracts_with_no_answers_at_all() {
    let dir = TempDir::new().unwrap();
    let archive = zip_with(
        dir.path(),
        &[("notes.txt", b"hello"), ("sub/deep.txt", b"hi")],
    );
    let out = dir.path().join("out");

    let outcome = extract_answering(&archive, &out, &[]).unwrap();

    assert_eq!(written(outcome), ["notes.txt", "sub/deep.txt"]);
    // What was reported is what is there: the listing is not a promise made
    // from the archive's own names.
    assert_eq!(files_under(&out), ["notes.txt", "sub/deep.txt"]);
    assert_eq!(fs::read(out.join("notes.txt")).unwrap(), b"hello");
}

#[test]
fn an_answer_containing_a_separator_is_refused_before_anything_is_written() {
    // `?` is writable on this host, so nothing in this archive needs answering:
    // the answer is refused on its own account, by the ruleset, before the
    // archive is opened. That is what keeps a bad answer from being discovered
    // half way through an extraction.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[("notes.txt", b"hello")]);
    let out = dir.path().join("out");

    let problem = refusal(extract_answering(&archive, &out, &[("?", "../escaped")]).unwrap());

    assert!(problem.contains("path separator"), "{problem}");
    assert!(
        !out.exists(),
        "the output directory was created before the answer was judged"
    );
}

#[test]
fn an_answer_keyed_by_a_whole_word_is_a_question_rather_than_a_failure() {
    // The webview's mistake, not the user's, but it still wrote nothing, so it
    // comes back the same way and the dialog stays open on it.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[("notes.txt", b"hello")]);
    let out = dir.path().join("out");

    let problem = refusal(extract_answering(&archive, &out, &[("colon", "-")]).unwrap());

    assert!(problem.contains("not a single character"), "{problem}");
    assert!(!out.exists(), "nothing may be written on a bad answer");
}

// --------------------------------------------- what this host really refuses --

/// An entry name this machine cannot write, and the character behind it.
///
/// The NUL byte cannot cross the libc boundary, so no Unix filesystem can hold
/// it: `std` answers `InvalidInput` before the kernel is ever asked. It is the
/// only character in that position on Unix, which is why the dialog is a
/// Windows feature in practice, and why it is also the only fixture that can
/// drive the real host rules from a Mac.
#[cfg(unix)]
const UNWRITABLE_HERE: &str = "bad\u{0}name.txt";

#[cfg(unix)]
#[test]
fn a_name_this_computer_cannot_write_is_reported_with_the_character_to_ask_about() {
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[(UNWRITABLE_HERE, b"hello")]);

    let inspection = inspect(&archive).unwrap();

    assert!(!inspection.is_empty());
    assert_eq!(inspection.report.entries.len(), 1);
    assert_eq!(inspection.report.entries[0].entry, UNWRITABLE_HERE);
    assert_eq!(inspection.report.characters.len(), 1);
    assert_eq!(inspection.report.characters[0].character, '\u{0}');
    assert_eq!(inspection.report.characters[0].entries, 1);
    // Nothing is created by asking: the dialog goes up before any destination
    // has been touched.
    assert_eq!(files_under(dir.path()), ["input.zip"]);
}

#[cfg(unix)]
#[test]
fn the_answer_is_written_and_the_listing_names_what_is_on_disk() {
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[(UNWRITABLE_HERE, b"hello")]);
    let out = dir.path().join("out");

    let outcome = extract_answering(&archive, &out, &[("\u{0}", "_")]).unwrap();

    // The name on disk, never the archive's: reporting `bad\0name.txt` would
    // name a file that exists nowhere and send the user looking for it.
    assert_eq!(written(outcome), ["bad_name.txt"]);
    assert_eq!(files_under(&out), ["bad_name.txt"]);
    assert_eq!(fs::read(out.join("bad_name.txt")).unwrap(), b"hello");
}

#[cfg(unix)]
#[test]
fn an_empty_answer_removes_the_character() {
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[(UNWRITABLE_HERE, b"hello")]);
    let out = dir.path().join("out");

    let outcome = extract_answering(&archive, &out, &[("\u{0}", "")]).unwrap();

    assert_eq!(written(outcome), ["badname.txt"]);
}

#[cfg(unix)]
#[test]
fn a_name_left_unanswered_stops_before_anything_is_written() {
    // The other entry is perfectly writable and still does not get written:
    // extraction settles every name from the listing before the first byte, so
    // a user who dismissed the dialog is not left with half an archive.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(
        dir.path(),
        &[("fine.txt", b"hello"), (UNWRITABLE_HERE, b"hello")],
    );
    let out = dir.path().join("out");

    let problem = refusal(extract_answering(&archive, &out, &[]).unwrap());

    assert!(
        problem.contains("no replacement for it was given"),
        "{problem}"
    );
    assert!(problem.contains("bad"), "the entry names itself: {problem}");
    assert!(
        !out.exists(),
        "an unanswered name wrote {:?}",
        files_under(&out)
    );
}

#[cfg(unix)]
#[test]
fn an_answer_this_computer_cannot_write_either_is_refused_with_the_reason() {
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[(UNWRITABLE_HERE, b"hello")]);
    let out = dir.path().join("out");

    // Replacing the NUL with a NUL is not an answer, and it is caught before
    // the archive is opened rather than by the write failing.
    let problem = refusal(extract_answering(&archive, &out, &[("\u{0}", "a\u{0}b")]).unwrap());

    assert!(problem.contains("cannot write"), "{problem}");
    assert!(!out.exists());
}

#[cfg(unix)]
#[test]
fn two_entries_that_would_land_on_one_name_are_refused_naming_both() {
    // Renaming one of them behind the user's back is how a file disappears
    // without anyone noticing, so the answer is refused and both names are
    // said out loud. The second entry here is one the host could write
    // perfectly well: the collision is created by the answer, not found in the
    // archive.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(
        dir.path(),
        &[(UNWRITABLE_HERE, b"first"), ("bad_name.txt", b"second")],
    );
    let out = dir.path().join("out");

    let problem = refusal(extract_answering(&archive, &out, &[("\u{0}", "_")]).unwrap());

    assert!(problem.contains("bad_name.txt"), "{problem}");
    assert!(problem.contains("both be written as"), "{problem}");
    assert!(!out.exists(), "a collision wrote {:?}", files_under(&out));

    // And an answer that keeps them apart goes through.
    let outcome = extract_answering(&archive, &out, &[("\u{0}", "-")]).unwrap();
    assert_eq!(written(outcome), ["bad-name.txt", "bad_name.txt"]);
}
