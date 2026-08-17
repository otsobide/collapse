import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import App from '../src/App.vue'

// The app talks to its own origin, so the whole backend is one fetch stub.
let responses
let calls

function reply(body, { ok = true, status = 200 } = {}) {
  return { ok, status, json: async () => body, blob: async () => new Blob(['archive']) }
}

/** Queue the four responses one successful compression makes. */
function happyPath({ archive_name = 'notes.txt.zip' } = {}) {
  return [
    reply({ status: 'ok' }), // health, on mount
    reply({ job_id: 'j1', archive_name }, { status: 202 }),
    reply({ status: 'completed' }),
    reply({}), // download
    reply({ deleted: true }),
  ]
}

/** A File the way a directory picker hands one over. */
function fileAt(relativePath, content = 'x') {
  const file = new File([content], relativePath.split('/').pop())
  Object.defineProperty(file, 'webkitRelativePath', { value: relativePath })
  return file
}

function setFiles(input, files) {
  Object.defineProperty(input.element, 'files', { value: files, configurable: true })
}

async function mountApp() {
  const wrapper = mount(App)
  await flushPromises() // the health check on mount
  return wrapper
}

const compressButton = (w) => w.findAll('button').find((b) => b.text() === 'Compress')

beforeEach(() => {
  calls = []
  responses = happyPath()
  vi.stubGlobal('fetch', vi.fn(async (url, options = {}) => {
    calls.push({ url, method: options.method || 'GET', body: options.body })
    const next = responses.shift()
    if (!next) throw new Error(`unexpected request: ${url}`)
    return next
  }))
  // Only the object-URL helpers: replacing URL itself would break the
  // constructor these tests use to read the query string back.
  URL.createObjectURL = () => 'blob:fake'
  URL.revokeObjectURL = () => {}
})

describe('App', () => {
  it('checks the server on load and says so', async () => {
    const w = await mountApp()
    expect(calls[0].url).toBe('/health')
    expect(w.find('.where').text()).toBe('on the server')
    expect(w.text()).toContain('Drop a file to compress')
  })

  it('says when the server cannot be reached', async () => {
    responses = [reply({}, { ok: false, status: 502 })]
    const w = await mountApp()
    expect(w.find('.where').text()).toContain('unreachable')
  })

  it('compresses a picked file and offers the archive', async () => {
    const w = await mountApp()
    const input = w.find('input[type=file]:not([webkitdirectory])')
    setFiles(input, [new File(['hello'], 'notes.txt')])
    await input.trigger('change')

    expect(w.find('.file-name').text()).toBe('notes.txt')
    await compressButton(w).trigger('click')
    await flushPromises()

    const upload = calls.find((c) => c.method === 'POST')
    const query = new URL(`http://x${upload.url}`).searchParams
    expect(Object.fromEntries(query)).toEqual({
      name: 'notes.txt',
      algorithm: 'zip',
      level: '3',
      envelope: 'none',
    })
    expect(w.find('.done-title').text()).toBe('Compressed')
    expect(w.find('.done .cta').attributes('download')).toBe('notes.txt.zip')
  })

  /// A folder cannot travel over HTTP, so the app packs it into the tar
  /// envelope the backend unwraps. This is the wiring a browser test cannot
  /// reach, because a directory picker cannot be simulated from outside.
  it('packs a chosen folder into a tar envelope', async () => {
    responses = happyPath({ archive_name: 'photos.zip' })
    const w = await mountApp()

    const input = w.find('input[webkitdirectory]')
    setFiles(input, [fileAt('photos/a.txt', 'one'), fileAt('photos/sub/b.txt', 'two')])
    await input.trigger('change')
    await flushPromises()

    expect(w.find('.file-name').text()).toBe('photos')
    expect(w.find('.drop .hint').text()).toContain('2 files')

    await compressButton(w).trigger('click')
    await flushPromises()

    const upload = calls.find((c) => c.method === 'POST')
    const query = new URL(`http://x${upload.url}`).searchParams
    expect(query.get('envelope')).toBe('tar')
    expect(query.get('name')).toBe('photos')
    // What goes on the wire is the tar itself, not the files.
    expect(upload.body.type).toBe('application/x-tar')
    expect(upload.body.size).toBeGreaterThan(0)
  })

  it('surfaces the reason the server refused', async () => {
    responses = [
      reply({ status: 'ok' }),
      reply({ detail: 'Invalid file name.' }, { ok: false, status: 400 }),
    ]
    const w = await mountApp()
    const input = w.find('input[type=file]:not([webkitdirectory])')
    setFiles(input, [new File(['x'], 'bad')])
    await input.trigger('change')

    await compressButton(w).trigger('click')
    await flushPromises()

    expect(w.find('.error').text()).toContain('Invalid file name.')
  })

  it('disables the level selector for tar, like the desktop app', async () => {
    const w = await mountApp()
    const input = w.find('input[type=file]:not([webkitdirectory])')
    setFiles(input, [new File(['x'], 'notes.txt')])
    await input.trigger('change')

    const formats = w.findAll('.segmented:not(.levels) button')
    await formats[2].trigger('click') // TAR
    const levels = w.findAll('.segmented.levels button')
    expect(levels.every((b) => b.attributes('disabled') !== undefined)).toBe(true)
  })
})
