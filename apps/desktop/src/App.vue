<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import {
  baseName,
  dirOf,
  isArchive,
  levelHint as levelHintFor,
  verifyNote as verifyNoteFor,
} from './paths.js'
import {
  LOCAL,
  labelFor,
  loadDestination,
  loadSources,
  makeSource,
  removeSource,
  saveDestination,
  saveSources,
  upsertSource,
  urlFor,
} from './sources.js'

const mode = ref('compress') // 'compress' | 'extract'
const inputPath = ref(null)
const inputName = ref('')
const isDir = ref(false)
const dragging = ref(false)
const processing = ref(false)
const result = ref(null) // { output } | { files, dir }
const error = ref(null)

const format = ref('zip')
const level = ref(3)
// The deeper of the two checks the archive gets before it is saved, off by
// default because it costs about as much again as the compression did. The
// cheap one (reading the listing back) is not optional and runs regardless.
const verifyContents = ref(false)

// Where compression runs. Remotes are remembered between launches; the
// dropdown is always on screen in compress mode, so the active one is never
// hidden state.
const sources = ref(loadSources())
const destination = ref(loadDestination(sources.value))
const showSources = ref(false)
const newLabel = ref('')
const newUrl = ref('')
const checking = ref(null) // source id being tested
const checkResults = ref({}) // id -> { ok, message }
const addError = ref(null)

const destinationLabel = computed(() => labelFor(sources.value, destination.value))
const serverUrl = computed(() => urlFor(sources.value, destination.value))
const isRemote = computed(() => serverUrl.value !== null)

const levelHint = computed(() => levelHintFor(level.value))
const levelDisabled = computed(() => format.value === 'tar')

// A remote run is compressed on the server and comes back as finished bytes
// this app never described, so there is no list of expected entries here to
// check them against. The row stays on screen, dimmed, rather than
// disappearing the way the destination picker does in extract mode: the picker
// goes because the whole feature does, while this one is the direct
// consequence of the choice the user just made one row above, and a control
// that vanishes under the pointer explains nothing.
const verifyDisabled = computed(() => isRemote.value)
// What is really asked for, and what the box shows. Keeping the preference in
// its own ref means picking a server does not silently forget it, and reading
// the effective value here means a disabled box can never send `true`.
const verifyRequested = computed(() => verifyContents.value && !verifyDisabled.value)
const verifyNote = computed(() =>
  verifyNoteFor({
    checked: verifyRequested.value,
    format: format.value,
    remote: isRemote.value,
  })
)

function selectDestination(value) {
  destination.value = value
  saveDestination(value)
}

function addSource() {
  addError.value = null
  const source = makeSource(newLabel.value, newUrl.value)
  if (!source) {
    addError.value = 'Enter a server address, for example http://localhost:8000'
    return
  }
  sources.value = upsertSource(sources.value, source)
  saveSources(sources.value)
  newLabel.value = ''
  newUrl.value = ''
  checkSource(source)
}

function forgetSource(id) {
  sources.value = removeSource(sources.value, id)
  saveSources(sources.value)
  // Never leave the app pointing at a server that is no longer listed.
  if (destination.value === id) selectDestination(LOCAL)
}

async function checkSource(source) {
  checking.value = source.id
  try {
    await invoke('check_server', { url: source.url })
    checkResults.value = { ...checkResults.value, [source.id]: { ok: true, message: 'Reachable' } }
  } catch (e) {
    checkResults.value = { ...checkResults.value, [source.id]: { ok: false, message: String(e) } }
  } finally {
    checking.value = null
  }
}

async function pick(path) {
  error.value = null
  result.value = null
  inputPath.value = path
  inputName.value = baseName(path)
  try {
    isDir.value = await invoke('is_directory', { path })
  } catch {
    isDir.value = false
  }
}

function setMode(next) {
  if (mode.value === next) return
  mode.value = next
  reset()
}

function reset() {
  inputPath.value = null
  inputName.value = ''
  isDir.value = false
  result.value = null
  error.value = null
}

async function browse() {
  const selected =
    mode.value === 'compress'
      ? await open({ multiple: false, title: 'Choose a file or folder', directory: false })
      : await open({ multiple: false, title: 'Choose an archive' })
  if (selected) pick(selected)
}

