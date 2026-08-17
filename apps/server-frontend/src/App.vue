<script setup>
import { ref, computed, onMounted } from 'vue'
import { compress as runCompress, health } from './api.js'
import { rootOf, tarball } from './tar.js'
import { FORMATS, archiveName, humanSize, levelHint as levelHintFor, savings, stepLabel } from './format.js'

const picked = ref(null) // { kind: 'file'|'folder', name, size, body, envelope }
const dragging = ref(false)
const processing = ref(false)
const steps = ref([]) // [{ status, at }]
const result = ref(null) // { url, name, size, savedPercent }
const error = ref(null)
const online = ref(null) // null while unknown, then true/false

const format = ref('zip')
const level = ref(3)

const levelHint = computed(() => levelHintFor(level.value))
const levelDisabled = computed(() => format.value === 'tar')
const outputName = computed(() => (picked.value ? archiveName(picked.value.name, format.value) : ''))
const canProceed = computed(() => !!picked.value && !processing.value)

const fileInput = ref(null)
const folderInput = ref(null)

onMounted(async () => {
  try {
    await health()
    online.value = true
  } catch {
    online.value = false
  }
})

function reset() {
  if (result.value?.url) URL.revokeObjectURL(result.value.url)
  picked.value = null
  steps.value = []
  result.value = null
  error.value = null
}

function takeFile(file) {
  reset()
  picked.value = { kind: 'file', name: file.name, size: file.size, body: file, envelope: 'none' }
}

/** Pack a directory selection into the tar envelope the backend unwraps. */
async function takeFolder(files) {
  reset()
  const list = [...files]
  const root = rootOf(list)
  if (!root) {
    error.value = 'That folder looks empty.'
    return
  }
  try {
    const entries = await Promise.all(
      list.map(async (file) => ({
        path: file.webkitRelativePath || file.name,
        data: new Uint8Array(await file.arrayBuffer()),
      })),
    )
    const body = tarball(entries)
    const size = list.reduce((total, file) => total + file.size, 0)
    picked.value = { kind: 'folder', name: root, size, body, envelope: 'tar', count: list.length }
  } catch (e) {
    error.value = String(e.message || e)
  }
}

function onDrop(event) {
  dragging.value = false
  const file = event.dataTransfer?.files?.[0]
  // Browsers hand a dropped folder over as a directory entry with no bytes,
  // so dropping is for files and the folder button is for folders.
  if (!file) return
  if (file.type === '' && file.size === 0) {
    error.value = 'Drops are for files. Use "Choose a folder" to send a whole folder.'
    return
  }
  takeFile(file)
}

async function start() {
  if (!picked.value || processing.value) return
  error.value = null
  result.value = null
  steps.value = []
  processing.value = true

  const started = performance.now()
  try {
    const { archive, name } = await runCompress(
      {
        body: picked.value.body,
        name: picked.value.name,
        algorithm: format.value,
        level: level.value,
        envelope: picked.value.envelope,
      },
      {
        onStatus: (status) => {
          steps.value = [...steps.value, { status, at: Math.round(performance.now() - started) }]
        },
      },
    )
    result.value = {
      url: URL.createObjectURL(archive),
      name,
      size: archive.size,
      savedPercent: savings(picked.value.size, archive.size),
    }
  } catch (e) {
    error.value = String(e.message || e)
  } finally {
    processing.value = false
  }
}
</script>

