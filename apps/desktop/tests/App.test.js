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
    return null
  })
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
