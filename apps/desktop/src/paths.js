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