<template>
  <div class="app">
    <div class="glow"></div>

    <header>
      <span class="wordmark">Collapse</span>
      <span class="where" :class="{ bad: online === false }">
        {{ online === false ? 'server unreachable' : 'on the server' }}
      </span>
    </header>

    <main>
      <!-- Result -->
      <div v-if="result" class="done">
        <div class="check">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M20 6L9 17l-5-5" />
          </svg>
        </div>
        <p class="done-title">Compressed</p>
        <p class="done-path">{{ result.name }}</p>
        <p class="done-meta">
          {{ humanSize(result.size) }}
          <template v-if="result.savedPercent"> · {{ result.savedPercent }}% smaller</template>
        </p>
        <a class="cta" :href="result.url" :download="result.name">Save {{ result.name }}</a>
        <button class="ghost" @click="reset">Compress another</button>
      </div>

      <!-- Working area -->
      <div v-else class="work">
        <div
          class="drop"
          :class="{ active: dragging, filled: picked }"
          @dragover.prevent="dragging = true"
          @dragleave="dragging = false"
          @drop.prevent="onDrop"
          @click="fileInput?.click()"
        >
          <template v-if="picked">
            <div class="file-icon">
              <svg v-if="picked.kind === 'folder'" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
              </svg>
              <svg v-else viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
                <path d="M14 3v5h5" />
                <path d="M6 3h8l5 5v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z" />
              </svg>
            </div>
            <p class="file-name">{{ picked.name }}</p>
            <p class="hint">
              {{ humanSize(picked.size) }}
              <template v-if="picked.kind === 'folder'"> · {{ picked.count }} files</template>
            </p>
          </template>
          <template v-else>
            <svg class="arrow" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 4v14M6 12l6 6 6-6" />
            </svg>
            <p class="label">Drop a file to compress</p>
            <p class="hint">it is sent to the server and comes back smaller</p>
          </template>
        </div>

        <div class="pickers">
          <button class="ghost small" @click="fileInput?.click()">Choose a file</button>
          <button class="ghost small" @click="folderInput?.click()">Choose a folder</button>
        </div>
        <input ref="fileInput" type="file" hidden @change="(e) => e.target.files[0] && takeFile(e.target.files[0])" />
        <input ref="folderInput" type="file" webkitdirectory hidden @change="(e) => takeFolder(e.target.files)" />

        <div v-if="picked" class="panel">
          <div class="row">
            <span class="row-label">Format</span>
            <div class="segmented">
              <button
                v-for="f in FORMATS" :key="f"
                :class="{ on: format === f }"
                @click="format = f"
              >{{ f === '7z' ? '7z' : f.toUpperCase() }}</button>
            </div>
          </div>

          <div class="row" :class="{ dim: levelDisabled }">
            <span class="row-label">Level<em>{{ levelDisabled ? 'n/a' : levelHint }}</em></span>
            <div class="segmented levels">
              <button
                v-for="l in 5" :key="l"
                :disabled="levelDisabled"
                :class="{ on: level === l && !levelDisabled }"
                @click="level = l"
              >{{ l }}</button>
            </div>
          </div>

          <p class="output-note">→ {{ outputName }}</p>

          <button class="cta" :disabled="!canProceed" @click="start">
            <span v-if="processing" class="spinner"></span>
            {{ processing ? 'Working…' : 'Compress' }}
          </button>
        </div>

        <!-- The job flow, which is the thing a server does that a local run
             cannot show: each state the worker passes through, timed. -->
        <ol v-if="steps.length" class="steps">
          <li v-for="(s, i) in steps" :key="i">
            <span class="dot" :class="{ current: i === steps.length - 1 && processing }"></span>
            <span class="step-name">{{ stepLabel(s.status) }}</span>
            <span class="step-at">{{ s.at }} ms</span>
          </li>
        </ol>

        <div v-if="error" class="error">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <circle cx="12" cy="12" r="10" />
            <line x1="15" y1="9" x2="9" y2="15" />
            <line x1="9" y1="9" x2="15" y2="15" />
          </svg>
          <span>{{ error }}</span>
        </div>
      </div>
    </main>
  </div>
</template>

<style>
/* The cervantic palette, same as the desktop app. */
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
  --danger-dim: rgba(158, 59, 42, 0.1);
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
}

.app { position: relative; min-height: 100vh; display: flex; flex-direction: column; align-items: center; }

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

header {
  position: relative;
  width: 100%;
  max-width: 560px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 26px 22px 6px;
}
.wordmark { font-size: 0.98rem; font-weight: 700; letter-spacing: -0.01em; }
.where {
  font-size: 0.72rem;
  font-weight: 600;
  color: var(--accent);
  background: var(--accent-dim);
  padding: 4px 10px;
  border-radius: 999px;
}
.where.bad { color: var(--danger); background: var(--danger-dim); }

main { position: relative; width: 100%; max-width: 560px; flex: 1; padding: 10px 22px 32px; }

.work { display: flex; flex-direction: column; gap: 14px; }

.drop {
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
  cursor: pointer;
  transition: border-color 0.25s, background 0.25s;
}
.drop:hover { border-color: var(--accent); background: rgba(188, 90, 56, 0.05); }
.drop.active { border-style: solid; border-color: var(--accent); background: var(--accent-dim); }
.drop.filled { border-style: solid; border-color: var(--border-2); background: transparent; }

