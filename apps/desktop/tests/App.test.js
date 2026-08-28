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
const verifyBox = (w) => w.find('input[type="checkbox"]')

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
      overwrite: true, // the save dialog already asked before returning the path
      verify: false, // the contents check is opt-in; the listing is checked anyway
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

    // Where the archive is and where it goes, and nothing else: there is no
    // answer to carry any more.
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
      overwrite: true,
      verify: false,
    })
  })

  it('asks for the contents check when the box is ticked', async () => {
    open.mockResolvedValue('/Users/me/report.pdf')
    save.mockResolvedValue('/Users/me/report.pdf.zip')

    const w = await mountApp()
    await w.find('.drop').trigger('click')
    await flushPromises()

    // Off to begin with, and the note says the cheap check happens regardless,
    // so nobody reads the empty box as "this archive is not checked at all".
    expect(verifyBox(w).element.checked).toBe(false)
    expect(w.text()).toContain("The archive's listing is always checked")

    await verifyBox(w).setValue(true)
    expect(w.text()).toContain('roughly twice the work')

    await w.find('.cta').trigger('click')
    await flushPromises()

    // The one key that changed. A component that dropped `verify` from the
    // payload, or sent the ref instead of the effective value, fails here.
    expect(invoke).toHaveBeenCalledWith('compress_path', {
      path: '/Users/me/report.pdf',
      output: '/Users/me/report.pdf.zip',
      format: 'zip',
      level: 3,
      server: null,
      overwrite: true,
      verify: true,
    })
  })

  it('says what the contents check can prove about a tar', async () => {
    // tar keeps no checksum over an entry's data, so ticking the box buys
    // something weaker there than it does for zip and 7z. Promising the same
    // thing for all three would be the easy copy and a false one.
    open.mockResolvedValue('/Users/me/report.pdf')

    const w = await mountApp()
    await w.find('.drop').trigger('click')
    await flushPromises()
    await verifyBox(w).setValue(true)

    expect(w.text()).toContain('its checksum checked')

    await formatButtons(w)[2].trigger('click') // TAR
    expect(w.text()).toContain('Tar keeps no checksum of the data')
    expect(w.text()).not.toContain('its checksum checked')
  })

  it('never asks a server for a contents check, and remembers the choice', async () => {
    open.mockResolvedValue('/Users/me/report.pdf')
    save.mockResolvedValue('/Users/me/report.pdf.zip')

    const w = await mountApp()

    await w.find('.gear').trigger('click')
    const [, url] = w.findAll('.add-source input')
    await url.setValue('http://box.local:8000')
    await w.find('.add-source').trigger('submit')
    await flushPromises()
    await w.find('.sheet-head .ghost').trigger('click') // Done

    await w.find('.drop').trigger('click')
    await flushPromises()

    // Ticked while the work is local...
    await verifyBox(w).setValue(true)
    expect(verifyBox(w).element.checked).toBe(true)

    // ...then handed to a server, which compresses it on the other side: the
    // box goes back to unticked and dead, and says why.
    await w.find('.picker').setValue('http://box.local:8000')
    expect(verifyBox(w).attributes('disabled')).toBeDefined()
    expect(verifyBox(w).element.checked).toBe(false)
    expect(w.text()).toContain('The server compresses this one')

    // Back to this computer and the tick is still there: choosing a server
    // must not quietly forget what the user asked for. (Checked before the
    // compression below, which replaces the whole panel with the result.)
    await w.find('.picker').setValue('local')
    expect(verifyBox(w).attributes('disabled')).toBeUndefined()
    expect(verifyBox(w).element.checked).toBe(true)

    await w.find('.picker').setValue('http://box.local:8000')
    await w.find('.cta').trigger('click')
    await flushPromises()

    // The remembered tick must not ride along to a server that cannot honour
    // it: this is the assertion that fails if the payload sends the raw ref.
    expect(invoke).toHaveBeenLastCalledWith(
      'compress_path',
      expect.objectContaining({ server: 'http://box.local:8000', verify: false })
    )
  })

  it('offers no contents check in extract mode', async () => {
    // Extraction verifies nothing and takes no such option, so the control
    // must not be on screen at all.
    open.mockResolvedValue('/Users/me/photos.zip')

    const w = await mountApp()
    await modeButtons(w)[1].trigger('click') // Extract
    await w.find('.drop').trigger('click')
    await flushPromises()

    expect(verifyBox(w).exists()).toBe(false)
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

  it('shows a naming refusal in the banner', async () => {
    // The dialog that used to hold these is gone: extraction refuses a name it
    // cannot write instead of asking what to call it, so the refusal is an
    // ordinary failure and the banner is the only place it can land. If it
    // stopped arriving, the Extract button would appear to do nothing at all.
    open
      .mockResolvedValueOnce('/Users/me/logs.tar')
      .mockResolvedValueOnce('/Users/me/out')
    const stub = invoke.getMockImplementation()
    invoke.mockImplementation(async (cmd, args) => {
      if (cmd === 'extract_archive') {
        throw 'the archive entry "what?.txt" cannot be written on this system'
      }
      return stub(cmd, args)
    })

    const w = await mountApp()
    await modeButtons(w)[1].trigger('click')
    await w.find('.drop').trigger('click')
    await flushPromises()
    await w.find('.work .cta').trigger('click')
    await flushPromises()

    expect(w.text()).toContain('cannot be written on this system')
  })

  it('shows a failure that is not about names in the banner too', async () => {
    open
      .mockResolvedValueOnce('/Users/me/logs.tar')
      .mockResolvedValueOnce('/Users/me/out')
    const stub = invoke.getMockImplementation()
    invoke.mockImplementation(async (cmd, args) => {
      if (cmd === 'extract_archive') throw 'IO error: No space left on device'
      return stub(cmd, args)
    })

    const w = await mountApp()
    await modeButtons(w)[1].trigger('click')
    await w.find('.drop').trigger('click')
    await flushPromises()
    await w.find('.work .cta').trigger('click')
    await flushPromises()

    expect(w.find('.error').text()).toContain('No space left on device')
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
