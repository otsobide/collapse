//! Entry names the host cannot write: the rules, the report a front end asks
//! for, and extraction with the answers a user gave (issues #63 and #64).
//!
//! Covers `src/compression/names.rs` plus the two dispatchers that exist for
//! it, `unwritable_names` and `extract_with`.
//!
//! **Almost every test here runs the Windows rules on whatever machine is
//! running the suite.** That is the point of `NameRules::windows()`: nobody
//! working on this repository has Windows, and the CI leg that does runs on the
//! release path only, so a rule reachable only under `#[cfg(windows)]` is a rule
//! nobody would find out was broken. The handful of tests that must know where
//! they are say so in their name.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use collapse_core::compression::{
    extract_7z, extract_tar, extract_zip, CharacterFault, NameError, NameProblem, NameReport,
    NameRules, Substitutions,
};
use collapse_core::{extract, extract_with, unwritable_names_with, ExtractOptions};
use sevenz_rust2::{SevenZArchiveEntry, SevenZWriter};
use tar::{Builder, EntryType, Header};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

// ---------------------------------------------------------------- fixtures --

const FORMATS: [&str; 3] = ["zip", "7z", "tar"];

/// Build an archive of `format` whose entries are named exactly as given.
///
/// The writers are driven at a low enough level that a name Windows hates
/// survives into the file: that is the whole fixture. tar goes through a raw
/// header for the same reason `security.rs` does, since `Builder::append_data`
/// has opinions of its own about names.
fn archive_with(dir: &Path, format: &str, entries: &[(&str, &[u8])]) -> PathBuf {
    let archive = dir.join(format!("input.{format}"));
    match format {
        "zip" => {
            let file = fs::File::create(&archive).unwrap();
            let mut writer = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            for (name, content) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(content).unwrap();
            }
            writer.finish().unwrap();
        }
        "7z" => {
            let mut writer = SevenZWriter::create(&archive).unwrap();
            for (name, content) in entries {
                let entry = SevenZArchiveEntry {
                    name: (*name).to_string(),
                    ..Default::default()
                };
                writer.push_archive_entry(entry, Some(*content)).unwrap();
            }
            writer.finish().unwrap();
        }
        "tar" => {
            let file = fs::File::create(&archive).unwrap();
            let mut builder = Builder::new(file);
            for (name, content) in entries {
                let mut header = Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_entry_type(EntryType::Regular);
                let bytes = name.as_bytes();
                header.as_old_mut().name[..bytes.len()].copy_from_slice(bytes);
                header.set_cksum();
                builder.append(&header, *content).unwrap();
            }
            builder.finish().unwrap();
        }
        other => panic!("no fixture builder for {other}"),
    }
    archive
}

/// Every file under `dir`, relative and sorted, with `/` separators so the
/// expectations read the same on Windows.
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

fn sorted(mut names: Vec<String>) -> Vec<String> {
    for name in &mut names {
        *name = name.replace('\\', "/");
    }
    names.sort();
    names
}

/// Extract the way a Windows machine would, wherever this runs.
fn as_windows(replacements: Substitutions) -> ExtractOptions {
    ExtractOptions::new()
        .with_rules(NameRules::windows())
        .with_replacements(replacements)
}

// ------------------------------------------------------------- the ruleset --

#[test]
fn windows_refuses_every_character_win32_reserves() {
    // Straight from "Naming Files, Paths, and Namespaces". Dropping any one of
    // these from the ruleset makes this fail, which is the only thing standing
    // between the list and somebody's memory of it. The two separators are
    // deliberately not here: the rules judge one component, and splitting is
    // the caller's job.
    for character in ['<', '>', '"', '|', '?', '*'] {
        let name = format!("a{character}b.txt");
        assert_eq!(
            NameRules::windows().problems(&name),
            vec![NameProblem::Character {
                character,
                fault: CharacterFault::Rejected,
            }],
            "{name}"
        );
    }
}

#[test]
fn a_colon_is_reinterpreted_rather_than_rejected() {
    // Issue #63, and the reason `CharacterFault` has two variants at all:
    // Windows *accepts* `notes.txt:hidden` and writes the bytes into an
    // alternate data stream of `notes.txt`. Calling it `Rejected` would let a
    // front end tell the user the write fails, when the danger is that it does
    // not.
    assert_eq!(
        NameRules::windows().problems("notes.txt:hidden"),
        vec![NameProblem::Character {
            character: ':',
            fault: CharacterFault::Reinterpreted,
        }]
    );
}

