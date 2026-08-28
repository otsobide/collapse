//! Which entry names the host can write, and which ones stop an extraction.
//!
//! Extraction takes names from an archive, which is to say from another
//! machine. Containment ([`sanitize_entry_path`](super::sanitize_entry_path))
//! asks whether a name would escape the output directory; this module asks the
//! different question of whether the host can hold the name at all. On Windows
//! an ordinary Unix name often cannot be held: `what?.txt` is refused outright,
//! `notes.txt.` is not preserved, `CON` resolves to a device in every
//! directory, and `notes.txt:hidden` is accepted as the `hidden` stream of
//! `notes.txt` rather than as a file, with no error at all.
//!
//! **The answer to all four is the same: refuse the archive.** This module used
//! to negotiate instead. It asked the user to supply a character in place of
//! the `?`, dropped the trailing dot on its own and suffixed the device name,
//! then extracted under the names it had arrived at. The output was then a tree
//! whose names were this crate's invention rather than the archive's, with no
//! way for anything downstream to tell which files those were. An extraction
//! that cannot reproduce what the archive says is one that should not happen,
//! so [`refuse_unwritable_names`] stops it before the first byte and says which
//! entry and why. The machinery for the old answer is still here and unused,
//! because the front ends still hand it answers; see
//! [`super::ExtractOptions::with_replacements`].
//!
//! The rules are **data** ([`NameRules`]) rather than `#[cfg]`, and that is the
//! point: [`NameRules::windows`] can be asked for from any machine, so every
//! rule here is exercised on a Mac and on Linux CI as well as on the Windows
//! leg. A rule reachable only under `#[cfg(windows)]` is a rule this repository
//! cannot test, and that habit is what left a data-loss guard broken on Windows
//! for months.
//!
//! Nothing here touches the filesystem: these are string rules, and the reading
//! and writing all happen in [`super`].
//!
//! The Windows rules follow "Naming Files, Paths, and Namespaces"
//! (learn.microsoft.com/windows/win32/fileio/naming-a-file), read rather than
//! remembered, which is where the two easily-missed details come from: a
//! reserved device name is reserved *with* an extension too (`NUL.tar.gz` is
//! `NUL`), and the superscript digits `¹²³` count as digits in `COM#`/`LPT#`.

use serde::Serialize;
use thiserror::Error;

/// Characters Win32 refuses in a file name outright, minus the one separator.
///
/// `/` is on the documented list as well and is deliberately absent here: it is
/// the separator **the archive formats define** (ZIP APPNOTE 4.4.17.1, and tar
/// by convention), so [`entry_components`] has already split on it and no
/// component reaching a [`NameRules`] can contain one. Reporting it would ask
/// the user to replace something that is not in any name we ever check.
///
/// `\` is a different case and belongs here. It is *not* an archive separator,
/// so it arrives as an ordinary character inside a component, and Windows
/// genuinely cannot hold it: there it is a path separator, which is precisely
/// why the component cannot carry one. Unix can, and does (see [`UNIX_REJECTED`]),
/// which is the whole reason this is a rule and not a constant.
const WINDOWS_REJECTED: &[char] = &['<', '>', '"', '|', '?', '*', '\\'];

/// The colon is not refused by Win32, it is *honoured*: `notes.txt:hidden` names
/// the `hidden` alternate data stream of `notes.txt`, so the write succeeds, the
/// bytes go somewhere invisible, and the listing names a file that exists
/// nowhere. That silence is why it is a fault of its own rather than one more
/// rejected character (issue #63).
const WINDOWS_REINTERPRETED: &[char] = &[':'];

/// "Do not end a file or directory name with a space or a period."
const WINDOWS_TRAILING: &[char] = &['.', ' '];

/// A NUL byte cannot cross the libc boundary, so no Unix filesystem can hold
/// one; `std` answers `InvalidInput` before the kernel is ever asked. It is the
/// only character that fails here, which is why this whole feature is a Windows
/// feature in practice.
const UNIX_REJECTED: &[char] = &['\0'];

const NO_CHARS: &[char] = &[];

/// Device names that resolve in every directory, whatever the extension.
const DEVICE_NAMES: &[&str] = &["CON", "PRN", "AUX", "NUL"];

/// `COM#` and `LPT#` are devices too, for a single digit.
const DEVICE_PREFIXES: &[&str] = &["COM", "LPT"];

/// The digits `COM#`/`LPT#` accept. The superscripts are not decoration:
/// Windows reads the ISO 8859-1 `¹`, `²` and `³` as digits, so `COM¹` is as
/// reserved as `COM1`.
const DEVICE_DIGITS: &[char] = &['1', '2', '3', '4', '5', '6', '7', '8', '9', '¹', '²', '³'];

