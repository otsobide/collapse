import { describe, it, expect } from 'vitest'
import { archiveName, humanSize, levelHint, savings, stepLabel } from '../src/format.js'

describe('levelHint', () => {
  it('matches the desktop wording', () => {
    expect([1, 2, 3, 4, 5].map(levelHint)).toEqual([
      'Faster', 'Faster', 'Balanced', 'Smaller', 'Smaller',
    ])
  })
})

describe('humanSize', () => {
  it('keeps small sizes in bytes', () => {
    expect(humanSize(0)).toBe('0 B')
    expect(humanSize(1023)).toBe('1023 B')
  })

  it('steps up through the units', () => {
    expect(humanSize(1024)).toBe('1.0 KiB')
    expect(humanSize(1024 * 1024 * 3.5)).toBe('3.5 MiB')
    expect(humanSize(1024 ** 3 * 2)).toBe('2.0 GiB')
  })

  it('says nothing about a size it cannot read', () => {
    expect(humanSize(NaN)).toBe('')
    expect(humanSize(-1)).toBe('')
  })
})

describe('savings', () => {
  it('reports how much smaller the archive is', () => {
    expect(savings(1000, 250)).toBe(75)
  })

  /// Some inputs do not compress; claiming "0% smaller" would be noise.
  it('says nothing when it did not really shrink', () => {
    expect(savings(1000, 998)).toBeNull()
    expect(savings(1000, 1200)).toBeNull()
    expect(savings(0, 100)).toBeNull()
  })
})

describe('archiveName', () => {
  it('appends the format, keeping the original extension', () => {
    expect(archiveName('notes.txt', 'zip')).toBe('notes.txt.zip')
    expect(archiveName('photos', '7z')).toBe('photos.7z')
  })
})

describe('stepLabel', () => {
  it('labels every state the flow reports', () => {
    expect(['uploading', 'queued', 'compressing', 'downloading', 'done'].map(stepLabel)).toEqual([
      'Uploading', 'Queued', 'Compressing', 'Downloading', 'Done',
    ])
  })

  it('shows an unknown state rather than hiding it', () => {
    expect(stepLabel('surprising')).toBe('surprising')
  })
})