#[test]
fn windows_refuses_control_characters_and_unix_refuses_only_the_nul() {
    // The documented range is 0 through 31 for Windows. On Unix the only byte a
    // file name cannot hold is the NUL, and this pins the asymmetry so nobody
    // "simplifies" the two rulesets into one.
    for character in ['\u{0}', '\u{1}', '\t', '\n', '\u{1f}'] {
        let name = format!("a{character}b");
        assert!(
            !NameRules::windows().can_write(&name),
            "windows should refuse {:?}",
            character
        );
    }
    assert!(!NameRules::unix().can_write("a\u{0}b"));
    for character in ['\u{1}', '\t', '\n', '\u{1f}'] {
        let name = format!("a{character}b");
        assert!(
            NameRules::unix().can_write(&name),
            "unix holds {:?} perfectly well",
            character
        );
    }
}

#[test]
fn a_trailing_dot_or_space_is_a_problem_and_the_whole_run_is_reported() {
    assert_eq!(
        NameRules::windows().problems("notes.txt."),
        vec![NameProblem::TrailingCharacters {
            removed: ".".to_string()
        }]
    );
    assert_eq!(
        NameRules::windows().problems("notes.txt . "),
        vec![NameProblem::TrailingCharacters {
            removed: " . ".to_string()
        }],
        "the run is everything the host would drop, not just the last character"
    );
    // The counterweight: dots and spaces are ordinary in the middle of a name,
    // and a rule that flagged them would make this feature fire on nearly every
    // archive.
    assert!(NameRules::windows().can_write("my notes.v2.txt"));
    assert!(NameRules::windows().can_write(".hidden"));
}

#[test]
fn reserved_device_names_are_matched_without_their_extension_and_case_insensitively() {
    for name in [
        "CON",
        "con",
        "Con.txt",
        "PRN",
        "AUX",
        "NUL",
        "NUL.tar.gz",
        "COM1",
        "com9",
        "LPT1",
        "LPT9",
        "CON ",
    ] {
        assert!(
            !NameRules::windows().can_write(name),
            "{name} resolves to a device"
        );
    }
    // The superscripts are not a typo: Windows reads ISO 8859-1 `¹ ² ³` as
    // digits, so these are devices too. Nothing but reading the documentation
    // would put them in the ruleset, and nothing but this test would keep them.
    for name in ["COM¹", "com²", "LPT³"] {
        assert!(
            !NameRules::windows().can_write(name),
            "{name} resolves to a device"
        );
    }
    // And the neighbours that are perfectly ordinary files. A rule written as
    // "starts with CON" or "contains COM" fails here.
    for name in [
        "COM0",
        "COM10",
        "CONS",
        "CONSOLE.txt",
        "my CON",
        "a.CON",
        "LPT",
        "NULL",
    ] {
        assert!(
            NameRules::windows().can_write(name),
            "{name} is an ordinary file name"
        );
    }
}

#[test]
fn unix_writes_what_windows_will_not() {
    // The honest framing of the whole feature: on Unix nearly nothing here is a
    // problem, so extraction has no question to ask. If this ever fails, the
    // host rules have picked up portability rules, and every Mac and Linux user
    // is being asked about names their machine can hold.
    for name in ["what?.txt", "notes.txt.", "CON", "a:b", "trailing "] {
        assert!(
            NameRules::unix().can_write(name),
            "{name} is writable on Unix"
        );
        assert!(
            !NameRules::windows().can_write(name),
            "{name} is not writable on Windows"
        );
    }
}

#[test]
fn the_host_rules_are_this_platform_s_rules() {
    let expected = if cfg!(windows) {
        NameRules::windows()
    } else {
        NameRules::unix()
    };
    assert_eq!(NameRules::host(), expected);
    assert_eq!(NameRules::default(), NameRules::host());
}

// ------------------------------------------------------------- rewriting ----

#[test]
fn a_replacement_is_applied_to_every_occurrence_and_may_be_empty() {
    let rules = NameRules::windows();
    let answers = Substitutions::new().with('?', "_");
    assert_eq!(rules.rewrite("a?b?c", &answers).unwrap(), "a_b_c");
    let dropped = Substitutions::new().with('?', "");
    assert_eq!(rules.rewrite("a?b", &dropped).unwrap(), "ab");
}