/// What a filesystem will accept as the name of an ordinary file.
///
/// Copy this rather than reaching for `#[cfg]`: [`Self::host`] is what
/// extraction uses, and [`Self::windows`] is what makes the Windows behaviour
/// testable from a machine that is not Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameRules {
    rejected: &'static [char],
    reinterpreted: &'static [char],
    /// Whether U+0000 to U+001F are refused (Win32 refuses all of them in a
    /// file name, and allows them only inside a stream name).
    rejects_control_characters: bool,
    trailing: &'static [char],
    devices: bool,
}

impl NameRules {
    /// The rules of the machine this build runs on.
    pub const fn host() -> Self {
        #[cfg(windows)]
        {
            Self::windows()
        }
        #[cfg(not(windows))]
        {
            Self::unix()
        }
    }

    /// What Windows can write, askable from anywhere.
    pub const fn windows() -> Self {
        Self {
            rejected: WINDOWS_REJECTED,
            reinterpreted: WINDOWS_REINTERPRETED,
            rejects_control_characters: true,
            trailing: WINDOWS_TRAILING,
            devices: true,
        }
    }

    /// What Unix can write, which is very nearly everything.
    pub const fn unix() -> Self {
        Self {
            rejected: UNIX_REJECTED,
            reinterpreted: NO_CHARS,
            rejects_control_characters: false,
            trailing: NO_CHARS,
            devices: false,
        }
    }

    /// Everything about **one component** of a name that this filesystem cannot
    /// hold. An empty answer means the component is writable as it stands.
    ///
    /// One problem per distinct offending character, in the order they first
    /// appear, so a caller can ask about each exactly once.
    pub fn problems(&self, component: &str) -> Vec<NameProblem> {
        let mut problems: Vec<NameProblem> = Vec::new();
        for character in component.chars() {
            let Some(fault) = self.fault_of(character) else {
                continue;
            };
            let problem = NameProblem::Character { character, fault };
            if !problems.contains(&problem) {
                problems.push(problem);
            }
        }
        let trailing = self.trailing_run(component);
        if !trailing.is_empty() {
            problems.push(NameProblem::TrailingCharacters {
                removed: trailing.to_string(),
            });
        }
        if let Some(device) = self.reserved_device(component) {
            problems.push(NameProblem::ReservedDevice {
                device: device.to_string(),
            });
        }
        problems
    }

    /// [`Self::problems`] over every component of an entry name, deduplicated:
    /// a `?` in two components is one question, not two.
    ///
    /// Split on `/` by [`entry_components`], so the answer does not depend on
    /// which machine is asking. Empty components, `.` and `..` are skipped:
    /// they are containment's business, not this module's, and extraction
    /// settles them before it gets here.
    pub fn entry_problems(&self, name: &str) -> Vec<NameProblem> {
        let mut problems: Vec<NameProblem> = Vec::new();
        for component in entry_components(name) {
            for problem in self.problems(component) {
                if !problems.contains(&problem) {
                    problems.push(problem);
                }
            }
        }
        problems
    }

    /// True when this filesystem can hold `component` exactly as spelled.
    pub fn can_write(&self, component: &str) -> bool {
        self.problems(component).is_empty()
    }

    fn fault_of(&self, character: char) -> Option<CharacterFault> {
        if self.reinterpreted.contains(&character) {
            return Some(CharacterFault::Reinterpreted);
        }
        if self.rejected.contains(&character) {
            return Some(CharacterFault::Rejected);
        }
        if self.rejects_control_characters && character <= '\u{1f}' {
            return Some(CharacterFault::Rejected);
        }
        None
    }

    /// The run of characters at the end of `component` that this filesystem
    /// would not keep, as a slice of it.
    fn trailing_run<'a>(&self, component: &'a str) -> &'a str {
        let kept = component.trim_end_matches(|c| self.trailing.contains(&c));
        &component[kept.len()..]
    }

    /// The device this component resolves to, if it resolves to one: the part
    /// before the **first** dot, since the docs make `NUL.tar.gz` equivalent to
    /// `NUL`, and matched case insensitively.
    ///
    /// Returned as a slice of `component` so a caller knows where the name ends
    /// and the extension begins.
    fn reserved_device<'a>(&self, component: &'a str) -> Option<&'a str> {
        if !self.devices {
            return None;
        }
        let stem = component.split('.').next().unwrap_or(component);
        // Win32 drops trailing spaces before it resolves a name, so `CON ` is
        // the console as much as `CON` is.
        let named = stem.trim_end_matches(' ');
        let upper = named.to_ascii_uppercase();
        if DEVICE_NAMES.contains(&upper.as_str()) {
            return Some(stem);
        }
        for prefix in DEVICE_PREFIXES {
            let Some(rest) = upper.strip_prefix(prefix) else {
                continue;
            };
            let mut digits = rest.chars();
            match (digits.next(), digits.next()) {
                (Some(digit), None) if DEVICE_DIGITS.contains(&digit) => return Some(stem),
                _ => {}
            }
        }
        None
    }
}

