import { describe, it, expect } from 'vitest'
import { baseName, dirOf, extOf, isArchive, levelHint } from '../src/paths.js'

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
