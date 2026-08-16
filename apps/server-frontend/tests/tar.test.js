import { describe, it, expect } from 'vitest'
import { directoriesOf, rootOf, splitPath, tarball } from '../src/tar.js'

const encoder = new TextEncoder()
const bytes = (text) => encoder.encode(text)

/** Read a NUL-terminated field out of a header block. */
function field(buffer, offset, length) {
  const slice = new Uint8Array(buffer, offset, length)
  const end = slice.indexOf(0)
  return new TextDecoder().decode(slice.subarray(0, end === -1 ? length : end))
}

async function blocks(blob) {
  const buffer = await blob.arrayBuffer()
  const out = []
  for (let at = 0; at < buffer.byteLength; at += 512) {
    const name = field(buffer, at, 100)
    if (!name) continue // padding, content or the end-of-archive blocks
    const size = parseInt(field(buffer, at + 124, 12) || '0', 8)
    out.push({ name, size, type: field(buffer, at + 156, 1) || '0', prefix: field(buffer, at + 345, 155) })
    at += Math.ceil(size / 512) * 512 // skip the content
  }
  return out
}

describe('splitPath', () => {
  it('leaves a short path in the name field', () => {
    expect(splitPath('photos/a.txt')).toEqual({ prefix: '', name: 'photos/a.txt' })
  })

  /// ustar's name field is 100 bytes; longer paths spill into prefix, which is
  /// the difference between working and refusing a deep folder.
  it('spills a long path into the prefix field', () => {
    const path = `${'nested/'.repeat(20)}file.txt`
    const { prefix, name } = splitPath(path)
    expect(prefix.length).toBeGreaterThan(0)
    expect(`${prefix}/${name}`).toBe(path)
    expect(bytes(name).length).toBeLessThanOrEqual(100)
    expect(bytes(prefix).length).toBeLessThanOrEqual(155)
  })

  it('refuses a path that cannot be split', () => {
    expect(() => splitPath('x'.repeat(300))).toThrow(/too long/)
  })
})

describe('directoriesOf', () => {
  it('lists every parent, outermost first', () => {
    expect(directoriesOf(['photos/sub/deep/a.txt', 'photos/b.txt'])).toEqual([
      'photos/',
      'photos/sub/',
      'photos/sub/deep/',
    ])
  })

  it('does not repeat a shared parent', () => {
    expect(directoriesOf(['p/a.txt', 'p/b.txt'])).toEqual(['p/'])
  })
})

describe('tarball', () => {
  it('writes a directory entry and one entry per file', async () => {
    const tar = tarball([
      { path: 'photos/a.txt', data: bytes('first') },
      { path: 'photos/sub/b.txt', data: bytes('second') },
    ])

    expect(await blocks(tar)).toEqual([
      { name: 'photos/', size: 0, type: '5', prefix: '' },
      { name: 'photos/sub/', size: 0, type: '5', prefix: '' },
      { name: 'photos/a.txt', size: 5, type: '0', prefix: '' },
      { name: 'photos/sub/b.txt', size: 6, type: '0', prefix: '' },
    ])
  })

  /// The backend refuses an envelope holding anything but one directory, so
  /// catching it here gives a better message than a failed job would.
  it('refuses more than one top-level directory', () => {
    expect(() =>
      tarball([{ path: 'a/x.txt', data: bytes('x') }, { path: 'b/y.txt', data: bytes('y') }]),
    ).toThrow(/exactly one directory/)
  })

  it('refuses an empty selection', () => {
    expect(() => tarball([])).toThrow(/nothing to pack/)
  })

  /// Content is padded to a 512-byte boundary and the archive ends with two
  /// zero blocks; get either wrong and the extractor rejects the whole thing.
  it('pads content and terminates the archive', async () => {
    const tar = tarball([{ path: 'p/a.txt', data: bytes('123') }])
    // dir header + file header + one padded content block + two end blocks.
    expect(tar.size).toBe(512 * 5)
  })

  it('produces the same bytes for the same input', async () => {
    const build = () => tarball([{ path: 'p/a.txt', data: bytes('same') }])
    const [a, b] = [await build().arrayBuffer(), await build().arrayBuffer()]
    expect(new Uint8Array(a)).toEqual(new Uint8Array(b))
  })

  it('handles a file whose size is an exact multiple of the block', async () => {
    const tar = tarball([{ path: 'p/a.bin', data: new Uint8Array(512) }])
    // No extra padding block should be added.
    expect(tar.size).toBe(512 * 5)
  })
})

describe('rootOf', () => {
  it('takes the first segment of the relative path', () => {
    expect(rootOf([{ webkitRelativePath: 'photos/sub/a.txt' }])).toBe('photos')
  })

  it('falls back to the file name when there is no relative path', () => {
    expect(rootOf([{ name: 'a.txt' }])).toBe('a.txt')
  })

  it('returns null for an empty selection', () => {
    expect(rootOf([])).toBeNull()
  })
})
