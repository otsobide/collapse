//! Which entry names the host can write, and what to do with the ones it
//! cannot.
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

use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

/// Characters Win32 refuses in a file name outright, minus the two separators.
///
/// `/` and `\` are on the documented list as well, and are deliberately absent
/// here: a [`NameRules`] judges one *component* of a path, and splitting a name
/// into components is the caller's job (extraction does it while checking for
/// traversal). Reporting a separator as an offending character would ask the
/// user to replace something that is not in any name we ever check.
const WINDOWS_REJECTED: &[char] = &['<', '>', '"', '|', '?', '*'];

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

/// What a reserved device name gains so it stops being one.
///
/// Appended to the part before the first dot, so `CON.txt` becomes `CON_.txt`
/// and keeps the extension that tells a person what the file is.
const DEVICE_SUFFIX: char = '_';

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

    /// Characters that need a replacement before a name carrying them can be
    /// written, in no particular order. Offered so a front end can explain the
    /// rules before it has an archive to complain about.
    pub fn offending_characters(&self) -> impl Iterator<Item = (char, CharacterFault)> + '_ {
        let rejected = self
            .rejected
            .iter()
            .map(|c| (*c, CharacterFault::Rejected))
            .chain(
                self.reinterpreted
                    .iter()
                    .map(|c| (*c, CharacterFault::Reinterpreted)),
            );
        let controls = self
            .rejects_control_characters
            .then_some('\u{0}'..='\u{1f}')
            .into_iter()
            .flatten()
            .map(|c| (c, CharacterFault::Rejected));
        rejected.chain(controls)
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
    /// Components that are not `Normal` (a root, a drive, `.`, `..`) are
    /// skipped. They are containment's business, not this module's, and
    /// extraction settles them before it gets here.
    pub fn entry_problems(&self, name: &str) -> Vec<NameProblem> {
        let mut problems: Vec<NameProblem> = Vec::new();
        for component in normal_components(name) {
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

    /// The name this filesystem would be given for **one component**, applying
    /// the caller's replacements and the two adjustments that need no answer.
    ///
    /// The order matters and is not arbitrary:
    ///
    /// 1. every offending character is replaced, because a replacement can
    ///    create or remove either of the problems below (`CO?1` answered with
    ///    `M` is `COM1`, a device that was not there before);
    /// 2. trailing dots and spaces go, which is what the host would silently do
    ///    to the name anyway;
    /// 3. a reserved device name gains [`DEVICE_SUFFIX`].
    pub fn rewrite(
        &self,
        component: &str,
        replacements: &Substitutions,
    ) -> Result<String, NameError> {
        let mut written = String::with_capacity(component.len());
        for character in component.chars() {
            if self.fault_of(character).is_none() {
                written.push(character);
                continue;
            }
            let replacement =
                replacements
                    .get(character)
                    .ok_or_else(|| NameError::NoReplacement {
                        entry: component.to_string(),
                        character,
                    })?;
            self.check_replacement(character, replacement)?;
            written.push_str(replacement);
        }

        let trailing = self.trailing_run(&written).len();
        written.truncate(written.len() - trailing);

        // The offset of the first dot, which is where the device name ends.
        let device_ends = self.reserved_device(&written).map(str::len);
        if let Some(at) = device_ends {
            written.insert(at, DEVICE_SUFFIX);
        }

        // Everything below is defence in depth against a replacement that turns
        // a name into something that is not a name: `??` answered with `.` is
        // `..`, which would climb out of the output directory, and an empty
        // answer can leave nothing at all. A caller cannot reach the write path
        // without coming through here.
        let unnameable = written.is_empty()
            || written == "."
            || written == ".."
            || written.contains('/')
            || written.contains('\\')
            || !self.can_write(&written);
        if unnameable {
            return Err(NameError::Unnameable {
                entry: component.to_string(),
                component: component.to_string(),
                result: written,
            });
        }
        Ok(written)
    }

    /// [`Self::rewrite`] over a whole entry name, rebuilt as a relative path.
    ///
    /// **Not a traversal guard**: components that are not `Normal` are dropped,
    /// exactly as `unpack_in` drops a root and as `sanitize_entry_path` reduces
    /// a name to what is left. Callers check containment first; all three
    /// extractors in this crate do.
    pub fn rewrite_entry(
        &self,
        name: &str,
        replacements: &Substitutions,
    ) -> Result<PathBuf, NameError> {
        let mut written = PathBuf::new();
        for component in normal_components(name) {
            written.push(
                self.rewrite(component, replacements)
                    .map_err(|e| e.in_entry(name))?,
            );
        }
        Ok(written)
    }

    /// Refuse a replacement this filesystem could not write either, before an
    /// archive is opened and before anything is on disk.
    ///
    /// An empty replacement is fine, and means "drop the character".
    pub fn check_replacements(&self, replacements: &Substitutions) -> Result<(), NameError> {
        for (character, replacement) in replacements.pairs() {
            self.check_replacement(character, replacement)?;
        }
        Ok(())
    }

    fn check_replacement(&self, character: char, replacement: &str) -> Result<(), NameError> {
        for candidate in replacement.chars() {
            // Checked before the ruleset, because neither ruleset lists the
            // separators (see WINDOWS_REJECTED) and this is the one that would
            // be a traversal rather than an unreadable name: `?` answered with
            // `../` moves the entry to another directory entirely.
            if candidate == '/' || candidate == '\\' {
                return Err(NameError::SeparatorInReplacement {
                    character,
                    replacement: replacement.to_string(),
                });
            }
            if self.fault_of(candidate).is_some() {
                return Err(NameError::UnwritableReplacement {
                    character,
                    replacement: replacement.to_string(),
                    offending: candidate,
                });
            }
        }
        Ok(())
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

impl NameProblem {
    /// The character a caller has to supply a replacement for, or `None` when
    /// the problem is adjusted automatically.
    pub fn replaceable(&self) -> Option<char> {
        match self {
            Self::Character { character, .. } => Some(*character),
            Self::TrailingCharacters { .. } | Self::ReservedDevice { .. } => None,
        }
    }
}

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

/// The caller's answers: what to write in place of each character the host
/// refuses.
///
/// A replacement may be empty, which drops the character. It is validated
/// against the same rules ([`NameRules::check_replacements`]), because "replace
/// `?` with `*`" is not an answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Substitutions {
    /// Ordered so that a caller who gives two bad answers is told about the
    /// same one every run.
    by_character: BTreeMap<char, String>,
}

impl Substitutions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.by_character.is_empty()
    }

    /// Answer for one character. A later answer replaces an earlier one.
    pub fn set(&mut self, character: char, replacement: impl Into<String>) {
        self.by_character.insert(character, replacement.into());
    }

    /// [`Self::set`] for a key that arrived as a string, which is how it
    /// crosses a UI boundary (a JSON object has no char keys). Both front ends
    /// need this, so it lives here rather than twice.
    pub fn set_str(&mut self, key: &str, replacement: impl Into<String>) -> Result<(), NameError> {
        let mut characters = key.chars();
        match (characters.next(), characters.next()) {
            (Some(character), None) => {
                self.set(character, replacement);
                Ok(())
            }
            _ => Err(NameError::NotOneCharacter {
                key: key.to_string(),
            }),
        }
    }

    /// Builder form, for a caller that has its answers to hand.
    pub fn with(mut self, character: char, replacement: impl Into<String>) -> Self {
        self.set(character, replacement);
        self
    }

    pub fn get(&self, character: char) -> Option<&str> {
        self.by_character.get(&character).map(String::as_str)
    }

    pub fn pairs(&self) -> impl Iterator<Item = (char, &str)> {
        self.by_character.iter().map(|(c, r)| (*c, r.as_str()))
    }
}

