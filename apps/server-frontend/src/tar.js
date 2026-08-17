// A minimal ustar writer.
//
// HTTP carries no notion of a folder, so the backend accepts a directory as a
// tar it unpacks (`envelope=tar`). The desktop app builds that tar with the
// Rust engine; in a browser there is no such thing, so this builds it by hand.
// Tar is a simple format: a 512-byte header per entry, the content padded to
// the next 512-byte boundary, and two zero blocks to finish.

const BLOCK = 512
const encoder = new TextEncoder()

/** Write `text` into `buffer` at `offset`, NUL-padded to `length` bytes. */
function writeString(buffer, offset, length, text) {
  const bytes = encoder.encode(text)
  if (bytes.length > length) throw new Error(`field does not fit in ${length} bytes: ${text}`)
  buffer.set(bytes, offset)
}

/** Tar stores numbers as zero-padded octal followed by a NUL. */
function writeOctal(buffer, offset, length, value) {
  const text = value.toString(8).padStart(length - 1, '0')
  writeString(buffer, offset, length, text)
}

/**
 * Split a path into ustar's `prefix` and `name` fields, which together allow
 * 255 bytes rather than the 100 a bare name field holds.
 */
export function splitPath(path) {
  const bytes = encoder.encode(path)
  if (bytes.length <= 100) return { prefix: '', name: path }

  // The split has to fall on a separator, and neither half may overflow.
  for (let i = path.length - 1; i >= 0; i -= 1) {
    if (path[i] !== '/') continue
    const prefix = path.slice(0, i)
    const name = path.slice(i + 1)
    if (encoder.encode(prefix).length <= 155 && encoder.encode(name).length <= 100) {
      return { prefix, name }
    }
  }
  throw new Error(`path is too long for a tar archive: ${path}`)
}

/** One 512-byte header. `type` is '0' for a file and '5' for a directory. */
function header(path, size, type) {
  const block = new Uint8Array(BLOCK)
  const { prefix, name } = splitPath(path)

  writeString(block, 0, 100, name)
  writeOctal(block, 100, 8, type === '5' ? 0o755 : 0o644)
  writeOctal(block, 108, 8, 0) // uid
  writeOctal(block, 116, 8, 0) // gid
  writeOctal(block, 124, 12, size)
  // A fixed timestamp keeps the same folder producing the same bytes, which
  // is what lets a test compare two runs.
  writeOctal(block, 136, 12, 0)
  writeString(block, 148, 8, '        ') // checksum, scored as spaces first
  writeString(block, 156, 1, type)
  writeString(block, 257, 6, 'ustar\0')
  writeString(block, 263, 2, '00')
  writeString(block, 345, 155, prefix)

  let sum = 0
  for (const byte of block) sum += byte
  writeString(block, 148, 8, `${sum.toString(8).padStart(6, '0')}\0 `)

  return block
}

/** Every directory that has to exist for these paths, outermost first. */
export function directoriesOf(paths) {
  const dirs = new Set()
  for (const path of paths) {
    const parts = path.split('/')
    parts.pop() // the file itself
    for (let i = 1; i <= parts.length; i += 1) dirs.add(`${parts.slice(0, i).join('/')}/`)
  }
  return [...dirs].sort()
}

/**
 * Build a tar from `entries` ({ path, data }), where every path shares one
 * top-level directory. That is what the backend validates before compressing.
 *
 * Returns a Blob, so the caller can hand it straight to fetch without a second
 * copy in memory.
 */
export function tarball(entries) {
  if (entries.length === 0) throw new Error('there is nothing to pack')

  const paths = entries.map((e) => e.path)
  const roots = new Set(paths.map((p) => p.split('/')[0]))
  if (roots.size !== 1) {
    throw new Error(`a tar envelope must hold exactly one directory, got ${roots.size}`)
  }

  const parts = []
  for (const dir of directoriesOf(paths)) parts.push(header(dir, 0, '5'))
  for (const { path, data } of entries) {
    parts.push(header(path, data.byteLength, '0'))
    parts.push(data)
    const remainder = data.byteLength % BLOCK
    if (remainder !== 0) parts.push(new Uint8Array(BLOCK - remainder))
  }
  parts.push(new Uint8Array(BLOCK * 2)) // end of archive

  return new Blob(parts, { type: 'application/x-tar' })
}

/**
 * The root folder name of a browser directory selection. Files picked with
 * `webkitdirectory` carry a relative path like `photos/sub/a.txt`.
 */
export function rootOf(files) {
  const first = files[0]
  if (!first) return null
  const relative = first.webkitRelativePath || first.name
  return relative.split('/')[0] || null
}