#[test]
fn the_structural_problems_are_adjusted_without_being_asked() {
    // A trailing dot and a device name have no offending character, so there is
    // nothing to put a text field beside; they are stated, not asked. If this
    // ever needs an answer, the UI has a field with no question.
    let rules = NameRules::windows();
    let nothing = Substitutions::new();
    assert_eq!(rules.rewrite("notes.txt.", &nothing).unwrap(), "notes.txt");
    assert_eq!(rules.rewrite("CON.txt", &nothing).unwrap(), "CON_.txt");
    assert_eq!(
        rules.rewrite("con", &nothing).unwrap(),
        "con_",
        "the adjustment keeps the spelling the archive used"
    );
}

#[test]
fn the_adjustments_run_after_the_replacements_not_before() {
    // Both of these come out wrong if the order in `rewrite` is reversed, and
    // both are reachable from an ordinary answer:
    let rules = NameRules::windows();
    // `M` for `?` spells a device that was not in the archive.
    assert_eq!(
        rules
            .rewrite("CO?1", &Substitutions::new().with('?', "M"))
            .unwrap(),
        "COM1_"
    );
    // `.` for `?` puts a dot at the end, which Windows would drop silently.
    assert_eq!(
        rules
            .rewrite("notes?", &Substitutions::new().with('?', "."))
            .unwrap(),
        "notes"
    );
}

#[test]
fn a_replacement_the_host_cannot_write_either_is_refused() {
    let err = NameRules::windows()
        .rewrite("a?b", &Substitutions::new().with('?', "*"))
        .unwrap_err();
    assert_eq!(
        err,
        NameError::UnwritableReplacement {
            character: '?',
            replacement: "*".to_string(),
            offending: '*',
        }
    );
}

#[test]
fn a_replacement_may_not_contain_a_path_separator() {
    // Not a usability rule: `../` in an answer is a traversal, since the
    // replacement lands inside a component that has already been cleared by the
    // containment guard.
    for replacement in ["../", "a/b", r"a\b"] {
        let err = NameRules::windows()
            .rewrite("a?b", &Substitutions::new().with('?', replacement))
            .unwrap_err();
        assert!(
            matches!(err, NameError::SeparatorInReplacement { .. }),
            "{replacement}: {err}"
        );
    }
}

#[test]
fn an_answer_that_leaves_no_name_or_spells_a_parent_directory_is_refused() {
    let rules = NameRules::windows();
    // An answer of `.` for a two-character name spells the directory above,
    // which would climb out of the output directory. Under these rules the
    // trailing-dot adjustment gets there first and leaves nothing at all, so
    // the refusal says `""` rather than `".."`; either way it is refused, and
    // the second assertion is the one that matters if a future ruleset stops
    // trimming trailing dots.
    let outcome = rules.rewrite("??", &Substitutions::new().with('?', "."));
    assert!(
        matches!(&outcome, Err(NameError::Unnameable { .. })),
        "{outcome:?}"
    );
    assert_ne!(outcome.unwrap_or_default(), "..");
    // An empty answer that empties the whole name.
    let err = rules
        .rewrite("??", &Substitutions::new().with('?', ""))
        .unwrap_err();
    assert!(matches!(err, NameError::Unnameable { result, .. } if result.is_empty()));
    // And a name that is nothing but what the host drops, which no answer can
    // help with because there is no character to answer for.
    let err = rules.rewrite("...", &Substitutions::new()).unwrap_err();
    assert!(matches!(err, NameError::Unnameable { .. }), "{err}");
}

#[test]
fn an_unanswered_character_is_reported_against_the_whole_entry() {
    // The user sees entry names, not components. Dropping `in_entry` would name
    // `a?b.txt` here, which is not a line they can find in any listing.
    let err = NameRules::windows()
        .rewrite_entry("photos/2026/a?b.txt", &Substitutions::new())
        .unwrap_err();
    assert_eq!(
        err,
        NameError::NoReplacement {
            entry: "photos/2026/a?b.txt".to_string(),
            character: '?',
        }
    );
    assert!(err.to_string().contains("photos/2026/a?b.txt"), "{err}");
}

#[test]
fn every_component_of_an_entry_is_rewritten() {
    let answers = Substitutions::new().with(':', "-").with('?', "_");
    let written = NameRules::windows()
        .rewrite_entry("a:b/CON/c?.txt.", &answers)
        .unwrap();
    assert_eq!(written, PathBuf::from("a-b").join("CON_").join("c_.txt"));
}