impl FromIterator<(char, String)> for Substitutions {
    fn from_iter<T: IntoIterator<Item = (char, String)>>(pairs: T) -> Self {
        Self {
            by_character: pairs.into_iter().collect(),
        }
    }
}

/// A name that cannot be written, or an answer that does not help.
///
/// Separate from the IO and format errors because every one of these is
/// actionable by the person in front of the screen: each names the entry, and
/// says what would fix it.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NameError {
    #[error(
        "the archive entry {entry:?} contains {character:?}, which this system cannot write in a \
         file name, and no replacement for it was given"
    )]
    NoReplacement { entry: String, character: char },

    #[error(
        "{replacement:?} cannot replace {character:?}: this system cannot write {offending:?} in a \
         file name either"
    )]
    UnwritableReplacement {
        character: char,
        replacement: String,
        offending: char,
    },

    #[error(
        "{replacement:?} cannot replace {character:?}: a replacement may not contain a path \
         separator, which would move the entry to another directory"
    )]
    SeparatorInReplacement {
        character: char,
        replacement: String,
    },

    #[error(
        "the archive entries {first:?} and {second:?} would both be written as {name:?}; choose a \
         replacement that keeps them apart"
    )]
    Collision {
        first: String,
        second: String,
        name: String,
    },

    #[error(
        "the archive entry {entry:?} cannot be written: {component:?} becomes {result:?}, which is \
         not a name this system can hold"
    )]
    Unnameable {
        entry: String,
        component: String,
        result: String,
    },

    #[error("{key:?} is not a single character, so there is nothing to replace")]
    NotOneCharacter { key: String },
}

