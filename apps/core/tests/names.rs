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
    NameRules,
};
use collapse_core::{
    extract, extract_with, unwritable_names_with, CompressionError, ExtractOptions,
};
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
fn as_windows() -> ExtractOptions {
    ExtractOptions::new().with_rules(NameRules::windows())
}

/// The message of the refusal an unwritable entry must produce.
///
/// Panics naming what happened instead, because the two ways this goes wrong
/// need telling apart: extracting anything at all is the policy being broken,
/// while a different error is usually the fixture failing to build the name.
fn refusal(result: Result<Vec<String>, CompressionError>, context: &str) -> String {
    match result {
        Err(CompressionError::Name(problem @ NameError::Unwritable { .. })) => problem.to_string(),
        Err(other) => panic!("{context}: expected a naming refusal, got {other}"),
        Ok(files) => panic!("{context}: expected a refusal, extracted {files:?}"),
    }
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

/// `characters` counts only the faults that are about a character, which is
/// what the CLI's refusal message groups by. A trailing dot and a device name
/// are entries with a problem and nothing to group.
#[test]
fn only_a_character_fault_reaches_the_characters_half_of_the_report() {
    let names = ["what?.txt", "notes.txt.", "CON.log"];
    let report = NameReport::of(&names, NameRules::windows());

    assert_eq!(report.entries.len(), 3, "all three are refused");
    assert_eq!(
        report.characters.len(),
        1,
        "but only the `?` is a character anyone could name"
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
fn every_fault_stops_the_whole_archive_the_same_way() {
    // Issue #64 end to end, for all three formats, under the policy that
    // replaced the answers. The four faults used to have four different
    // endings: a question put to the user, a trailing run truncated in
    // silence, a `_` appended to a device name. They have one ending now, and
    // that is the whole of what this pins.
    //
    // `summary.txt` rides along in every archive and is the assertion that
    // matters most. It is perfectly writable on any host, and it must still not
    // be on disk: the refusal is judged over the listing before a byte is
    // written, so a good entry beside a bad one goes nowhere either.
    for format in FORMATS {
        for (bad, fault) in [
            ("what?.txt", "a character the host refuses"),
            ("notes.txt:hidden", "a character the host reinterprets"),
            ("notes.txt.", "a trailing dot"),
            ("CON.txt", "a reserved device"),
        ] {
            let dir = tempfile::TempDir::new().unwrap();
            let archive = archive_with(
                dir.path(),
                format,
                &[("summary.txt", b"fine"), (bad, b"trouble")],
            );
            let out = dir.path().join("out");
            let context = format!("{format}, {fault}");

            let message = refusal(extract_with(&archive, &out, &as_windows()), &context);

            assert!(message.contains(bad), "{context}: {message}");
            assert!(
                files_under(&out).is_empty(),
                "{context}: {:?} was written before the refusal",
                files_under(&out)
            );
        }
    }
}

#[test]
fn a_colon_entry_is_refused_and_its_neighbour_is_never_written() {
    // Issue #63. On Windows the unfixed path writes these bytes into the
    // `hidden` stream of `notes.txt`, which changes nothing about `notes.txt`
    // that `dir` can see and leaves the listing naming a file that exists
    // nowhere.
    //
    // The answer used to be a substitution that turned it into a file of its
    // own. That is gone: `notes.txt-hidden` is not the file the archive named
    // either, and inventing it hid the problem rather than reporting it.
    //
    // `notes.txt` is what would catch a future "simplification" letting the
    // colon through. It is the innocent half of the archive, and it must not
    // exist: on a host that refuses the other entry, nothing is written.
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

        let message = refusal(extract_with(&archive, &out, &as_windows()), format);

        assert!(message.contains("notes.txt:hidden"), "{format}: {message}");
        assert!(message.contains(':'), "{format}: {message}");
        assert!(
            !out.join("notes.txt").exists(),
            "{format}: the neighbour was written despite the refusal"
        );
        assert!(files_under(&out).is_empty(), "{format}");
    }
}

#[test]
fn two_entries_that_used_to_collide_are_refused_for_their_own_names() {
    // These two were the collision case: `?` and `*` both answered with `_`
    // made one name out of two, and the refusal named all three spellings so
    // the answer could be changed.
    //
    // A collision can no longer be manufactured, because nothing is renamed, so
    // the guard that looked for one has gone with it. The archive is still
    // refused, for the plainer reason that this host cannot write either name.
    //
    // Only the first offending entry is reported, and the assertion that
    // `a_b.txt` is absent is the one that matters: a planned name appearing in
    // a message would mean something is still working one out.
    for format in FORMATS {
        let dir = tempfile::TempDir::new().unwrap();
        let archive = archive_with(
            dir.path(),
            format,
            &[("a?b.txt", b"first"), ("a*b.txt", b"second")],
        );
        let out = dir.path().join("out");

        let message = refusal(extract_with(&archive, &out, &as_windows()), format);

        assert!(message.contains("a?b.txt"), "{format}: {message}");
        assert!(
            !message.contains("a_b.txt"),
            "{format}: nothing is renamed, so no planned name should appear: {message}"
        );
        assert!(files_under(&out).is_empty(), "{format}");
    }
}

#[test]
fn a_trailing_dot_is_refused_rather_than_folded_onto_the_name_beside_it() {
    // `notes.txt.` used to lose its dot and land on `notes.txt`, which the
    // archive already holds, and the collision guard existed to catch exactly
    // that. Now the dot is never dropped, so the two names stay two names and
    // the archive is refused for the one the host will not preserve.
    //
    // The neighbour is ordinary and still goes nowhere, which is the all-or-
    // nothing half of the policy.
    let dir = tempfile::TempDir::new().unwrap();
    let archive = archive_with(
        dir.path(),
        "zip",
        &[("notes.txt", b"first"), ("notes.txt.", b"second")],
    );
    let out = dir.path().join("out");

    let message = refusal(extract_with(&archive, &out, &as_windows()), "zip");

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

    let written = extract_with(&archive, &out, &as_windows()).unwrap();

    assert_eq!(
        written,
        vec!["notes.txt".to_string(), "notes.txt".to_string()]
    );
    assert_eq!(fs::read(out.join("notes.txt")).unwrap(), b"second");
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
fn an_archive_that_cannot_be_listed_still_fails_in_the_parser_s_words() {
    // The listing pass is no longer advisory (issue #89): it stops the
    // extraction. The message is unaffected, because both passes go through the
    // same parser, which is what made the old "keep going so the extractor can
    // phrase it better" argument weaker than it looked.
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
        // Quoted, as an entry, not merely present as a substring of the
        // destination path. This assertion used to be a bare `contains`, which
        // tar satisfied for the wrong reason: its message was `failed to unpack
        // \`/…/out/a.txt/b.txt\``, so the entry name "appeared" only as part of
        // the path, and the guarantee issue #64 asked for was not there at all
        // (issue #93).
        assert!(
            message.contains(r#"entry "a.txt/b.txt""#)
                || message.contains(r#"entry "a.txt\b.txt""#),
            "{format}: the message must name the entry as an entry: {message}"
        );
        // And say what the operating system said, in the same register on every
        // format, rather than the dependency's own wrapper.
        assert!(
            message.contains("os error"),
            "{format}: the message must carry the real cause: {message}"
        );
        // Windows renders this from a canonicalized root, which carries a `\\?\`
        // verbatim prefix and expands any 8.3 short name on the way, so the
        // message legitimately does not contain the path this test built. What
        // it must contain is where extraction actually resolved to.
        let resolved = out.canonicalize().unwrap_or_else(|_| out.clone());
        assert!(
            message.contains(&resolved.display().to_string()),
            "{format}: the message must say where it was going: {message}"
        );
    }
}

// ------------------------------------- the seam: splitting is not the host's --

/// The Windows half of the same seam: the report must ask about a colon
/// wherever it sits, including the leading component that used to vanish.
///
/// Note this one passed on Unix before the fix and failed only on Windows,
/// since `a:b` was already a single component here. It is a pin, not a
/// reproduction: what it stops is the answer diverging by host again.
#[test]
fn the_report_sees_a_colon_in_the_first_component_too() {
    for name in ["a:b/c.txt", "C:/x/y", "deep/a:b.txt"] {
        let problems = NameRules::windows().entry_problems(name);
        assert_eq!(
            problems,
            vec![NameProblem::Character {
                character: ':',
                fault: CharacterFault::Reinterpreted,
            }],
            "{name}: the colon must be a question on every host"
        );
    }
}

/// A backslash is an ordinary character in an archive entry, so the two rulesets
/// must disagree about it, and each must be right about its own filesystem.
#[test]
fn only_windows_refuses_a_backslash_inside_a_component() {
    assert_eq!(
        NameRules::windows().entry_problems(r"dir\file.txt"),
        vec![NameProblem::Character {
            character: '\\',
            fault: CharacterFault::Rejected,
        }]
    );
    assert!(NameRules::unix().entry_problems(r"dir\file.txt").is_empty());
}

/// Issue caught by nothing until Windows CI ran: collapse could build an archive
/// it then refused to extract.
///
/// A backslash is a legal character in a Unix file name. The old splitter made
/// it one component on Unix, and `rewrite` refused any rewritten component
/// holding a separator, so `extract` failed the **whole** archive and wrote
/// nothing, after `compress_dir` had happily archived it and the verification
/// pass had signed it off. The user could have deleted the originals by then.
#[cfg(unix)]
#[test]
fn a_unix_name_holding_a_backslash_survives_the_round_trip() {
    use collapse_core::{compress_dir, Algorithm, Verify};

    for algorithm in [Algorithm::Zip, Algorithm::Tar, Algorithm::SevenZ] {
        let dir = tempfile::TempDir::new().unwrap();
        let tree = dir.path().join("tree");
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join(r"a\b.txt"), b"payload").unwrap();
        fs::write(tree.join("ok.txt"), b"fine").unwrap();

        let archive = dir.path().join(format!("t.{}", algorithm.extension()));
        compress_dir(&tree, &archive, algorithm, 3, Verify::Index).unwrap();

        let out = dir.path().join("back");
        let mut files = extract(&archive, &out).unwrap();
        files.sort();
        assert_eq!(
            files,
            vec![r"tree/a\b.txt".to_string(), "tree/ok.txt".to_string()],
            "{algorithm}: the archive this crate just built must extract"
        );
        assert_eq!(
            fs::read(out.join("tree").join(r"a\b.txt")).unwrap(),
            b"payload"
        );
    }
}

// ------------------------------------------ a listing that cannot be read ---

/// A tar whose entries are sound and whose end-of-archive marker is not.
///
/// This shape is the whole of issue #89: `list_tar_entries` walks every header
/// before extraction starts, while extraction writes as it walks, so the
/// listing dies on the damage after the extractor would already have written
/// everything before it.
fn tar_with_a_broken_tail(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
    let mut builder = Builder::new(Vec::new());
    for (name, content) in entries {
        let mut header = Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(EntryType::Regular);
        let raw = name.as_bytes();
        header.as_old_mut().name[..raw.len()].copy_from_slice(raw);
        header.set_cksum();
        builder.append(&header, *content).unwrap();
    }
    let mut bytes = builder.into_inner().unwrap();
    // Replace the two trailing zero blocks with something that is not a header.
    let tail = bytes.len() - 1024;
    for byte in bytes[tail..].iter_mut() {
        *byte = 0xAA;
    }
    let archive = dir.join("broken-tail.tar");
    fs::write(&archive, &bytes).unwrap();
    archive
}

/// Issue #89. An unreadable listing used to turn the entire naming layer off
/// and let every entry before the damage be written under its raw name.
///
/// On Windows `notes.txt:hidden` is not a file name: it is the `hidden`
/// alternate data stream of `notes.txt`, so the write succeeds, the bytes land
/// where no listing shows them, and the user was told there was nothing to
/// answer for. That is issue #63's harm performed without consent, and one bad
/// 512 byte header was enough to arrange it.
#[test]
fn a_damaged_archive_writes_nothing_rather_than_writing_raw_names() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = tar_with_a_broken_tail(
        dir.path(),
        &[("notes.txt:hidden", b"payload"), ("second.txt", b"more")],
    );
    let out = dir.path().join("out");

    let options = ExtractOptions::new().with_rules(NameRules::windows());
    let err = extract_with(&archive, &out, &options).unwrap_err();

    // The parser's own words, not a second-hand summary.
    assert!(err.to_string().starts_with("Compression failed:"), "{err}");
    // And nothing on disk. Before the fix both entries were here, the first as
    // an NTFS stream on a Windows host.
    let written: Vec<_> = fs::read_dir(&out)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        written.is_empty(),
        "a damaged archive wrote {written:?} before failing"
    );
}

/// The same archive with an intact tail is refused too, but for the reason the
/// user can act on. The pair is the point: an archive must not become *more*
/// permissive by being damaged.
#[test]
fn the_same_names_are_refused_whether_or_not_the_archive_is_damaged() {
    let dir = tempfile::TempDir::new().unwrap();
    let entries: &[(&str, &[u8])] = &[("notes.txt:hidden", b"payload"), ("second.txt", b"more")];
    let options = ExtractOptions::new().with_rules(NameRules::windows());

    let intact = archive_with(dir.path(), "tar", entries);
    let out = dir.path().join("intact");
    let err = extract_with(&intact, &out, &options).unwrap_err();
    assert!(
        matches!(err, CompressionError::Name(_)),
        "an intact archive must name the character to answer for: {err}"
    );
    assert!(!out.exists() || fs::read_dir(&out).unwrap().count() == 0);

    let damaged = tar_with_a_broken_tail(dir.path(), entries);
    let out = dir.path().join("damaged");
    assert!(extract_with(&damaged, &out, &options).is_err());
    assert!(!out.exists() || fs::read_dir(&out).unwrap().count() == 0);
}

/// Recovering what a damaged archive still holds is deliberately not gone, it
/// is just no longer what `extract` does by default.
///
/// The backends take no options, so they never go through the planning pass.
/// That is the escape hatch, and it is worth knowing it exists: without this
/// test the capability would look like it had been deleted.
#[test]
fn recovering_from_a_damaged_archive_is_still_possible_through_the_backend() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive =
        tar_with_a_broken_tail(dir.path(), &[("first.txt", b"one"), ("second.txt", b"two")]);
    let out = dir.path().join("salvage");

    // It still fails, on the damage, but only after handing back what preceded
    // it. `extract` writes nothing at all for the same archive.
    let _ = extract_tar(&archive, &out);
    let salvaged: Vec<_> = fs::read_dir(&out)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(salvaged.len(), 2, "the backend salvaged {salvaged:?}");
}

