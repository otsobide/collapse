import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

// Mock the Tauri APIs App.vue depends on. vi.hoisted makes the mock fns exist
// before the vi.mock factories run.
const { invoke, open, save, onDragDropEvent } = vi.hoisted(() => ({
  invoke: vi.fn(),
  open: vi.fn(),
  save: vi.fn(),
  onDragDropEvent: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open, save }))
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent }),
}))

import App from '../src/App.vue'

async function mountApp() {
  const wrapper = mount(App)
  await flushPromises() // let onMounted register the drag-drop listener
  return wrapper
}

const modeButtons = (w) => w.findAll('.modes button')
const formatButtons = (w) => w.findAll('.segmented:not(.levels) button')

beforeEach(() => {
  vi.clearAllMocks()
  onDragDropEvent.mockResolvedValue(() => {})
  invoke.mockImplementation(async (cmd) => {
    if (cmd === 'is_directory') return false
    if (cmd === 'compress_path') return '/Users/me/report.pdf.7z'
    if (cmd === 'extract_archive') return ['notes.txt', 'sub/data.bin']
    if (cmd === 'check_server') return null
    return null
  })
  // Each test starts with no remembered servers. Guarded: the happy-dom
  // environment does not always expose storage, and sources.js copes with
  // that too (it simply stops remembering).
  globalThis.localStorage?.clear()
})

describe('App', () => {
  it('starts in Compress mode with the drop prompt', async () => {
    const w = await mountApp()
    expect(w.text()).toContain('Drop a file or folder')
    expect(modeButtons(w)[0].classes()).toContain('on') // Compress active
  })

  it('switches to Extract mode', async () => {
    const w = await mountApp()
    await modeButtons(w)[1].trigger('click') // Extract
    expect(w.text()).toContain('Drop an archive to extract')
    expect(modeButtons(w)[1].classes()).toContain('on')
  })

  it('compresses a picked file with the right command and args', async () => {
    open.mockResolvedValue('/Users/me/report.pdf')
    save.mockResolvedValue('/Users/me/report.pdf.7z')

    const w = await mountApp()
    await w.find('.drop').trigger('click') // browse
    await flushPromises() // open() resolves, pick() runs is_directory

    // Choose 7z, then compress.
    await formatButtons(w)[1].trigger('click') // ZIP, 7z, TAR -> 7z
    await w.find('.cta').trigger('click')
    await flushPromises() // save() then compress_path

    expect(invoke).toHaveBeenCalledWith('compress_path', {
      path: '/Users/me/report.pdf',
      output: '/Users/me/report.pdf.7z',
      format: '7z',
      level: 3,
      server: null, // local by default
    })
    expect(w.text()).toContain('Compressed')
    expect(w.text()).toContain('/Users/me/report.pdf.7z')
  })

  it('extracts an archive with the right command and args', async () => {
    open
      .mockResolvedValueOnce('/Users/me/photos.zip') // browse: pick archive
      .mockResolvedValueOnce('/Users/me/out') // extract: pick destination

    const w = await mountApp()
    await modeButtons(w)[1].trigger('click') // Extract mode
    await w.find('.drop').trigger('click') // browse
    await flushPromises()

    await w.find('.cta').trigger('click') // "Extract to…"
    await flushPromises()

    expect(invoke).toHaveBeenCalledWith('extract_archive', {
      archive: '/Users/me/photos.zip',
      outputDir: '/Users/me/out',
    })
    expect(w.text()).toContain('Extracted 2 files')
  })

  it('sends the chosen server, and only in compress mode', async () => {
    open.mockResolvedValue('/Users/me/report.pdf')
    save.mockResolvedValue('/Users/me/report.pdf.zip')

    const w = await mountApp()

    // Add a server through the panel, the way a user would.
    await w.find('.gear').trigger('click')
    const [label, url] = w.findAll('.add-source input')
    await label.setValue('Build box')
    await url.setValue('box.local:8000')
    await w.find('.add-source').trigger('submit')
    await flushPromises()

    expect(invoke).toHaveBeenCalledWith('check_server', { url: 'http://box.local:8000' })

    await w.find('.sheet-head .ghost').trigger('click') // Done
    await w.find('.drop').trigger('click')
    await flushPromises()

    // The picker offers local plus the new server, and defaults to local.
    const picker = w.find('.picker')
    expect(picker.findAll('option').map((o) => o.text())).toEqual([
      'This computer',
      'Build box',
    ])
    expect(picker.element.value).toBe('local')

    await picker.setValue('http://box.local:8000')
    await w.find('.cta').trigger('click')
    await flushPromises()

    expect(invoke).toHaveBeenCalledWith('compress_path', {
      path: '/Users/me/report.pdf',
      output: '/Users/me/report.pdf.zip',
      format: 'zip',
      level: 3,
      server: 'http://box.local:8000',
    })
  })

  it('offers no destination picker in extract mode', async () => {
    open.mockResolvedValue('/Users/me/photos.zip')

    const w = await mountApp()
    await modeButtons(w)[1].trigger('click') // Extract
    await w.find('.drop').trigger('click')
    await flushPromises()

    // Extraction has no remote path, so the picker must not be offered.
    expect(w.find('.picker').exists()).toBe(false)
  })

  it('disables the level selector for tar', async () => {
    open.mockResolvedValue('/Users/me/report.pdf')

    const w = await mountApp()
    await w.find('.drop').trigger('click')
    await flushPromises()

    await formatButtons(w)[2].trigger('click') // TAR
    const levelBtns = w.findAll('.segmented.levels button')
    expect(levelBtns.every((b) => b.attributes('disabled') !== undefined)).toBe(true)
  })
})