#[test]
fn a_key_that_is_not_a_single_character_is_refused() {
    // The front ends receive their answers as strings (a JSON object has no
    // char keys), so this is where "??" or "" is caught, once, instead of in
    // each of them.
    let mut answers = Substitutions::new();
    assert!(answers.set_str("?", "_").is_ok());
    assert_eq!(answers.get('?'), Some("_"));
    assert!(matches!(
        answers.set_str("??", "_"),
        Err(NameError::NotOneCharacter { .. })
    ));
    assert!(matches!(
        answers.set_str("", "_"),
        Err(NameError::NotOneCharacter { .. })
    ));
}

// ---------------------------------------------------------------- reports ---

#[test]
fn the_report_asks_about_each_character_once_and_says_how_many_entries_carry_it() {
    // One text field per character is the whole shape of the UI: a report that
    // listed the characters per entry would put three fields on screen for the
    // same question.
    let names = [
        "ok.txt",
        "a?b.txt",
        "c?d.txt",
        "notes.txt:hidden",
        "photos/e?f.txt",
    ];
    let report = NameReport::of(&names, NameRules::windows());
    assert_eq!(report.entries.len(), 4);
    assert_eq!(
        report
            .characters
            .iter()
            .map(|c| (c.character, c.fault, c.entries))
            .collect::<Vec<_>>(),
        vec![
            ('?', CharacterFault::Rejected, 3),
            (':', CharacterFault::Reinterpreted, 1),
        ]
    );
}

#[test]
fn the_report_separates_the_questions_from_the_stated_adjustments() {
    let names = ["what?.txt", "notes.txt.", "CON.log"];
    let report = NameReport::of(&names, NameRules::windows());
    let asked: Vec<Option<char>> = report
        .entries
        .iter()
        .map(|e| e.problems[0].replaceable())
        .collect();
    assert_eq!(
        asked,
        vec![Some('?'), None, None],
        "only a character is a question; the other two are announcements"
    );
    assert_eq!(
        report.characters.len(),
        1,
        "a trailing dot and a device name must not produce a text field"
    );
    assert_eq!(
        report.entries[2].problems,
        vec![NameProblem::ReservedDevice {
            device: "CON".to_string()
        }]
    );
}

#[test]
fn a_listing_the_host_can_write_reports_nothing() {
    let names = ["a.txt", "photos/b.jpg", "photos/sub/"];
    assert!(NameReport::of(&names, NameRules::windows()).is_empty());
    assert!(NameReport::of(&names, NameRules::unix()).is_empty());
}

// --------------------------------------------------- inspecting an archive --

#[test]
fn inspecting_an_archive_finds_the_unwritable_names_without_extracting_anything() {
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[
                ("summary.txt", b"fine"),
                ("what?.txt", b"question"),
                ("notes.txt.", b"trailing"),
                ("CON.txt", b"device"),
            ],
        );

        let report = unwritable_names_with(&archive, NameRules::windows()).unwrap();

        assert_eq!(
            report
                .entries
                .iter()
                .map(|e| e.entry.as_str())
                .collect::<Vec<_>>(),
            vec!["what?.txt", "notes.txt.", "CON.txt"],
            "{format}: the writable entry must not be reported"
        );
        assert_eq!(
            report.characters.len(),
            1,
            "{format}: one question, for `?`"
        );
        // Nothing was written: this is what a front end calls before it has
        // even asked the user where the files should go.
        assert_eq!(
            files_under(dir.path()),
            vec![format!("input.{format}")],
            "{format}: inspection created something"
        );
    }
}

#[test]
fn inspecting_this_machine_s_own_archives_asks_nothing() {
    // The regression that would make the feature intolerable: a report that
    // fires on ordinary names would put a dialog in front of every extraction.
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[("summary.txt", b"fine"), ("photos/a.jpg", b"also fine")],
        );
        assert!(
            collapse_core::unwritable_names(&archive)
                .unwrap()
                .is_empty(),
            "{format}"
        );
    }
}

// -------------------------------------------- extracting with the answers ---