async function compress() {
  if (!inputPath.value || processing.value) return
  error.value = null

  const { dir, sep } = dirOf(inputPath.value)
  // For a folder "photos" → photos.zip; for a file "notes.txt" → notes.txt.zip.
  // inputName already carries the extension for files and none for folders.
  const defaultPath = `${dir}${sep}${inputName.value}.${format.value}`
  const savePath = await save({
    defaultPath,
    title: 'Save archive as',
    filters: [{ name: format.value.toUpperCase(), extensions: [format.value] }],
  })
  if (!savePath) return

  processing.value = true
  try {
    const output = await invoke('compress_path', {
      path: inputPath.value,
      output: savePath,
      format: format.value,
      level: level.value,
      server: serverUrl.value,
      // Always the effective value, never the raw preference: the checkbox is
      // disabled for a remote run, so asking for a check the server side
      // cannot make would be this app lying to itself.
      verify: verifyRequested.value,
      // The save dialog asks before handing back a path that is already
      // taken, on every platform, so reaching here means the user has
      // already agreed to replace it. The backend still refuses the cases
      // that prompt does not cover, such as a file inside the folder being
      // compressed.
      overwrite: true,
    })
    result.value = { output }
  } catch (e) {
    error.value = String(e)
  } finally {
    processing.value = false
  }
}

async function extract() {
  if (!inputPath.value || processing.value) return
  error.value = null

  const { dir } = dirOf(inputPath.value)
  const outputDir = await open({
    directory: true,
    multiple: false,
    defaultPath: dir || undefined,
    title: 'Extract into…',
  })
  if (!outputDir) return

  processing.value = true
  try {
    const files = await invoke('extract_archive', {
      archive: inputPath.value,
      outputDir,
    })
    result.value = { files, dir: outputDir }
  } catch (e) {
    error.value = String(e)
  } finally {
    processing.value = false
  }
}

const canProceed = computed(() => !!inputPath.value && !processing.value)

let unlisten = null
onMounted(async () => {
  unlisten = await getCurrentWebview().onDragDropEvent((event) => {
    const { type } = event.payload
    if (type === 'over') {
      dragging.value = true
    } else if (type === 'leave') {
      dragging.value = false
    } else if (type === 'drop') {
      dragging.value = false
      const paths = event.payload.paths
      if (paths && paths.length > 0) {
        // Auto-switch mode from the dropped item's extension.
        if (isArchive(paths[0])) mode.value = 'extract'
        else if (mode.value === 'extract') mode.value = 'compress'
        pick(paths[0])
      }
    }
  })
})
onUnmounted(() => {
  if (unlisten) unlisten()
})
</script>