.arrow { width: 30px; height: 30px; color: var(--accent); }
.file-icon {
  width: 54px; height: 54px; border-radius: 50%;
  display: flex; align-items: center; justify-content: center;
  color: var(--accent); background: var(--accent-dim);
}
.file-icon svg { width: 24px; height: 24px; }
.label { font-size: 0.92rem; font-weight: 500; color: var(--muted); }
.file-name { font-size: 0.95rem; font-weight: 600; word-break: break-all; line-height: 1.3; }
.hint { font-size: 0.78rem; color: var(--faint); }

.pickers { display: flex; gap: 8px; }

.panel { display: flex; flex-direction: column; gap: 12px; }
.row { display: flex; align-items: center; justify-content: space-between; gap: 12px; transition: opacity 0.2s; }
.row.dim { opacity: 0.5; }
.row-label {
  font-size: 0.7rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.07em;
  color: var(--faint); display: flex; align-items: baseline; gap: 7px;
}
.row-label em { font-style: normal; text-transform: none; letter-spacing: 0; font-size: 0.72rem; color: var(--accent); }

.segmented {
  display: flex; gap: 3px; padding: 3px;
  border-radius: var(--r-sm); background: var(--surface); border: 1px solid var(--border);
}
.segmented button {
  border: none; background: transparent; color: var(--muted);
  font-family: var(--font); font-size: 0.82rem; font-weight: 600;
  padding: 6px 15px; border-radius: 8px; cursor: pointer;
  transition: background 0.18s, color 0.18s;
}
.segmented.levels button { padding: 6px 12px; }
.segmented button:hover:not(:disabled) { color: var(--text); }
.segmented button.on { background: var(--accent-dim); color: var(--accent); }
.segmented button:disabled { cursor: not-allowed; }

.output-note { font-size: 0.76rem; color: var(--faint); }

.cta {
  margin-top: 2px; width: 100%; padding: 13px;
  border: none; border-radius: var(--r-sm);
  background: var(--accent); color: var(--cream);
  font-family: var(--font); font-size: 0.92rem; font-weight: 700;
  cursor: pointer; text-align: center; text-decoration: none;
  display: flex; align-items: center; justify-content: center; gap: 9px;
  transition: background 0.2s, transform 0.15s;
}
.cta:hover:not(:disabled) { background: var(--accent-hover); transform: translateY(-1px); }
.cta:disabled { opacity: 0.55; cursor: not-allowed; }

.spinner {
  width: 15px; height: 15px;
  border: 2px solid rgba(247, 241, 230, 0.4); border-top-color: var(--cream);
  border-radius: 50%; animation: spin 0.6s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }

.steps { list-style: none; display: flex; flex-direction: column; gap: 6px; padding: 4px 2px; }
.steps li { display: flex; align-items: center; gap: 9px; font-size: 0.8rem; color: var(--muted); }
.dot { width: 7px; height: 7px; border-radius: 50%; background: var(--success); flex-shrink: 0; }
.dot.current { background: var(--accent); animation: pulse 1s ease-in-out infinite; }
@keyframes pulse { 50% { opacity: 0.35; } }
.step-name { font-weight: 600; }
.step-at { margin-left: auto; color: var(--faint); font-size: 0.74rem; }

.done { display: flex; flex-direction: column; align-items: center; gap: 10px; text-align: center; padding: 28px 16px; }
.check {
  width: 56px; height: 56px; border-radius: 50%;
  background: var(--success-dim); color: var(--success);
  display: flex; align-items: center; justify-content: center;
}
.check svg { width: 26px; height: 26px; }
.done-title { font-size: 1rem; font-weight: 700; color: var(--success); }
.done-path { font-size: 0.85rem; word-break: break-all; }
.done-meta { font-size: 0.78rem; color: var(--faint); }
.done .cta { margin-top: 8px; }

.ghost {
  padding: 9px 22px;
  border: 1px solid var(--border-2); border-radius: var(--r-sm);
  background: transparent; color: var(--muted);
  font-family: var(--font); font-size: 0.83rem; cursor: pointer;
  transition: border-color 0.2s, color 0.2s, background 0.2s;
}
.ghost.small { padding: 7px 14px; font-size: 0.76rem; }
.ghost:hover { border-color: var(--accent); color: var(--accent); background: var(--accent-dim); }

.error {
  display: flex; align-items: center; gap: 9px;
  padding: 11px 15px; border-radius: var(--r-sm);
  background: var(--danger-dim); border: 1px solid rgba(158, 59, 42, 0.18);
  color: var(--danger); font-size: 0.82rem; line-height: 1.4;
}
.error svg { width: 17px; height: 17px; flex-shrink: 0; }
</style>