#[test]
fn the_answers_are_written_and_the_listing_names_what_is_on_disk() {
    // Issue #64 end to end, for all three formats. The listing is the half that
    // matters most: returning the archive's names would have a front end show
    // `what?.txt` next to a file called `what_.txt`.
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[
                ("summary.txt", b"fine"),
                ("what?.txt", b"question"),
                ("notes.txt.", b"trailing"),
                ("CON.txt", b"device"),
            ],
        );
        let out = dir.path().join("out");

        let written = extract_with(
            &archive,
            &out,
            &as_windows(Substitutions::new().with('?', "_")),
        )
        .unwrap();

        let expected = vec![
            "CON_.txt".to_string(),
            "notes.txt".to_string(),
            "summary.txt".to_string(),
            "what_.txt".to_string(),
        ];
        assert_eq!(sorted(written), expected, "{format}: the returned listing");
        assert_eq!(files_under(&out), expected, "{format}: what is on disk");
        assert_eq!(
            fs::read(out.join("what_.txt")).unwrap(),
            b"question",
            "{format}: the renamed entry kept its content"
        );
    }
}

#[test]
fn a_colon_entry_becomes_a_file_of_its_own_and_leaves_its_neighbour_alone() {
    // Issue #63. On Windows the unfixed path writes these bytes into the
    // `hidden` stream of `notes.txt`, which changes nothing about `notes.txt`
    // that `dir` can see and leaves the listing naming a file that exists
    // nowhere. Here the answer turns it into a file, and the assertion that
    // `notes.txt` still holds its own bytes is what would fail if a future
    // "simplification" let the colon through.
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[
                ("notes.txt", b"the real file"),
                ("notes.txt:hidden", b"the payload"),
            ],
        );
        let out = dir.path().join("out");

        let written = extract_with(
            &archive,
            &out,
            &as_windows(Substitutions::new().with(':', "-")),
        )
        .unwrap();

        assert_eq!(
            sorted(written),
            vec!["notes.txt".to_string(), "notes.txt-hidden".to_string()],
            "{format}"
        );
        assert_eq!(fs::read(out.join("notes.txt")).unwrap(), b"the real file");
        assert_eq!(
            fs::read(out.join("notes.txt-hidden")).unwrap(),
            b"the payload"
        );
    }
}

#[test]
fn an_entry_with_no_answer_stops_before_anything_is_written() {
    // The pre-pass earning its keep: judged one entry at a time, `summary.txt`
    // would already be on disk when `what?.txt` was refused, and the user would
    // be left with half a directory and no list of what is in it (issue #64's
    // other complaint).
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[("summary.txt", b"fine"), ("what?.txt", b"question")],
        );
        let out = dir.path().join("out");

        let err = extract_with(&archive, &out, &as_windows(Substitutions::new())).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("what?.txt"), "{format}: {message}");
        assert!(message.contains('?'), "{format}: {message}");
        assert!(
            files_under(&out).is_empty(),
            "{format}: {:?} was written before the refusal",
            files_under(&out)
        );
    }
}

#[test]
fn two_entries_that_would_land_on_one_name_are_refused_by_name() {
    // Deliberately not disambiguated: renaming one of them to `a_b (2).txt` is
    // how a user ends up with a file they never look at again. Both names are
    // in the message so the answer can be changed.
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[("a?b.txt", b"first"), ("a*b.txt", b"second")],
        );
        let out = dir.path().join("out");

        let answers = Substitutions::new().with('?', "_").with('*', "_");
        let err = extract_with(&archive, &out, &as_windows(answers)).unwrap_err();

        let message = err.to_string();
        assert!(message.contains("a?b.txt"), "{format}: {message}");
        assert!(message.contains("a*b.txt"), "{format}: {message}");
        assert!(message.contains("a_b.txt"), "{format}: {message}");
        assert!(
            files_under(&out).is_empty(),
            "{format}: a collision must leave the output alone"
        );
    }
}

#[test]
fn a_renamed_entry_colliding_with_an_untouched_one_is_refused_too() {
    // The case a "compare the rewritten names to each other" check would miss:
    // only one of these two changes, and it lands on a name the archive already
    // uses.
    let dir = tempfile::TempDir::new().unwrap();
    let archive = archive_with(
        dir.path(),
        "zip",
        &[("notes.txt", b"first"), ("notes.txt.", b"second")],
    );
    let out = dir.path().join("out");

    let err = extract_with(&archive, &out, &as_windows(Substitutions::new())).unwrap_err();

    let message = err.to_string();
    assert!(message.contains("notes.txt."), "{message}");
    assert!(files_under(&out).is_empty());
}

