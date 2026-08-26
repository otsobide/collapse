import { describe, it, expect } from 'vitest'
import {
  DEFAULT_REPLACEMENT,
  adjustmentNote,
  characterLabel,
  faultNote,
  initialAnswers,
  replacementError,
  substitutions,
} from '../src/names.js'

// The Windows set, which is the only one that makes this dialog appear in
// practice, plus the two separators. It arrives from the Rust side as
// `NameInspection.rejectedInReplacement`; spelled out here so these cases read
// on their own.
const WINDOWS_REJECTED = '<>"|?*:/\\'

describe('characterLabel', () => {
  it('shows an ordinary character as itself', () => {
    for (const character of ['?', '*', ':', '<', '|', ' ']) {
      expect(characterLabel(character)).toBe(character)
    }
  })

  it('names a control character by its code point', () => {
    // A raw control character renders as nothing at all, so a field labelled
    // with one asks the user about a blank. They do reach this dialog: the NUL
    // is the single character Unix itself refuses, and Windows refuses every
    // one of U+0000 to U+001F. (U+007F is labelled too, for the same reason,
    // though no ruleset here refuses it today.)
    expect(characterLabel('\u0000')).toBe('U+0000')
    expect(characterLabel('\u0007')).toBe('U+0007')
    expect(characterLabel('\u001F')).toBe('U+001F')
    expect(characterLabel('\u007F')).toBe('U+007F')
  })
})

describe('faultNote', () => {
  it('says a rejected character makes the write fail', () => {
    expect(faultNote('rejected')).toContain('cannot write it')
  })

  it('says a reinterpreted character makes the file disappear instead', () => {
    // The colon is the dangerous one precisely because Windows ACCEPTS it: the
    // write succeeds and the bytes land in an alternate data stream. Wording
    // this as another refusal would describe the wrong thing entirely.
    const note = faultNote('reinterpreted')
    expect(note).toContain('accepts it')
    expect(note).toContain('hidden stream')
    expect(note).not.toContain('cannot write')
  })
})

describe('adjustmentNote', () => {
  it('has nothing to say about a character, which gets a field instead', () => {
    expect(
      adjustmentNote({ kind: 'character', character: '?', fault: 'rejected' }, 'what?.txt')
    ).toBeNull()
  })

  it('names the entry and the trailing characters that will go', () => {
    const note = adjustmentNote({ kind: 'trailingCharacters', removed: '.' }, 'notes.txt.')
    expect(note).toBe(
      '"notes.txt." ends in ".", which this computer does not keep. The name is saved without it.'
    )
  })

  it('calls a trailing space a space, since it cannot be seen', () => {
    // `"x " ends in " "` is a sentence with a hole in it: the quotes are all
    // the reader gets.
    const note = adjustmentNote({ kind: 'trailingCharacters', removed: ' ' }, 'draft ')
    expect(note).toContain('ends in a space')
    expect(note).toContain('without it')
  })

  it('describes a run of several trailing characters once each', () => {
    const note = adjustmentNote({ kind: 'trailingCharacters', removed: '. ..' }, 'odd. ..')
    expect(note).toContain('ends in "." and a space')
    expect(note).toContain('without them')
  })

  it('explains a device name without promising a name it has not computed', () => {
    const note = adjustmentNote({ kind: 'reservedDevice', device: 'CON' }, 'CON.txt')
    expect(note).toContain('"CON.txt" is the "CON" device in every folder')
    expect(note).toContain('whatever follows the dot')
  })
})

describe('replacementError', () => {
  it('accepts an ordinary replacement', () => {
    for (const replacement of ['_', '-', 'at', '·', '']) {
      expect(replacementError(replacement, WINDOWS_REJECTED)).toBeNull()
    }
  })

  it('accepts an empty replacement, which drops the character', () => {
    expect(replacementError('', WINDOWS_REJECTED)).toBeNull()
  })

  it('refuses a replacement the host cannot write either', () => {
    // "Replace ? with *" is not an answer. Caught here so the user is told
    // while typing rather than after a round trip.
    const problem = replacementError('*', WINDOWS_REJECTED)
    expect(problem).toContain('"*"')
    expect(problem).toContain('cannot write in a file name either')
  })

  it('refuses a separator for the reason that makes it worse than unwritable', () => {
    for (const replacement of ['../', 'a\\b']) {
      const problem = replacementError(replacement, WINDOWS_REJECTED)
      expect(problem).toContain('move the file into another folder')
    }
  })

  it('checks every character of a longer replacement, not only the first', () => {
    expect(replacementError('ok?', WINDOWS_REJECTED)).not.toBeNull()
  })

  it('judges only what the host sent, so Unix accepts what Windows refuses', () => {
    // The rules are the host's, and this is the shape that proves this file
    // holds none of its own: with the Unix set, `*` is a perfectly good
    // replacement. A hardcoded list of "bad characters" here would fail.
    const unixRejected = '\u0000/\\'
    expect(replacementError('*', unixRejected)).toBeNull()
    expect(replacementError('\u0000', unixRejected)).toContain('U+0000')
  })
})

describe('initialAnswers', () => {
  it('prefills every character with the default stand-in', () => {
    const answers = initialAnswers([
      { character: '?', fault: 'rejected', entries: 2 },
      { character: ':', fault: 'reinterpreted', entries: 1 },
    ])
    expect(answers).toEqual({ '?': DEFAULT_REPLACEMENT, ':': DEFAULT_REPLACEMENT })
  })

  it('is writable everywhere, or the default would be refused on sight', () => {
    expect(replacementError(DEFAULT_REPLACEMENT, WINDOWS_REJECTED)).toBeNull()
  })
})

describe('substitutions', () => {
  it('sends what the user typed for each character asked about', () => {
    const characters = [
      { character: '?', fault: 'rejected', entries: 1 },
      { character: ':', fault: 'reinterpreted', entries: 1 },
    ]
    expect(substitutions(characters, { '?': '-', ':': '' })).toEqual({ '?': '-', ':': '' })
  })

  it('leaves out an answer for a character this archive never asked about', () => {
    // Driven by the archive's questions, so an answer left over from a
    // previous one cannot ride along into this extraction.
    const characters = [{ character: '?', fault: 'rejected', entries: 1 }]
    expect(substitutions(characters, { '?': '-', '*': 'stale' })).toEqual({ '?': '-' })
  })

  it('sends nothing when there is nothing to answer', () => {
    expect(substitutions([], { '?': '-' })).toEqual({})
  })
})
