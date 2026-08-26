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

/// The naming sheet: the second modal, so every selector inside it is scoped
/// to `.naming` rather than fighting the servers sheet for `.sheet`.
const namingSheet = (w) => w.find('.naming')
const answerFields = (w) => w.findAll('.naming .answer-field')
const confirmNames = (w) => w.find('.naming .cta')

/**
 * What `unwritable_names` answers when the host cannot write something.
 *
 * Every case below builds one of these by hand, because the machine running
 * this suite is a Mac or a Linux CI runner and both write `what?.txt` without
 * complaint: the dialog literally cannot be produced here by extracting a real
 * archive. The shape is not invented, it is `NameInspection`'s serialization,
 * pinned on the Rust side by `src-tauri/tests/names.rs`.
 */
function inspection({ entries = [], characters = [], rejected = '<>"|?*:/\\' } = {}) {
  return { entries, characters, rejectedInReplacement: rejected }
}

const NOTHING_TO_ASK = inspection()

/**
 * An archive holding two entries a Windows host cannot write, both for the
 * same reason: one question, two files behind it.
 */
const A_QUESTION = inspection({
  entries: [
    {
      entry: 'logs/what?.txt',
      problems: [{ kind: 'character', character: '?', fault: 'rejected' }],
    },
    { entry: 'when?.txt', problems: [{ kind: 'character', character: '?', fault: 'rejected' }] },
  ],
  characters: [{ character: '?', fault: 'rejected', entries: 2 }],
})

/**
 * Pick an archive, choose a destination, and get as far as the naming sheet.
 *
 * `extraction` is what `extract_archive` will answer once the user confirms;
 * everything else falls through to the stub switch in `beforeEach`.
 */
async function ask({ report = A_QUESTION, extraction } = {}) {
  open
    .mockResolvedValueOnce('/Users/me/logs.tar') // browse: pick the archive
    .mockResolvedValueOnce('/Users/me/out') // extract: pick the destination
  const stub = invoke.getMockImplementation()
  invoke.mockImplementation(async (cmd, args) => {
    if (cmd === 'unwritable_names') return report
    if (cmd === 'extract_archive' && extraction) return extraction
    return stub(cmd, args)
  })

  const w = await mountApp()
  await modeButtons(w)[1].trigger('click') // Extract mode
  await w.find('.drop').trigger('click') // browse
  await flushPromises()
  await w.find('.work .cta').trigger('click') // "Extract to…"
  await flushPromises()
  return w
}