#[test]
fn an_archive_that_already_names_one_entry_twice_still_extracts() {
    // The counterweight to the collision rule. Duplicate entries are an old
    // property of tar (and of extraction here: the last one wins), and this
    // feature must not turn them into a refusal, because no answer the user
    // gives could fix an archive that was already like that.
    let dir = tempfile::TempDir::new().unwrap();
    let archive = archive_with(
        dir.path(),
        "tar",
        &[("notes.txt", b"first"), ("notes.txt", b"second")],
    );
    let out = dir.path().join("out");

    let written = extract_with(&archive, &out, &as_windows(Substitutions::new())).unwrap();

    assert_eq!(
        written,
        vec!["notes.txt".to_string(), "notes.txt".to_string()]
    );
    assert_eq!(fs::read(out.join("notes.txt")).unwrap(), b"second");
}

#[test]
fn an_answer_the_host_cannot_write_is_refused_before_the_archive_is_opened() {
    // Checked against a path that does not exist: if the answer were validated
    // per entry instead of up front, this would fail with "no such file"
    // instead, and a user would only learn their replacement was no good after
    // choosing an archive.
    let dir = tempfile::TempDir::new().unwrap();
    let missing = dir.path().join("nowhere.zip");
    let err = extract_with(
        &missing,
        &dir.path().join("out"),
        &as_windows(Substitutions::new().with('?', "<")),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            collapse_core::CompressionError::Name(NameError::UnwritableReplacement { .. })
        ),
        "{err}"
    );
}

#[test]
fn extraction_with_no_options_leaves_ordinary_names_alone() {
    // `extract` is `extract_with` with the host's rules and no answers, and on
    // this machine that has to be exactly what it always was.
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[("summary.txt", b"fine"), ("photos/a.jpg", b"also fine")],
        );
        let out = dir.path().join("out");
        let written = extract(&archive, &out).unwrap();
        assert_eq!(
            sorted(written),
            vec!["photos/a.jpg".to_string(), "summary.txt".to_string()],
            "{format}"
        );
        assert_eq!(files_under(&out), vec!["photos/a.jpg", "summary.txt"]);
    }
}

#[test]
fn the_backends_called_directly_still_write_the_archive_s_own_names() {
    // The plan is the dispatcher's business. `extract_zip` and friends are
    // public and are called directly (the server unpacks a tar envelope that
    // way), so they must keep behaving as they did: no listing pass, no
    // renaming, on any platform.
    let dir = tempfile::TempDir::new().unwrap();
    for (format, extractor) in [
        ("zip", extract_zip as fn(&Path, &Path) -> _),
        ("7z", extract_7z),
        ("tar", extract_tar),
    ] {
        let archive = archive_with(dir.path(), format, &[("plain.txt", b"content")]);
        let out = dir.path().join(format!("out-{format}"));
        assert_eq!(
            extractor(&archive, &out).unwrap(),
            vec!["plain.txt".to_string()],
            "{format}"
        );
    }
}

#[test]
fn an_archive_that_cannot_be_listed_still_fails_in_the_extractor_s_words() {
    // The listing pass is advisory on purpose. A corrupt archive must produce
    // the message extraction has always produced, not a second-hand one from a
    // pass that only exists to plan names.
    let dir = tempfile::TempDir::new().unwrap();
    let archive = dir.path().join("truncated.zip");
    fs::write(&archive, b"PK\x03\x04 and then nothing").unwrap();

    let err = extract(&archive, &dir.path().join("out")).unwrap_err();
    let message = err.to_string();
    assert!(message.starts_with("Compression failed:"), "{message}");
    assert!(message.contains("Zip"), "{message}");
}

// ----------------------------------------------- naming the failing entry ---

#[test]
fn a_failing_entry_names_itself_and_its_destination() {
    // Issue #64, piece one, and it helps on every platform: this exact archive
    // (an entry whose parent is another entry, a plain file) used to produce
    // `IO error: File exists (os error 17)` with no clue which of four entries
    // was at fault. A read-only output directory and a full disk were just as
    // blank.
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[("a.txt", b"a file"), ("a.txt/b.txt", b"a child of a file")],
        );
        let out = dir.path().join("out");

        let err = extract(&archive, &out).unwrap_err();

        let message = err.to_string();
        assert!(
            message.contains("a.txt/b.txt") || message.contains(r"a.txt\b.txt"),
            "{format}: the message must name the entry: {message}"
        );
        assert!(
            message.contains(&out.display().to_string()),
            "{format}: the message must say where it was going: {message}"
        );
    }
}
