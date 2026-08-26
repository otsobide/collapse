// The wording and the small pure decisions behind the "names this computer
// cannot write" dialog, split out of App.vue so they can be unit-tested. The
// dialog itself cannot be seen on a Mac (macOS writes every one of these names
// happily), so this file plus tests/names.test.js is where its behaviour is
// actually verified.
//
// There are NO naming rules here. What a file name may contain is
// `collapse_core::NameRules`' answer and nobody else's: the `rejected` string
// this file checks against arrives from the Rust side, produced by the very
// ruleset the extraction will be judged by, and the extraction validates the
// answers again before it opens the archive. A second copy of the rules in
// JavaScript would be a copy that drifts.

/**
 * What a character becomes unless the user says otherwise.
 *
 * One character for every case, deliberately: `_` is what every tool that has
 * ever had to do this picks, it is writable everywhere, and it keeps the name
 * the same length so nothing looks like it went missing. Clearing the field is
 * how a user asks for the character to be dropped instead.
 */
export const DEFAULT_REPLACEMENT = '_'

/**
 * A character as it can be shown on screen.
 *
 * A control character has no glyph: rendering it raw would put an invisible
 * label on a text field and leave the user with nothing to read. Its code
 * point is the only honest thing to show.
 */
export function characterLabel(character) {
  const code = character.codePointAt(0)
  if (code < 0x20 || code === 0x7f) {
    return `U+${code.toString(16).toUpperCase().padStart(4, '0')}`
  }
  return character
}

/** How a character reads in prose: a space has to say so, or it is invisible. */
function named(character) {
  return character === ' ' ? 'a space' : `"${characterLabel(character)}"`
}

/**
 * Why this computer cannot keep a character, in one sentence.
 *
 * The two faults are genuinely different, and saying so is the point of the
 * split: a rejected character makes the write fail, while a reinterpreted one
 * makes it SUCCEED and put the bytes somewhere the user will never find them.
 * The second is the more alarming of the two and must not be described as if
 * it were a refusal.
 */
export function faultNote(fault) {
  if (fault === 'reinterpreted') {
    return 'This computer accepts it and then reads it as the start of a hidden stream, so the file would exist under no name at all.'
  }
  return 'This computer cannot write it in a file name.'
}

/**
 * The sentence for a problem that has no character to replace, or null when
 * the problem is a character (which gets a field instead of a sentence).
 *
 * These two are applied without asking, because there is nothing to ask: a
 * name the host would silently mangle is adjusted the way the host would have
 * adjusted it. Saying so is all the dialog can usefully do.
 */
export function adjustmentNote(problem, entry) {
  if (problem.kind === 'trailingCharacters') {
    const characters = [...new Set(problem.removed)].map(named).join(' and ')
    const them = problem.removed.length === 1 ? 'it' : 'them'
    return `"${entry}" ends in ${characters}, which this computer does not keep. The name is saved without ${them}.`
  }
  if (problem.kind === 'reservedDevice') {
    return `"${entry}" is the "${problem.device}" device in every folder, whatever follows the dot, so a "_" is added to it.`
  }
  return null
}

/**
 * Why a replacement will not do, or null when it is fine.
 *
 * `rejected` is the set the Rust side sent: every character this host cannot
 * write, plus the path separators. Membership is its answer; this function
 * only chooses the wording, so a rule that changes on the Rust side changes
 * here with it. An empty replacement is valid and means "drop the character".
 */
export function replacementError(replacement, rejected) {
  for (const character of replacement) {
    if (!rejected.includes(character)) continue
    if (character === '/' || character === '\\') {
      return `A replacement cannot contain ${named(character)}: it would move the file into another folder rather than rename it.`
    }
    return `${named(character)} is a character this computer cannot write in a file name either.`
  }
  return null
}

/** The prefilled answers for the characters an archive asks about. */
export function initialAnswers(characters) {
  const answers = {}
  for (const { character } of characters) answers[character] = DEFAULT_REPLACEMENT
  return answers
}

/**
 * The payload for `extract_archive`, built from the characters this archive
 * actually asks about.
 *
 * Driven by the character list rather than by the answers object, so an answer
 * left over from a previous archive cannot ride along into this extraction.
 */
export function substitutions(characters, answers) {
  const payload = {}
  for (const { character } of characters) payload[character] = answers[character] ?? ''
  return payload
}