beforeEach(() => {
  vi.clearAllMocks()
  onDragDropEvent.mockResolvedValue(() => {})
  invoke.mockImplementation(async (cmd) => {
    if (cmd === 'is_directory') return false
    if (cmd === 'compress_path') return '/Users/me/report.pdf.7z'
    if (cmd === 'unwritable_names') return NOTHING_TO_ASK
    if (cmd === 'extract_archive') {
      return { status: 'extracted', files: ['notes.txt', 'sub/data.bin'] }
    }
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

    expect(invoke).toHaveBeenCalledWith('extract_archive', {
      archive: '/Users/me/photos.zip',
      outputDir: '/Users/me/out',
      // Nothing to answer, so nothing is substituted. An archive the host can
      // write goes straight through, without a dialog in the way.
      replacements: {},
    })
    expect(namingSheet(w).exists()).toBe(false)
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

  it('asks before extracting a name this computer cannot write', async () => {
    const w = await ask()

    expect(namingSheet(w).exists()).toBe(true)
    expect(w.text()).toContain('Names this computer cannot write')
    expect(w.text()).toContain('2 entries are named')
    expect(w.text()).toContain('logs/what?.txt')
    // The whole point of the feature: the archive is not touched until the
    // question is answered. A component that extracted first and asked
    // afterwards would pass every other assertion here.
    expect(invoke).not.toHaveBeenCalledWith('extract_archive', expect.anything())
    // One field for the one character, whatever the number of entries, and
    // prefilled so the common case is one click.
    expect(answerFields(w)).toHaveLength(1)
    expect(answerFields(w)[0].element.value).toBe('_')
  })

  it('extracts with the answers the user gave, and lists what is on disk', async () => {
    const w = await ask({
      extraction: { status: 'extracted', files: ['logs/what-.txt', 'when-.txt'] },
    })

    await answerFields(w)[0].setValue('-')
    await confirmNames(w).trigger('click')
    await flushPromises()

    expect(invoke).toHaveBeenCalledWith('extract_archive', {
      archive: '/Users/me/logs.tar',
      outputDir: '/Users/me/out',
      replacements: { '?': '-' },
    })
    expect(namingSheet(w).exists()).toBe(false)
    // The names as written, never the archive's: `what?.txt` is on no disk
    // here, and showing it would send the user looking for a file that is not
    // there.
    expect(w.text()).toContain('when-.txt')
    expect(w.text()).not.toContain('when?.txt')
  })

  it('reads an empty answer as "remove the character"', async () => {
    const w = await ask()

    await answerFields(w)[0].setValue('')
    await confirmNames(w).trigger('click')
    await flushPromises()

    // Empty is an answer, not a missing one: a component that treated a blank
    // field as "unanswered" and refused to send it would fail here.
    expect(invoke).toHaveBeenCalledWith(
      'extract_archive',
      expect.objectContaining({ replacements: { '?': '' } })
    )
  })

  it('extracts nothing when the question is cancelled', async () => {
    const w = await ask()

    await w.find('.naming .ghost').trigger('click') // Cancel

    expect(namingSheet(w).exists()).toBe(false)
    expect(invoke).not.toHaveBeenCalledWith('extract_archive', expect.anything())
    expect(w.text()).not.toContain('Extracted')
  })

  it('refuses a replacement this computer cannot write either', async () => {
    const w = await ask()

    await answerFields(w)[0].setValue('*')
    expect(w.find('.answer-error').text()).toContain('cannot write in a file name either')
    expect(confirmNames(w).attributes('disabled')).toBeDefined()

    // A separator is refused for its own reason: it would not rename the entry,
    // it would move it somewhere else entirely.
    await answerFields(w)[0].setValue('../')
    expect(w.find('.answer-error').text()).toContain('move the file into another folder')

    await confirmNames(w).trigger('click')
    await flushPromises()
    expect(invoke).not.toHaveBeenCalledWith('extract_archive', expect.anything())

    // And a good answer clears the way again.
    await answerFields(w)[0].setValue('-')
    expect(w.find('.answer-error').exists()).toBe(false)
    expect(confirmNames(w).attributes('disabled')).toBeUndefined()
  })

  it('brings a collision back into the dialog naming both entries', async () => {
    const message =
      'the archive entries "what?.txt" and "what_.txt" would both be written as "what_.txt"; ' +
      'choose a replacement that keeps them apart'
    const w = await ask({ extraction: { status: 'nameProblem', message } })

    await confirmNames(w).trigger('click')
    await flushPromises()

    // Nothing was written, so this is a question and not a failure: the sheet
    // stays open on it, the success screen never appears, and the error banner
    // (which means "this did not work") is not the thing that says so.
    expect(namingSheet(w).exists()).toBe(true)
    expect(w.find('.name-problem').text()).toBe(message)
    expect(w.text()).not.toContain('Extracted')
    expect(w.find('.error').exists()).toBe(false)
  })

  it('shows a naming refusal in the banner when no dialog is open to hold it', async () => {
    // The report and the extractor read the archive in two separate passes and
    // can disagree: a listing the first pass could not read is reported as
    // "nothing to ask", and then extraction refuses a name. `nameProblem` is
    // rendered only inside the sheet, so this combination used to make the
    // Extract button do nothing at all: no files, no question, no banner.
    const message = 'the archive entry "x:y.txt" contains \':\' and no replacement for it was given'
    const w = await ask({
      report: NOTHING_TO_ASK,
      extraction: { status: 'nameProblem', message },
    })

    expect(namingSheet(w).exists()).toBe(false)
    expect(w.find('.error').text()).toContain(message)
    expect(w.text()).not.toContain('Extracted')
  })

  it('states the adjustment for a problem with no character to replace', async () => {
    // A trailing dot and a device name have nothing to substitute: the host
    // would mangle them whatever anyone typed. So they are explained, not
    // asked about, and a text field beside them would be a lie.
    const w = await ask({
      report: inspection({
        entries: [
          { entry: 'notes.txt.', problems: [{ kind: 'trailingCharacters', removed: '.' }] },
          { entry: 'CON.txt', problems: [{ kind: 'reservedDevice', device: 'CON' }] },
        ],
      }),
    })

    expect(answerFields(w)).toHaveLength(0)
    expect(w.text()).toContain('"notes.txt." ends in "."')
    expect(w.text()).toContain('is the "CON" device in every folder')

    await confirmNames(w).trigger('click')
    await flushPromises()

    expect(invoke).toHaveBeenCalledWith(
      'extract_archive',
      expect.objectContaining({ replacements: {} })
    )
  })

  it('abandons the question when another archive is dropped on it', async () => {
    const w = await ask()
    expect(namingSheet(w).exists()).toBe(true)

    // A drop reaches the webview even with the sheet on screen. Keeping the
    // question up would let one archive's answers be applied to another
    // archive's names, which is a rename nobody asked for.
    const onDrop = onDragDropEvent.mock.calls[0][0]
    onDrop({ payload: { type: 'drop', paths: ['/Users/me/other.zip'] } })
    await flushPromises()

    expect(namingSheet(w).exists()).toBe(false)
    expect(invoke).not.toHaveBeenCalledWith('extract_archive', expect.anything())
  })

  it('gives way to the error banner when the failure is not about names', async () => {
    const w = await ask()

    invoke.mockImplementation(async (cmd) => {
      if (cmd === 'extract_archive') throw 'IO error: No space left on device'
      return null
    })
    await confirmNames(w).trigger('click')
    await flushPromises()

    // A full disk is nothing the dialog can help with, so it gets out of the
    // way instead of holding a question the user cannot answer.
    expect(namingSheet(w).exists()).toBe(false)
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