impl Default for NameRules {
    fn default() -> Self {
        Self::host()
    }
}

/// Why a filesystem cannot hold a name, as data a UI can render.
///
/// The split that matters to a front end is [`Self::replaceable`]: a character
/// is a question for the user, while the other two are adjustments that need no
/// answer and only need explaining.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NameProblem {
    /// A character the filesystem will not take, or will take and read as
    /// something other than part of the name.
    Character {
        character: char,
        fault: CharacterFault,
    },
    /// The name ends in characters the filesystem does not preserve. Removed
    /// automatically: they are what the host would have dropped anyway.
    TrailingCharacters { removed: String },
    /// The name resolves to a device rather than to a file, in any directory.
    /// A `_` is appended to it automatically, before the extension.
    ReservedDevice { device: String },
}

impl NameProblem {}

/// How a filesystem gets a character in a name wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CharacterFault {
    /// The write fails. Loud, and the easy case.
    Rejected,
    /// The write succeeds and does something else: on Windows `:` opens an
    /// alternate data stream, so the bytes attach to another file and the name
    /// exists as no file at all.
    Reinterpreted,
}

/// One entry an archive holds that the filesystem cannot write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnwritableEntry {
    /// The name exactly as the archive spells it.
    pub entry: String,
    /// Every reason it cannot be written, deduplicated across its components.
    pub problems: Vec<NameProblem>,
}

/// One character to ask the user about, and how much of the archive it holds up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OffendingCharacter {
    pub character: char,
    pub fault: CharacterFault,
    /// How many entries carry it. A UI can say "3 files" beside the field.
    pub entries: usize,
}

/// What an archive holds that this filesystem cannot write, gathered from its
/// listing alone.
///
/// Two views of the same thing, because a UI needs both: [`Self::entries`] to
/// show what is affected, and [`Self::characters`] to put one text field per
/// character on screen rather than one per file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NameReport {
    pub entries: Vec<UnwritableEntry>,
    pub characters: Vec<OffendingCharacter>,
}

impl NameReport {
    /// Judge a listing. Pure: no archive is opened here.
    pub fn of<S: AsRef<str>>(names: &[S], rules: NameRules) -> Self {
        let mut entries = Vec::new();
        let mut characters: Vec<OffendingCharacter> = Vec::new();
        for name in names {
            let name = name.as_ref();
            let problems = rules.entry_problems(name);
            if problems.is_empty() {
                continue;
            }
            for problem in &problems {
                let NameProblem::Character { character, fault } = problem else {
                    continue;
                };
                match characters.iter_mut().find(|c| c.character == *character) {
                    Some(known) => known.entries += 1,
                    None => characters.push(OffendingCharacter {
                        character: *character,
                        fault: *fault,
                        entries: 1,
                    }),
                }
            }
            entries.push(UnwritableEntry {
                entry: name.to_string(),
                problems,
            });
        }
        Self {
            entries,
            characters,
        }
    }

    /// True when every name in the archive can be written as it stands.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A name that cannot be written, or an answer that does not help.
///
/// Separate from the IO and format errors because every one of these is
/// actionable by the person in front of the screen: each names the entry, and
/// says what would fix it.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NameError {
    /// The host cannot hold this name exactly as the archive spells it.
    ///
    /// The whole extraction stops here. Extraction writes what the archive
    /// says or it writes nothing: a name this filesystem would refuse, or
    /// silently store under a different name, is reported rather than adjusted,
    /// because the adjusted file is not the file the archive named and nothing
    /// downstream can tell the difference afterwards.
    #[error(
        "the archive entry {entry:?} cannot be written on this system: {component:?} {}. \
         Nothing was extracted; extract it on a system that can hold the name.",
        describe(.problems)
    )]
    Unwritable {
        entry: String,
        component: String,
        problems: Vec<NameProblem>,
    },
}