<template>
  <div class="app">
    <div class="glow"></div>

    <header data-tauri-drag-region>
      <span class="wordmark">Collapse</span>
      <div class="header-right">
        <div class="modes">
          <button :class="{ on: mode === 'compress' }" @click="setMode('compress')">Compress</button>
          <button :class="{ on: mode === 'extract' }" @click="setMode('extract')">Extract</button>
        </div>
        <button class="gear" title="Servers" aria-label="Servers" @click="showSources = true">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
        </button>
      </div>
    </header>

    <!-- Servers -->
    <Transition name="fade">
      <div v-if="showSources" class="sheet" @click.self="showSources = false">
        <div class="sheet-panel">
          <div class="sheet-head">
            <p class="sheet-title">Servers</p>
            <button class="ghost small" @click="showSources = false">Done</button>
          </div>
          <p class="sheet-hint">
            Compress on another machine running <code>collapse-server-backend</code>. Files and folders
            are sent over the network, so only add servers you trust.
          </p>

          <ul v-if="sources.length" class="sources">
            <li v-for="s in sources" :key="s.id">
              <div class="source-text">
                <span class="source-label">{{ s.label }}</span>
                <span class="source-url">{{ s.url }}</span>
                <span
                  v-if="checkResults[s.id]"
                  class="source-check"
                  :class="{ bad: !checkResults[s.id].ok }"
                >{{ checkResults[s.id].message }}</span>
              </div>
              <div class="source-actions">
                <button class="ghost small" :disabled="checking === s.id" @click="checkSource(s)">
                  {{ checking === s.id ? 'Testing…' : 'Test' }}
                </button>
                <button class="ghost small danger" @click="forgetSource(s.id)">Remove</button>
              </div>
            </li>
          </ul>
          <p v-else class="sheet-empty">No servers yet. Everything compresses on this computer.</p>

          <form class="add-source" @submit.prevent="addSource">
            <input v-model="newLabel" type="text" placeholder="Name (optional)" />
            <input v-model="newUrl" type="text" placeholder="http://localhost:8000" />
            <button class="cta small" type="submit">Add</button>
          </form>
          <p v-if="addError" class="add-error">{{ addError }}</p>
        </div>
      </div>
    </Transition>

    <main>
      <Transition name="fade" mode="out-in">
        <!-- Success -->
        <div v-if="result" class="done" key="done">
          <div class="check">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M20 6L9 17l-5-5" />
            </svg>
          </div>
          <template v-if="result.output">
            <p class="done-title">Compressed</p>
            <p class="done-path">{{ result.output }}</p>
          </template>
          <template v-else>
            <p class="done-title">Extracted {{ result.files.length }} file{{ result.files.length === 1 ? '' : 's' }}</p>
            <p class="done-path">{{ result.dir }}</p>
            <ul v-if="result.files.length" class="file-list">
              <li v-for="f in result.files.slice(0, 6)" :key="f">{{ f }}</li>
              <li v-if="result.files.length > 6" class="more">+{{ result.files.length - 6 }} more</li>
            </ul>
          </template>
          <button class="ghost" @click="reset">{{ mode === 'compress' ? 'Compress' : 'Extract' }} another</button>
        </div>

        <!-- Working area -->
        <div v-else class="work" key="work">
          <div
            class="drop"
            :class="{ active: dragging, filled: inputPath }"
            @click="browse"
          >
            <template v-if="inputPath">
              <div class="file-icon">
                <svg v-if="isDir" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
                </svg>
                <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M14 3v5h5" />
                  <path d="M6 3h8l5 5v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" />
                </svg>
              </div>
              <p class="file-name">{{ inputName }}</p>
              <p class="hint">Click to choose a different {{ mode === 'extract' ? 'archive' : 'item' }}</p>
            </template>
            <template v-else>
              <svg class="arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 4v14M6 12l6 6 6-6" />
              </svg>
              <p class="label">
                {{ mode === 'compress' ? 'Drop a file or folder' : 'Drop an archive to extract' }}
              </p>
              <p class="hint">or <button class="linkish" @click.stop="browse">browse</button></p>
            </template>
          </div>

          <!-- Compress options -->
          <Transition name="slide">
            <div v-if="inputPath && mode === 'compress'" class="panel">
              <!-- Only compression can run remotely, so this row belongs to
                   compress mode alone. -->
              <div class="row">
                <span class="row-label">Where</span>
                <select
                  class="picker"
                  :value="destination"
                  @change="selectDestination($event.target.value)"
                >
                  <option :value="LOCAL">This computer</option>
                  <option v-for="s in sources" :key="s.id" :value="s.id">{{ s.label }}</option>
                </select>
              </div>

              <div class="row">
                <span class="row-label">Format</span>
                <div class="segmented">
                  <button :class="{ on: format === 'zip' }" @click="format = 'zip'">ZIP</button>
                  <button :class="{ on: format === '7z' }" @click="format = '7z'">7z</button>
                  <button :class="{ on: format === 'tar' }" @click="format = 'tar'">TAR</button>
                </div>
              </div>

              <div class="row" :class="{ dim: levelDisabled }">
                <span class="row-label">Level<em v-if="!levelDisabled">{{ levelHint }}</em><em v-else>n/a</em></span>
                <div class="segmented levels">
                  <button
                    v-for="l in 5" :key="l"
                    :disabled="levelDisabled"
                    :class="{ on: level === l && !levelDisabled }"
                    @click="level = l"
                  >{{ l }}</button>
                </div>
              </div>

              <div class="row" :class="{ dim: verifyDisabled }">
                <span class="row-label">Verify<em v-if="verifyDisabled">n/a</em></span>
                <label class="checkbox" :class="{ on: verifyRequested }">
                  <input
                    type="checkbox"
                    :checked="verifyRequested"
                    :disabled="verifyDisabled"
                    @change="verifyContents = $event.target.checked"
                  />
                  <span>Contents</span>
                </label>
              </div>
              <p class="row-note">{{ verifyNote }}</p>

              <button class="cta" :disabled="!canProceed" @click="compress">
                <span v-if="processing" class="spinner"></span>
                {{ processing ? (isRemote ? `Compressing on ${destinationLabel}…` : 'Compressing…') : 'Compress' }}
              </button>
            </div>
          </Transition>

          <!-- Extract action -->
          <Transition name="slide">
            <div v-if="inputPath && mode === 'extract'" class="panel">
              <button class="cta" :disabled="!canProceed" @click="extract">
                <span v-if="processing" class="spinner"></span>
                {{ processing ? 'Extracting…' : 'Extract to…' }}
              </button>
            </div>
          </Transition>

          <Transition name="fade">
            <div v-if="error" class="error">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
                <circle cx="12" cy="12" r="10" />
                <line x1="15" y1="9" x2="9" y2="15" />
                <line x1="9" y1="9" x2="15" y2="15" />
              </svg>
              <span>{{ error }}</span>
            </div>
          </Transition>
        </div>
      </Transition>
    </main>
  </div>
