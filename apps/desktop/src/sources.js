// Where compression happens: locally, or on a remote Collapse server the
// user added. Pure helpers plus the storage read/write, split out of App.vue
// so they can be unit-tested.

/** The built-in destination. Always present, always the default. */
export const LOCAL = 'local'

const SOURCES_KEY = 'collapse.sources'
const DESTINATION_KEY = 'collapse.destination'

/**
 * Turn what someone typed into a usable base URL, or null if it cannot be
 * one. A bare host is assumed to be http, since the server speaks plain HTTP
 * by default; the trailing slash goes so paths join cleanly.
 */
export function normalizeUrl(input) {
  const text = String(input ?? '').trim()
  if (!text) return null

  const withScheme = /^https?:\/\//i.test(text) ? text : `http://${text}`
  let parsed
  try {
    parsed = new URL(withScheme)
  } catch {
    return null
  }
  if (!parsed.hostname) return null

  return `${parsed.origin}${parsed.pathname}`.replace(/\/+$/, '')
}

/** A short label for a server, used when the user does not give one. */
export function labelFromUrl(url) {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

/**
 * Build a source entry. `id` is derived from the URL, so adding the same
 * server twice replaces it rather than piling up duplicates.
 */
export function makeSource(label, url) {
  const normalized = normalizeUrl(url)
  if (!normalized) return null
  const trimmed = String(label ?? '').trim()
  return {
    id: normalized,
    label: trimmed || labelFromUrl(normalized),
    url: normalized,
  }
}

/** Add or replace a source, keeping the list stable for everything else. */
export function upsertSource(sources, source) {
  const rest = sources.filter((s) => s.id !== source.id)
  return [...rest, source]
}

export function removeSource(sources, id) {
  return sources.filter((s) => s.id !== id)
}

/** The URL to compress against, or null when the destination is local. */
export function urlFor(sources, destination) {
  if (!destination || destination === LOCAL) return null
  return sources.find((s) => s.id === destination)?.url ?? null
}

/** What the destination is called in the UI. */
export function labelFor(sources, destination) {
  if (!destination || destination === LOCAL) return 'This computer'
  return sources.find((s) => s.id === destination)?.label ?? 'This computer'
}

/**
 * Parse stored sources, tolerating anything: storage can hold whatever a
 * previous version (or a person with a devtools console) left behind, and a
 * bad entry must not take the app down on boot.
 */
export function parseSources(raw) {
  let parsed
  try {
    parsed = JSON.parse(raw)
  } catch {
    return []
  }
  if (!Array.isArray(parsed)) return []

  return parsed
    .map((entry) => (entry && typeof entry === 'object' ? makeSource(entry.label, entry.url) : null))
    .filter(Boolean)
}

// -- storage ----------------------------------------------------------------
// localStorage keeps this to zero extra dependencies and no new capability.
// Every access is guarded: a webview with storage disabled must degrade to an
// app that simply forgets, not one that fails to start.

function storage() {
  try {
    return globalThis.localStorage ?? null
  } catch {
    return null
  }
}

export function loadSources() {
  return parseSources(storage()?.getItem(SOURCES_KEY) ?? '[]')
}

export function saveSources(sources) {
  try {
    storage()?.setItem(SOURCES_KEY, JSON.stringify(sources))
  } catch {
    /* not persisting is survivable */
  }
}

/**
 * The remembered destination, but only if it still exists: a server removed
 * on the last run must not leave the app pointing at nothing.
 */
export function loadDestination(sources) {
  const stored = storage()?.getItem(DESTINATION_KEY)
  if (stored && sources.some((s) => s.id === stored)) return stored
  return LOCAL
}

export function saveDestination(destination) {
  try {
    storage()?.setItem(DESTINATION_KEY, destination)
  } catch {
    /* not persisting is survivable */
  }
}