/// Why one component cannot be written, as a phrase that follows its name.
///
/// Rendered here rather than on [`NameProblem`] itself: this is the sentence a
/// refusal reads in, and the same data has to render differently in the desktop
/// dialog, which shows it as structure rather than as prose.
fn describe(problems: &[NameProblem]) -> String {
    problems
        .iter()
        .map(|problem| match problem {
            NameProblem::Character {
                character,
                fault: CharacterFault::Rejected,
            } => format!("contains {character:?}, which this system refuses in a file name"),
            NameProblem::Character {
                character,
                fault: CharacterFault::Reinterpreted,
            } => format!(
                "contains {character:?}, which this system reads as something other than part \
                 of the name"
            ),
            NameProblem::TrailingCharacters { removed } => {
                format!("ends in {removed:?}, which this system does not preserve")
            }
            NameProblem::ReservedDevice { device } => {
                format!("is the reserved device name {device:?}")
            }
        })
        .collect::<Vec<_>>()
        .join("; and ")
}

/// Refuse a listing holding a name this filesystem cannot write exactly as the
/// archive spells it.
///
/// **Nothing is renamed.** This used to be a planning pass: it applied the
/// caller's replacements, dropped a trailing dot or space and suffixed a
/// reserved device, so an archive Windows could not hold was extracted anyway
/// under names it could. That traded one problem for a worse one. The file on
/// disk was then not the file the archive named, nothing downstream could tell
/// the two apart, and the listing handed back was the only record that a
/// substitution had happened at all. An extraction either reproduces what the
/// archive says or it does not happen.
///
/// So the question is now the narrow one [`NameRules::can_write`] answers, and
/// the answer is yes or no rather than a rewrite: can this host hold this
/// component, spelled this way. A component it would refuse (`what?.txt` on
/// Windows), silently store elsewhere (`notes.txt:hidden`, an NTFS stream),
/// silently rename (`notes.txt.`, whose trailing dot is dropped) or resolve to
/// a device (`CON`) all fail the same way, because from the caller's side they
/// are the same failure: the name they asked for is not the name they would
/// get.
///
/// The whole listing is judged before a byte is written, so a refusal leaves
/// the output directory exactly as it found it.
///
/// The first offending component stops it. Reporting every one at once is a
/// front end's job, and both have the listing to do it from
/// ([`NameReport::of`]), which is also what lets them ask before extracting
/// rather than after failing.
pub(crate) fn refuse_unwritable_names<S: AsRef<str>>(
    names: &[S],
    rules: NameRules,
) -> Result<(), NameError> {
    for name in names {
        let name = name.as_ref();
        // Component by component, and split on `/` rather than by the host's
        // rules: an archive entry name means the same thing on every machine,
        // which is the property `entry_components` exists to preserve.
        for component in entry_components(name) {
            let problems = rules.problems(component);
            if !problems.is_empty() {
                return Err(NameError::Unwritable {
                    entry: name.to_string(),
                    component: component.to_string(),
                    problems,
                });
            }
        }
    }
    Ok(())
}

/// Split an archive entry name into its components, the same way on every host.
///
/// **The separator is `/`, always.** An archive entry name is not a host path:
/// ZIP mandates the forward slash (APPNOTE 4.4.17.1) and tar has used it since
/// v7, so an entry means the same thing whatever machine reads it, and this
/// module has to agree with that rather than with the local convention.
///
/// It used to be `Path::new(name).components()`, and that was wrong in both
/// directions at once, because `std::path` is `#[cfg]`-dependent while
/// [`NameRules`] is data:
///
/// * on Windows, `\` is a separator, so `dir\file.txt` split into two
///   components; on Unix it is an ordinary character, so the same entry was one
///   component that [`NameRules::rewrite`] then refused, and a legal Unix file
///   name became an archive this crate could build but not extract;
/// * on Windows, a leading `a:` parses as a drive prefix, and a prefix is not a
///   `Normal` component, so it was silently *discarded*: `a:b/c.txt` was judged,
///   reported and written as `b/c.txt`. That hole was in the colon handling that
///   issue #63 exists for, on the only platform issue #63 is about.
///
/// Splitting here instead means `NameRules::windows()` answers the same question
/// on a Mac as on Windows, which is the property the whole design rests on.
///
/// Empty components, `.` and `..` are skipped, exactly as the `Component` filter
/// skipped everything that was not `Normal`. **This is not a traversal guard**:
/// containment is [`super::sanitize_entry_path`]'s job and it rejects, rather
/// than skips, the same input.
fn entry_components(name: &str) -> impl Iterator<Item = &str> {
    name.split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
}