</template>

<style>
:root {
  --bg: #ece4d5;
  --surface: #f5efe3;
  --surface-2: #efe7d8;
  --border: #e1d9c8;
  --border-2: #d3c9b5;
  --dashed: #c3b9a5;
  --text: #3e362b;
  --muted: #6e6456;
  --faint: #a99e8b;
  --accent: #bc5a38;
  --accent-hover: #a54e30;
  --accent-dim: rgba(188, 90, 56, 0.12);
  --cream: #f7f1e6;
  --success: #6f7a4a;
  --success-dim: rgba(111, 122, 74, 0.16);
  --danger: #9e3b2a;
  --danger-dim: rgba(158, 59, 42, 0.10);
  --r: 16px;
  --r-sm: 11px;
  --font: 'SF Mono', ui-monospace, 'JetBrains Mono', 'Menlo', 'Consolas', monospace;
}

* { margin: 0; padding: 0; box-sizing: border-box; }

body {
  font-family: var(--font);
  background: var(--bg);
  color: var(--text);
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  user-select: none;
  overflow: hidden;
}

.app {
  position: relative;
  display: flex;
  flex-direction: column;
  height: 100vh;
}

.glow {
  position: fixed;
  top: -35%;
  left: 50%;
  width: 560px;
  height: 560px;
  transform: translateX(-50%);
  background: radial-gradient(circle, rgba(188, 90, 56, 0.05) 0%, transparent 68%);
  pointer-events: none;
}

/* Extra top padding leaves room for the macOS traffic lights over the content */
header {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 34px 20px 6px;
}

.wordmark {
  font-size: 0.98rem;
  font-weight: 700;
  letter-spacing: -0.01em;
  color: var(--text);
  pointer-events: none;
}

.modes {
  display: flex;
  gap: 2px;
  padding: 3px;
  border-radius: var(--r-sm);
  background: var(--surface);
  border: 1px solid var(--border);
}
.modes button {
  border: none;
  background: transparent;
  color: var(--muted);
  font-family: var(--font);
  font-size: 0.76rem;
  font-weight: 600;
  padding: 5px 12px;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.18s, color 0.18s;
}
.modes button:hover { color: var(--text); }
.modes button.on { background: var(--accent-dim); color: var(--accent); }

.header-right { display: flex; align-items: center; gap: 8px; }

.gear {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  padding: 0;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--surface);
  color: var(--muted);
  cursor: pointer;
  transition: color 0.18s, border-color 0.18s;
}
.gear svg { width: 15px; height: 15px; }
.gear:hover { color: var(--accent); border-color: var(--accent); }

/* Scrolls rather than clips. The window is resizable down to `minHeight`, and
   a long file name wraps, so the options can always end up taller than the
   space: without this the Compress button is simply not there, and the app
   looks broken rather than cramped. */
main {
  position: relative;
  flex: 1;
  display: flex;
  flex-direction: column;
  padding: 10px 22px 22px;
  overflow-y: auto;
}

/* Destination picker: a native select, so the list stays usable as servers
   are added instead of a segmented control that outgrows the window. */
.picker {
  font-family: var(--font);
  font-size: 0.82rem;
  font-weight: 600;
  color: var(--text);
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  padding: 6px 10px;
  max-width: 62%;
  cursor: pointer;
}
.picker:hover { border-color: var(--accent); }

/* ---- servers sheet ---- */
.sheet {
  position: fixed;
  inset: 0;
  z-index: 10;
  display: flex;
  align-items: flex-end;
  background: rgba(62, 54, 43, 0.28);
}

