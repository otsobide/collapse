<script setup>
import { ref, computed, onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open, save } from '@tauri-apps/plugin-dialog'
import { getCurrentWebview } from '@tauri-apps/api/webview'

const ARCHIVE_EXTS = ['zip', '7z', 'tar']

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

const levelHint = computed(() =>
  level.value <= 2 ? 'Faster' : level.value >= 4 ? 'Smaller' : 'Balanced'
)
const levelDisabled = computed(() => format.value === 'tar')

function baseName(path) {
  return path.split(/[\\/]/).pop()
}
function dirOf(path) {
  const sep = path.includes('\\') ? '\\' : '/'
  const i = path.lastIndexOf(sep)
  return { dir: i >= 0 ? path.slice(0, i) : '', sep }
}
function extOf(path) {
  const name = baseName(path)
  const i = name.lastIndexOf('.')
  return i >= 0 ? name.slice(i + 1).toLowerCase() : ''
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
        if (ARCHIVE_EXTS.includes(extOf(paths[0]))) mode.value = 'extract'
        else if (mode.value === 'extract' && !ARCHIVE_EXTS.includes(extOf(paths[0]))) {
          mode.value = 'compress'
        }
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
      <div class="modes">
        <button :class="{ on: mode === 'compress' }" @click="setMode('compress')">Compress</button>
        <button :class="{ on: mode === 'extract' }" @click="setMode('extract')">Extract</button>
      </div>
    </header>

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

              <button class="cta" :disabled="!canProceed" @click="compress">
                <span v-if="processing" class="spinner"></span>
                {{ processing ? 'Compressing…' : 'Compress' }}
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

main {
  position: relative;
  flex: 1;
  display: flex;
  flex-direction: column;
  padding: 10px 22px 22px;
}

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