/// A refusal is now the whole of what the user gets for a damaged archive, so
/// the message has to be fit to read.
///
/// The tar crate embeds the bytes it choked on, so this one used to arrive as
/// "numeric field did not have utf-8 text: <four unprintable bytes> when
/// getting cksum for <a hundred more>". That was survivable while the listing
/// pass was advisory and the files came out anyway; it is not now.
#[test]
fn a_damaged_archive_says_so_in_words_a_person_can_read() {
    let dir = tempfile::TempDir::new().unwrap();
    let archive = tar_with_a_broken_tail(dir.path(), &[("a.txt", b"one")]);

    let err = extract(&archive, &dir.path().join("out")).unwrap_err();
    let message = err.to_string();

    assert!(
        message.contains("could not be read, so nothing was extracted"),
        "it must lead with what happened: {message}"
    );
    assert!(
        !message.contains(char::REPLACEMENT_CHARACTER),
        "the dependency's debris reached the user: {message}"
    );
    assert!(
        !message.chars().any(char::is_control),
        "control characters reached the user: {message}"
    );
    // The parser's own detail is kept, just cleaned up.
    assert!(message.contains("cksum"), "the detail was lost: {message}");
    // And it stays short enough to read in a terminal.
    assert!(
        message.chars().count() < 240,
        "{} chars",
        message.chars().count()
    );
}