.sheet-panel {
  width: 100%;
  max-height: 88vh;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 18px 20px 20px;
  background: var(--surface);
  border-top: 1px solid var(--border-2);
  border-radius: var(--r) var(--r) 0 0;
}

.sheet-head { display: flex; align-items: center; justify-content: space-between; }
.sheet-title { font-size: 0.95rem; font-weight: 700; }
.sheet-hint { font-size: 0.76rem; color: var(--muted); line-height: 1.5; }
.sheet-hint code { background: var(--surface-2); padding: 1px 4px; border-radius: 4px; }
.sheet-empty { font-size: 0.78rem; color: var(--faint); padding: 6px 0; }

.sources { list-style: none; display: flex; flex-direction: column; gap: 8px; }
.sources li {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 9px 11px;
  border: 1px solid var(--border);
  border-radius: var(--r-sm);
  background: var(--surface-2);
}
.source-text { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.source-label { font-size: 0.83rem; font-weight: 600; }
.source-url { font-size: 0.71rem; color: var(--faint); word-break: break-all; }
.source-check { font-size: 0.71rem; color: var(--success); }
.source-check.bad { color: var(--danger); }
.source-actions { display: flex; gap: 6px; flex-shrink: 0; }

.ghost.small, .cta.small {
  margin-top: 0;
  padding: 6px 11px;
  font-size: 0.74rem;
  width: auto;
}
.ghost.small.danger:hover { border-color: var(--danger); color: var(--danger); background: var(--danger-dim); }

.add-source { display: flex; gap: 6px; align-items: center; }
.add-source input {
  flex: 1;
  min-width: 0;
  font-family: var(--font);
  font-size: 0.78rem;
  color: var(--text);
  background: var(--cream);
  border: 1px solid var(--border-2);
  border-radius: var(--r-sm);
  padding: 8px 10px;
}
.add-source input:focus { outline: none; border-color: var(--accent); }
.add-error { font-size: 0.74rem; color: var(--danger); }

.work { display: flex; flex-direction: column; gap: 14px; flex: 1; }

.drop {
  flex: 0 0 auto;
  min-height: 178px;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 11px;
  text-align: center;
  padding: 24px;
  border: 1.5px dashed var(--dashed);
  border-radius: var(--r);
  background: transparent;
  cursor: pointer;
  transition: border-color 0.25s, background 0.25s, transform 0.2s;
}

.drop:hover { border-color: var(--accent); background: rgba(188, 90, 56, 0.05); }

.drop.active {
  border-style: solid;
  border-color: var(--accent);
  background: var(--accent-dim);
  transform: scale(1.005);
}

.drop.filled { border-style: solid; border-color: var(--border-2); background: transparent; }

.arrow { width: 30px; height: 30px; color: var(--accent); }

.file-icon {
  width: 54px;
  height: 54px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--accent);
  background: var(--accent-dim);
}
.file-icon svg { width: 24px; height: 24px; }

.label { font-size: 0.92rem; font-weight: 500; color: var(--muted); }
.file-name {
  font-size: 0.95rem;
  font-weight: 600;
  word-break: break-all;
  max-width: 100%;
  line-height: 1.3;
}
.hint { font-size: 0.78rem; color: var(--faint); }

.linkish {
  border: none;
  background: none;
  color: var(--accent);
  font-family: var(--font);
  font-size: inherit;
  font-weight: 600;
  cursor: pointer;
  padding: 0;
}
.linkish:hover { color: var(--accent-hover); text-decoration: underline; }

.panel { display: flex; flex-direction: column; gap: 12px; }

.row { display: flex; align-items: center; justify-content: space-between; gap: 12px; transition: opacity 0.2s; }
.row.dim { opacity: 0.5; }

.row-label {
  font-size: 0.7rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.07em;
  color: var(--faint);
  display: flex;
  align-items: baseline;
  gap: 7px;
}
.row-label em {
  font-style: normal;
  text-transform: none;
  letter-spacing: 0;
  font-size: 0.72rem;
  color: var(--accent);
}

