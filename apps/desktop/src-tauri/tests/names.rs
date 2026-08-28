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
fn a_hostile_answer_changes_nothing_because_no_answer_is_applied() {
    // This used to be the guard on the answer itself: `?` replaced with
    // `../escaped` would have carried an entry out of the output directory, so
    // the ruleset refused the answer before the archive was opened.
    //
    // There is no answer to refuse any more. Nothing in this archive is
    // unwritable, so it extracts, and the point is what does *not* happen: the
    // replacement reaches no name, creates no directory and moves nothing. A
    // regression here would show up as `escaped` existing somewhere.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[("notes.txt", b"hello")]);
    let out = dir.path().join("out");

    let outcome = extract_answering(&archive, &out, &[("?", "../escaped")]).unwrap();

    assert_eq!(written(outcome), ["notes.txt"]);
    assert_eq!(files_under(&out), ["notes.txt"]);
    assert!(
        !dir.path().join("escaped").exists() && !out.join("escaped").exists(),
        "the answer reached a path: {:?}",
        files_under(dir.path())
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
fn no_answer_rescues_a_name_this_computer_cannot_write() {
    // Both halves of what the dialog used to offer, and neither works now: a
    // character to put in its place, and an empty answer meaning "just drop
    // it". The name the archive gave is the only name that may be written, so
    // an entry holding a NUL does not arrive under some other spelling — it
    // does not arrive.
    //
    // This is the test to look at if the dialog is ever wired back up by
    // accident: it is the one that says the answers are inert.
    for answer in ["_", ""] {
        let dir = TempDir::new().unwrap();
        let archive = zip_with(dir.path(), &[(UNWRITABLE_HERE, b"hello")]);
        let out = dir.path().join("out");

        let problem = refusal(extract_answering(&archive, &out, &[("\u{0}", answer)]).unwrap());

        assert!(
            problem.contains("cannot be written on this system"),
            "answer {answer:?}: {problem}"
        );
        assert!(
            !out.exists(),
            "answer {answer:?} wrote {:?}",
            files_under(&out)
        );
    }
}

#[cfg(unix)]
#[test]
fn a_name_this_computer_cannot_write_stops_before_anything_is_written() {
    // The other entry is perfectly writable and still does not get written:
    // every name is judged from the listing before the first byte, so a user is
    // never left with half an archive and no way to tell which half.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(
        dir.path(),
        &[("fine.txt", b"hello"), (UNWRITABLE_HERE, b"hello")],
    );
    let out = dir.path().join("out");

    let problem = refusal(extract_answering(&archive, &out, &[]).unwrap());

    assert!(
        problem.contains("cannot be written on this system"),
        "{problem}"
    );
    assert!(problem.contains("bad"), "the entry names itself: {problem}");
    assert!(
        !out.exists(),
        "an unwritable name wrote {:?}",
        files_under(&out)
    );
}

#[cfg(unix)]
#[test]
fn the_refusal_says_which_character_is_the_problem() {
    // The reason has to travel, because the dialog renders it and it is all the
    // user gets: naming the entry without naming the character leaves them
    // staring at a file name that looks fine, since a NUL prints as nothing.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(dir.path(), &[(UNWRITABLE_HERE, b"hello")]);
    let out = dir.path().join("out");

    let problem = refusal(extract_answering(&archive, &out, &[]).unwrap());

    assert!(problem.contains("refuses in a file name"), "{problem}");
    assert!(
        problem.contains("\\0"),
        "the character is shown escaped: {problem}"
    );
    assert!(!out.exists());
}

#[cfg(unix)]
#[test]
fn an_entry_beside_the_name_it_would_have_taken_is_still_just_refused() {
    // This was the collision case: answering the NUL with `_` made this entry
    // into `bad_name.txt`, which the archive already holds, and one of the two
    // would have been written over the other. Both names went into the refusal
    // so the answer could be changed.
    //
    // Nothing is renamed now, so the two names stay two names and there is no
    // collision to find. The archive is refused for the NUL alone, and
    // `bad_name.txt` — an entry this host writes perfectly well — is not
    // written either, which is the part worth keeping.
    let dir = TempDir::new().unwrap();
    let archive = zip_with(
        dir.path(),
        &[(UNWRITABLE_HERE, b"first"), ("bad_name.txt", b"second")],
    );
    let out = dir.path().join("out");

    let problem = refusal(extract_answering(&archive, &out, &[("\u{0}", "_")]).unwrap());

    assert!(
        problem.contains("cannot be written on this system"),
        "{problem}"
    );
    assert!(!out.exists(), "wrote {:?}", files_under(&out));
}