impl NameError {
    /// Re-point an error raised over one component at the whole entry, which is
    /// the only name the person reading it has ever seen.
    fn in_entry(self, entry: &str) -> Self {
        match self {
            Self::NoReplacement { character, .. } => Self::NoReplacement {
                entry: entry.to_string(),
                character,
            },
            Self::Unnameable {
                component, result, ..
            } => Self::Unnameable {
                entry: entry.to_string(),
                component,
                result,
            },
            other => other,
        }
    }
}

/// The name every entry will be written under, for the entries whose name
/// changes.
///
/// Built from the whole listing before anything is written, because two of the
/// three answers it can give ("no replacement for this" and "these two entries
/// collide") must stop the extraction while the output directory is still
/// empty, and a collision cannot be seen one entry at a time.
#[derive(Debug, Clone, Default)]
pub(crate) struct NamePlan {
    /// Only the entries whose name changed. An entry that is absent is written
    /// under the name the extractor derived for it, which is what this would
    /// have stored anyway.
    rewritten: HashMap<String, PathBuf>,
}

impl NamePlan {
    /// The plan that changes nothing, for [`super::extract_zip`] and the other
    /// backends called directly with no options.
    pub(crate) fn identity() -> Self {
        Self::default()
    }

    pub(crate) fn written_as(&self, entry: &str) -> Option<&Path> {
        self.rewritten.get(entry).map(PathBuf::as_path)
    }
}

/// Work out what each name in `names` becomes, and refuse the two situations
/// nothing downstream could recover from.
pub(crate) fn plan_names<S: AsRef<str>>(
    names: &[S],
    rules: NameRules,
    replacements: &Substitutions,
) -> Result<NamePlan, NameError> {
    let mut rewritten = HashMap::new();
    // planned name -> the first entry that claimed it, and whether that entry's
    // name had to change to claim it.
    let mut claimed: HashMap<PathBuf, (&str, bool)> = HashMap::new();

    for name in names {
        let name = name.as_ref();
        let planned = rules.rewrite_entry(name, replacements)?;
        if planned.as_os_str().is_empty() {
            // Nothing normal in it (`.`, a bare root). Containment decides what
            // becomes of those, per format, and it is not this pass's business.
            continue;
        }
        let changed = planned != natural_path(name);

        if let Some((first, first_changed)) = claimed.get(&planned) {
            // Two entries spelled the same way is an archive that was already
            // like that, and extraction has always let the second win. Refusing
            // it here would start rejecting archives that have nothing to do
            // with this feature; a collision is only ours when a rewrite caused
            // it.
            if *first != name && (changed || *first_changed) {
                return Err(NameError::Collision {
                    first: (*first).to_string(),
                    second: name.to_string(),
                    name: planned.to_string_lossy().into_owned(),
                });
            }
        }
        claimed.insert(planned.clone(), (name, changed));
        if changed {
            rewritten.insert(name.to_string(), planned);
        }
    }

    Ok(NamePlan { rewritten })
}

/// The relative path an extractor derives from an entry name with no rules
/// applied: its `Normal` components and nothing else.
fn natural_path(name: &str) -> PathBuf {
    normal_components(name).collect()
}

fn normal_components(name: &str) -> impl Iterator<Item = &str> {
    Path::new(name).components().filter_map(|c| match c {
        // A component of a `&str` path is always valid UTF-8, so `to_str` never
        // drops one here.
        Component::Normal(part) => part.to_str(),
        _ => None,
    })
}
