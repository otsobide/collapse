// Pure presentation helpers, kept out of App.vue so they can be unit-tested
// (the same split the desktop app makes with paths.js).

export const FORMATS = ['zip', '7z', 'tar']

/** Human label for a 1-5 compression level, as in the desktop app. */
export function levelHint(level) {
  return level <= 2 ? 'Faster' : level >= 4 ? 'Smaller' : 'Balanced'
}

/** Bytes as something a person can read. */
export function humanSize(bytes) {
  if (!Number.isFinite(bytes) || bytes < 0) return ''
  if (bytes < 1024) return `${bytes} B`
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  let value = bytes / 1024
  let unit = 0
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024
    unit += 1
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`
}

/** How much smaller the archive came back, or null when it did not shrink. */
export function savings(originalBytes, archiveBytes) {
  if (!originalBytes || !Number.isFinite(archiveBytes)) return null
  const ratio = 1 - archiveBytes / originalBytes
  return ratio > 0.005 ? Math.round(ratio * 100) : null
}

/** What the archive will be called, mirroring the backend's own rule. */
export function archiveName(name, format) {
  return `${name}.${format}`
}

/** The label shown for each step of the job flow. */
export function stepLabel(status) {
  return {
    uploading: 'Uploading',
    queued: 'Queued',
    compressing: 'Compressing',
    downloading: 'Downloading',
    done: 'Done',
  }[status] || status
}
