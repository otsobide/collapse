import { describe, it, expect } from 'vitest'
import { baseName, dirOf, extOf, isArchive, levelHint, verifyNote } from '../src/paths.js'

describe('baseName', () => {
  it('returns the last segment for unix and windows paths', () => {
    expect(baseName('/home/user/notes.txt')).toBe('notes.txt')
    expect(baseName('C:\\Users\\me\\photos')).toBe('photos')
    expect(baseName('bare.zip')).toBe('bare.zip')
  })
})

describe('dirOf', () => {
  it('splits the directory and detects the separator', () => {
    expect(dirOf('/home/user/notes.txt')).toEqual({ dir: '/home/user', sep: '/' })
    expect(dirOf('C:\\Users\\me\\a.7z')).toEqual({ dir: 'C:\\Users\\me', sep: '\\' })
    expect(dirOf('bare.zip')).toEqual({ dir: '', sep: '/' })
  })
})

describe('extOf', () => {
  it('returns the lowercased final extension, or empty', () => {
    expect(extOf('/a/b/photo.PNG')).toBe('png')
    expect(extOf('archive.tar')).toBe('tar')
    expect(extOf('/a/b/noext')).toBe('')
    expect(extOf('folder')).toBe('')
  })
})

describe('isArchive', () => {
  it('recognizes the supported archive extensions', () => {
    for (const ext of ['zip', '7z', 'tar']) {
      expect(isArchive(`/x/file.${ext}`)).toBe(true)
      expect(isArchive(`/x/file.${ext.toUpperCase()}`)).toBe(true)
    }
    expect(isArchive('/x/notes.txt')).toBe(false)
    expect(isArchive('/x/photos')).toBe(false)
    expect(isArchive('/x/archive.rar')).toBe(false)
  })
})

describe('levelHint', () => {
  it('maps 1–5 to Faster / Balanced / Smaller', () => {
    expect(levelHint(1)).toBe('Faster')
    expect(levelHint(2)).toBe('Faster')
    expect(levelHint(3)).toBe('Balanced')
    expect(levelHint(4)).toBe('Smaller')
    expect(levelHint(5)).toBe('Smaller')
  })
})

describe('verifyNote', () => {
  it('never says an unticked box means an unchecked archive', () => {
    // The line under the box is the only thing telling a user that the cheap
    // check runs regardless. Wording it as "nothing is checked" would push
    // people into paying for the deep pass out of doubt.
    for (const format of ['zip', '7z', 'tar']) {
      const note = verifyNote({ checked: false, format, remote: false })
      expect(note).toBe("The archive's listing is always checked before it is saved.")
    }
  })

  it('promises checksums only for the formats that keep them', () => {
    // zip and 7z store a checksum per entry and the readers compare it; tar
    // stores none over an entry's data, so the same sentence there would be a
    // guarantee nothing in the file can back.
    for (const format of ['zip', '7z']) {
      const note = verifyNote({ checked: true, format, remote: false })
      expect(note).toContain('checksum checked')
      expect(note).toContain('twice the work')
    }

    const tar = verifyNote({ checked: true, format: 'tar', remote: false })
    expect(tar).toContain('Tar keeps no checksum of the data')
    expect(tar).not.toContain('checksum checked')
    expect(tar).toContain('twice the work')
  })

  it('claims nothing about an archive built somewhere else', () => {
    // The app cannot check what it did not describe, and must not imply the
    // server skips checking either: that is the server's business.
    for (const format of ['zip', '7z', 'tar']) {
      for (const checked of [false, true]) {
        const note = verifyNote({ checked, format, remote: true })
        expect(note).toBe('The server compresses this one, so what is checked is up to it.')
      }
    }
  })
})