.segmented {
  display: flex;
  gap: 3px;
  padding: 3px;
  border-radius: var(--r-sm);
  background: var(--surface);
  border: 1px solid var(--border);
}
.segmented button {
  border: none;
  background: transparent;
  color: var(--muted);
  font-family: var(--font);
  font-size: 0.82rem;
  font-weight: 600;
  padding: 6px 15px;
  border-radius: 8px;
  cursor: pointer;
  transition: background 0.18s, color 0.18s;
}
.segmented.levels button { padding: 6px 12px; }
.segmented button:hover:not(:disabled) { color: var(--text); }
.segmented button.on { background: var(--accent-dim); color: var(--accent); }
.segmented button:disabled { cursor: not-allowed; }

/* Verify: a checkbox dressed as one of the pills above it, so the options row
   reads as one set of controls rather than a form field bolted on. */
.checkbox {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 12px;
  border-radius: var(--r-sm);
  background: var(--surface);
  border: 1px solid var(--border);
  color: var(--muted);
  font-size: 0.82rem;
  font-weight: 600;
  cursor: pointer;
  transition: background 0.18s, color 0.18s, border-color 0.18s;
}
.checkbox:hover { color: var(--text); }
.checkbox.on { background: var(--accent-dim); border-color: var(--accent); color: var(--accent); }
.checkbox input {
  width: 13px;
  height: 13px;
  margin: 0;
  accent-color: var(--accent);
  cursor: pointer;
}
/* Not `:has(input:disabled)`: the dimmed row already says the control is out,
   and this way the rule needs nothing the oldest supported webview lacks. */
.row.dim .checkbox, .checkbox input:disabled { cursor: not-allowed; }

/* The line under the Verify row: it has to fit a sentence, which is more than
   the `em` beside a row label can carry. */
.row-note {
  margin-top: -5px;
  font-size: 0.71rem;
  line-height: 1.45;
  color: var(--faint);
}

.cta {
  margin-top: 2px;
  width: 100%;
  padding: 13px;
  border: none;
  border-radius: var(--r-sm);
  background: var(--accent);
  color: var(--cream);
  font-family: var(--font);
  font-size: 0.92rem;
  font-weight: 700;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 9px;
  transition: background 0.2s, transform 0.15s;
}
.cta:hover:not(:disabled) { background: var(--accent-hover); transform: translateY(-1px); }
.cta:active:not(:disabled) { transform: translateY(0); }
.cta:disabled { opacity: 0.55; cursor: not-allowed; }

.spinner {
  width: 15px;
  height: 15px;
  border: 2px solid rgba(247, 241, 230, 0.4);
  border-top-color: var(--cream);
  border-radius: 50%;
  animation: spin 0.6s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }

.done {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  text-align: center;
  padding: 16px;
}
.check {
  width: 56px;
  height: 56px;
  border-radius: 50%;
  background: var(--success-dim);
  color: var(--success);
  display: flex;
  align-items: center;
  justify-content: center;
  margin-bottom: 2px;
}
.check svg { width: 26px; height: 26px; }
.done-title { font-size: 1rem; font-weight: 700; color: var(--success); }
.done-path { font-size: 0.8rem; color: var(--faint); word-break: break-all; line-height: 1.4; max-width: 90%; }

.file-list {
  list-style: none;
  margin-top: 4px;
  width: 100%;
  max-width: 100%;
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 0.76rem;
  color: var(--muted);
}
.file-list li { word-break: break-all; }
.file-list .more { color: var(--faint); }

.ghost {
  margin-top: 8px;
  padding: 9px 22px;
  border: 1px solid var(--border-2);
  border-radius: var(--r-sm);
  background: transparent;
  color: var(--muted);
  font-family: var(--font);
  font-size: 0.83rem;
  font-weight: 500;
  cursor: pointer;
  transition: border-color 0.2s, color 0.2s, background 0.2s;
}
.ghost:hover { border-color: var(--accent); color: var(--accent); background: var(--accent-dim); }

.error {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 11px 15px;
  border-radius: var(--r-sm);
  background: var(--danger-dim);
  border: 1px solid rgba(158, 59, 42, 0.18);
  color: var(--danger);
  font-size: 0.82rem;
  line-height: 1.4;
}
.error svg { width: 17px; height: 17px; flex-shrink: 0; }

.fade-enter-active, .fade-leave-active { transition: opacity 0.2s ease; }
.fade-enter-from, .fade-leave-to { opacity: 0; }

.slide-enter-active { transition: opacity 0.28s ease, transform 0.28s cubic-bezier(0.4, 0, 0.2, 1); }
.slide-enter-from { opacity: 0; transform: translateY(8px); }
</style>
