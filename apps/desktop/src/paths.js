// Pure path/format helpers, split out from App.vue so they can be unit-tested.

export const ARCHIVE_EXTS = ['zip', '7z', 'tar']

/** Last path segment, tolerant of both `/` and `\` separators. */
export function baseName(path) {
  return path.split(/[\\/]/).pop()
}

/** Directory portion and the separator used, for building a sibling path. */
export function dirOf(path) {
  const sep = path.includes('\\') ? '\\' : '/'
  const i = path.lastIndexOf(sep)
  return { dir: i >= 0 ? path.slice(0, i) : '', sep }
}

/** Lowercased final extension (without the dot), or '' if none. */
export function extOf(path) {
  const name = baseName(path)
  const i = name.lastIndexOf('.')
  return i >= 0 ? name.slice(i + 1).toLowerCase() : ''
}

/** Whether a path looks like an archive we can extract. */
export function isArchive(path) {
  return ARCHIVE_EXTS.includes(extOf(path))
}

/** Human label for a 1–5 compression level. */
export function levelHint(level) {
  return level <= 2 ? 'Faster' : level >= 4 ? 'Smaller' : 'Balanced'
}

/**
 * One line saying what the archive gets checked for, under the Verify row.
 *
 * Unticked is not "unchecked": the backend reads every archive's listing back
 * and compares it with the entries it was asked to store, before the archive is
 * allowed to reach the place the user picked. Ticking adds the pass that
 * decompresses every entry, and what that buys is genuinely not the same for
 * all three formats, so the line says what the chosen one really gets. zip and
 * 7z keep a checksum per entry and compare it while the entry is read; tar
 * keeps none over an entry's data (its header checksum covers the header
 * alone), so there the deeper pass can only prove the archive is complete.
 * Saying "contents verified" for tar as well would be the comfortable sentence
 * and a false one.
 */
export function verifyNote({ checked, format, remote }) {
  if (remote) return 'The server compresses this one, so what is checked is up to it.'
  if (!checked) return "The archive's listing is always checked before it is saved."
  if (format === 'tar') {
    return 'Every entry is read back: roughly twice the work. Tar keeps no checksum of the data to check.'
  }
  return 'Every entry is decompressed and its checksum checked: roughly twice the work.'
}
