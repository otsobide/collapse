//! The naming question, on its way to the webview and back.
//!
//! Extraction can no longer be a single call. An archive built on Linux may
//! hold names this machine cannot save as they are spelled, and issues #63 and
//! #64 settled what to do about it: ask. So the UI first asks what is wrong
//! ([`NameInspection`], from `collapse_core::unwritable_names_with`), puts one
//! text field on screen per offending character, and only then extracts with
//! the answers.
//!
//! This module is the shape of that exchange plus the two conversions it needs.
//! It deliberately holds **no rules of its own**: every judgement about what a
//! name may contain belongs to `collapse_core::NameRules`, and duplicating any
//! part of it here is how the webview and the extractor would come to disagree
//! about the same name. What crosses to the webview is the *ruleset's own
//! answer*, as data ([`NameInspection::rejected_in_replacement`]), so the
//! dialog can refuse a bad answer as it is typed without knowing why it is bad.

use std::collections::BTreeMap;

use collapse_core::{NameError, NameReport, NameRules, Substitutions};
use serde::Serialize;

/// Path separators, which no replacement may contain.
///
/// Neither ruleset lists them (a `NameRules` judges one component of a path, so
/// a separator is never *in* a name it is asked about), yet
/// `NameRules::check_replacements` refuses them, because answering `?` with
/// `../` would move the entry to another directory rather than rename it. They
/// are therefore added to the set the dialog checks against, or the dialog
/// would accept an answer the extractor is about to reject.
const SEPARATORS: [char; 2] = ['/', '\\'];

/// What an archive holds that this machine cannot write, and what the dialog
/// needs to ask about it.
///
/// The report is flattened, so the webview sees one flat object
/// (`{ entries, characters, rejectedInReplacement }`) rather than a report
/// nested inside a wrapper: the JSON is the dialog's data model, and it has no
/// use for the seam between the two halves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NameInspection {
    #[serde(flatten)]
    pub report: NameReport,
    /// Every character a replacement may not contain: everything this machine
    /// cannot write, plus the two path separators.
    ///
    /// Sent as data rather than reimplemented in JavaScript. The dialog only
    /// picks the wording; whether a character is acceptable is answered here by
    /// the same ruleset the extraction will use.
    ///
    /// Nothing downstream re-checks it any more, and nothing needs to: core no
    /// longer substitutes anything, so an answer cannot reach a file name. This
    /// whole exchange is inert until the dialog is taken out.
    pub rejected_in_replacement: String,
}

impl NameInspection {
    pub fn new(report: NameReport, rules: NameRules) -> Self {
        let mut rejected: Vec<char> = rules
            .offending_characters()
            .map(|(character, _)| character)
            .chain(SEPARATORS)
            .collect();
        // Sorted and deduplicated so the string is the same on every run: it is
        // pinned by a test, and a set that arrived in a different order each
        // time would be untestable as well as unreadable.
        rejected.sort_unstable();
        rejected.dedup();
        Self {
            report,
            rejected_in_replacement: rejected.into_iter().collect(),
        }
    }

    /// True when every name in the archive can be written as it stands, which
    /// is when the dialog has nothing to ask.
    pub fn is_empty(&self) -> bool {
        self.report.is_empty()
    }
}

/// The webview's answers, turned into what core takes.
///
/// A JSON object has no `char` keys, so each one arrives as a string and is
/// checked for being exactly one character. That check is core's
/// ([`Substitutions::set_str`]), not a second copy of it here.
pub fn substitutions_from(
    replacements: &BTreeMap<String, String>,
) -> Result<Substitutions, NameError> {
    let mut answers = Substitutions::new();
    for (key, replacement) in replacements {
        answers.set_str(key, replacement.as_str())?;
    }
    Ok(answers)
}
