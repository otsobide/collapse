import { describe, it, expect } from 'vitest'
import {
  LOCAL,
  labelFor,
  labelFromUrl,
  makeSource,
  normalizeUrl,
  parseSources,
  removeSource,
  upsertSource,
  urlFor,
} from '../src/sources.js'

describe('normalizeUrl', () => {
  it('assumes http for a bare host, since the server speaks plain HTTP', () => {
    expect(normalizeUrl('localhost:8000')).toBe('http://localhost:8000')
    expect(normalizeUrl('192.168.1.10:8000')).toBe('http://192.168.1.10:8000')
  })

  it('keeps an explicit scheme and a path prefix', () => {
    expect(normalizeUrl('https://box.local:8000')).toBe('https://box.local:8000')
    expect(normalizeUrl('http://box.local/collapse')).toBe('http://box.local/collapse')
  })

  it('strips trailing slashes so paths join cleanly', () => {
    expect(normalizeUrl('http://box.local:8000/')).toBe('http://box.local:8000')
    expect(normalizeUrl('  http://box.local:8000///  ')).toBe('http://box.local:8000')
  })

  it('rejects what cannot be a server address', () => {
    for (const bad of ['', '   ', null, undefined, 'http://']) {
      expect(normalizeUrl(bad)).toBeNull()
    }
  })
})

describe('makeSource', () => {
  it('falls back to the host when no name is given', () => {
    expect(makeSource('', 'localhost:8000')).toEqual({
      id: 'http://localhost:8000',
      label: 'localhost:8000',
      url: 'http://localhost:8000',
    })
  })

  it('keeps the given name', () => {
    expect(makeSource('  Build box  ', 'box.local:8000').label).toBe('Build box')
  })

  it('returns null for an unusable address', () => {
    expect(makeSource('Nope', '')).toBeNull()
  })
})

describe('the source list', () => {
  const a = makeSource('A', 'a.local:8000')
  const b = makeSource('B', 'b.local:8000')

  it('adding the same server twice replaces it instead of duplicating', () => {
    const renamed = makeSource('A renamed', 'a.local:8000')
    const list = upsertSource(upsertSource([], a), renamed)
    expect(list).toHaveLength(1)
    expect(list[0].label).toBe('A renamed')
  })

  it('removes by id', () => {
    expect(removeSource([a, b], a.id)).toEqual([b])
  })

  it('resolves the URL to compress against, or null for local', () => {
    expect(urlFor([a, b], b.id)).toBe('http://b.local:8000')
    expect(urlFor([a, b], LOCAL)).toBeNull()
    // A destination that no longer exists must not become a silent remote.
    expect(urlFor([a], b.id)).toBeNull()
  })

  it('labels the destination for the UI', () => {
    expect(labelFor([a], LOCAL)).toBe('This computer')
    expect(labelFor([a], a.id)).toBe('A')
    expect(labelFor([], 'gone')).toBe('This computer')
  })
})

describe('parseSources', () => {
  it('reads back what was stored', () => {
    const stored = JSON.stringify([{ label: 'Box', url: 'http://box.local:8000' }])
    expect(parseSources(stored)).toEqual([
      { id: 'http://box.local:8000', label: 'Box', url: 'http://box.local:8000' },
    ])
  })

  /// Storage can hold anything an older version or a console left behind, and
  /// a bad entry must not stop the app from starting.
  it('survives anything stored', () => {
    for (const junk of ['', 'not json', '{}', '3', 'null', '[1,2,3]', '[{"url":""}]']) {
      expect(parseSources(junk)).toEqual([])
    }
  })

  it('drops only the broken entries', () => {
    const mixed = JSON.stringify([{ url: 'ok.local:8000' }, null, { url: '' }])
    expect(parseSources(mixed)).toHaveLength(1)
  })
})

describe('labelFromUrl', () => {
  it('uses the host', () => {
    expect(labelFromUrl('http://box.local:8000')).toBe('box.local:8000')
  })
})
