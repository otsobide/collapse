<script setup>
const repo = 'https://github.com/otsobide/collapse'

// Resolved client-side from the latest GitHub release (the .dmg asset name
// embeds the version, so a hardcoded URL would go stale on every release).
// Until it resolves — and as the no-JS / API-failure fallback — the button
// points at the releases page.
const macDownload = ref(null)
onMounted(async () => {
  try {
    const res = await fetch('https://api.github.com/repos/otsobide/collapse/releases/latest')
    if (!res.ok) return
    const release = await res.json()
    const dmg = release.assets?.find((a) => a.name.endsWith('.dmg'))
    if (dmg) macDownload.value = { url: dmg.browser_download_url, version: release.tag_name }
  } catch {
    // keep the fallback link
  }
})

const features = [
  { title: 'Files & folders', body: 'Compress a single file or an entire directory tree — the structure is preserved.' },
  { title: '7z · ZIP · tar', body: 'Standard archives any tool can open, with five compression levels.' },
  { title: 'Compress & extract', body: 'It goes both ways. The format is detected automatically on the way out.' },
  { title: 'Desktop & terminal', body: 'A native app and a command-line tool, sharing exactly the same engine.' },
  { title: 'macOS · Windows · Linux', body: 'One small codebase, running natively everywhere.' },
  { title: 'Safe by design', body: 'Extraction can never escape your folder, and nothing ever leaves your machine.' },
]

const principles = [
  { n: '01', title: 'Open by default', body: 'The code is public. Read it, fork it, and trust exactly what it does — no black boxes.' },
  { n: '02', title: 'Local-first & private', body: 'Your files stay on your device. Nothing is tracked, and nothing runs behind your back.' },
  { n: '03', title: 'Craft over bloat', body: 'One thing done well: it starts instantly and feels good to use every day.' },
]
</script>

<template>
  <div class="glow" />

  <header class="site-header">
    <div class="wrap bar">
      <a class="brand" href="#top">Collapse</a>
      <nav class="nav">
        <a href="#features">Features</a>
        <a href="#get">Download</a>
        <a :href="repo" target="_blank" rel="noopener">GitHub ↗</a>
      </nav>
    </div>
  </header>

  <main id="top">
    <!-- Hero -->
    <section class="hero">
      <div class="wrap hero-grid">
        <div class="hero-copy">
          <p class="eyebrow">cervantic</p>
          <h1>A small, fast<br />file compressor.</h1>
          <p class="lead">
            Drag in a file or a folder, pick a format, and get a smaller archive in
            seconds. Compress and extract 7z, ZIP and tar — on your desktop or in
            your terminal. Local, open-source, refreshingly simple.
          </p>
          <div class="cta-row">
            <a class="btn btn-primary" href="#get">Download for macOS →</a>
            <a class="btn btn-ghost" href="#features">See what it does</a>
          </div>
          <p class="tagline">Open source · Local-first · No tracking</p>
        </div>

        <div class="hero-visual" aria-hidden="true">
          <div class="drop">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 4v14M6 12l6 6 6-6" />
            </svg>
            <p class="drop-label">Drop a file or folder</p>
            <p class="drop-hint">or browse</p>
          </div>
          <div class="chips">
            <span>ZIP</span><span>7z</span><span>TAR</span>
          </div>
        </div>
      </div>
    </section>

    <!-- Features -->
    <section id="features" class="section">
      <div class="wrap">
        <p class="eyebrow">What it does</p>
        <h2>Everything you need to squeeze a file. Nothing you don't.</h2>
        <div class="grid">
          <article v-for="f in features" :key="f.title" class="card">
            <h3>{{ f.title }}</h3>
            <p>{{ f.body }}</p>
          </article>
        </div>
      </div>
    </section>

    <!-- Principles -->
    <section class="section alt">
      <div class="wrap">
        <p class="eyebrow">How it's built</p>
        <div class="principles">
          <div v-for="p in principles" :key="p.n" class="principle">
            <span class="pnum">{{ p.n }}</span>
            <div>
              <h3>{{ p.title }}</h3>
              <p>{{ p.body }}</p>
            </div>
          </div>
        </div>
      </div>
    </section>

    <!-- Download -->
    <section id="get" class="section">
      <div class="wrap get-grid">
        <div>
          <p class="eyebrow">Download</p>
          <h2>Two ways to use it.</h2>
          <p class="lead sm">
            A native desktop window for drag-and-drop, or a single command for
            scripts and the shell. Same engine, same results.
          </p>
          <div class="dl">
            <a class="btn btn-primary" :href="macDownload ? macDownload.url : `${repo}/releases/latest`">
               Download for macOS{{ macDownload ? ` · ${macDownload.version}` : '' }}
            </a>
            <p class="dl-meta">Universal app — Apple Silicon &amp; Intel · macOS 10.15+</p>
            <p class="dl-meta faint">
              Unsigned build for now: right-click → Open on first launch.
              Windows and Linux builds are on the way — the
              <a :href="`${repo}/releases/latest`" target="_blank" rel="noopener">release page</a>
              also has the CLI and checksums.
            </p>
          </div>
        </div>
        <div class="terminal">
          <div class="term-bar"><i /><i /><i /></div>
          <pre><span class="c">$</span> collapse compress photos/ <span class="f">-f 7z</span>
<span class="o">Created photos.7z</span>

<span class="c">$</span> collapse extract report.zip <span class="f">-o ./out</span>
<span class="o">Extracted 3 files into ./out</span></pre>
        </div>
      </div>
    </section>
  </main>

  <footer class="site-footer">
    <div class="wrap foot">
      <span class="brand">Collapse</span>
      <span class="foot-meta">Open source · Local-first · No tracking · part of cervantic</span>
      <a :href="repo" target="_blank" rel="noopener">GitHub ↗</a>
    </div>
  </footer>
</template>

<style scoped>
.glow {
  position: fixed;
  top: -280px;
  left: 50%;
  width: 900px;
  height: 900px;
  transform: translateX(-50%);
  background: radial-gradient(circle, rgba(188, 90, 56, 0.06) 0%, transparent 62%);
  pointer-events: none;
  z-index: 0;
}
header, main, footer { position: relative; z-index: 1; }

/* Header */
.site-header { padding: 22px 0; }
.bar { display: flex; align-items: center; justify-content: space-between; }
.brand { font-weight: 700; letter-spacing: -0.01em; }
.nav { display: flex; gap: 22px; font-size: 0.86rem; color: var(--muted); }
.nav a:hover { color: var(--accent); }

/* Hero */
.hero { padding: 56px 0 72px; }
.hero-grid { display: grid; grid-template-columns: 1.15fr 0.85fr; gap: 48px; align-items: center; }
h1 { font-size: 3rem; line-height: 1.08; letter-spacing: -0.02em; margin: 14px 0 20px; }
.lead { color: var(--muted); font-size: 1.02rem; max-width: 46ch; }
.lead.sm { font-size: 0.95rem; margin-bottom: 22px; }
.cta-row { display: flex; gap: 12px; margin: 26px 0 16px; flex-wrap: wrap; }
.tagline { font-size: 0.78rem; color: var(--faint); letter-spacing: 0.04em; }

.hero-visual { display: flex; flex-direction: column; align-items: center; gap: 16px; }
.drop {
  width: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 46px 24px;
  border: 1.5px dashed var(--dashed);
  border-radius: var(--r);
  background: var(--surface);
  text-align: center;
}
.drop svg { width: 34px; height: 34px; color: var(--accent); }
.drop-label { font-weight: 600; color: var(--muted); }
.drop-hint { font-size: 0.8rem; color: var(--faint); }
.chips { display: flex; gap: 8px; }
.chips span {
  font-size: 0.74rem;
  font-weight: 700;
  color: var(--accent);
  background: var(--accent-dim);
  padding: 5px 12px;
  border-radius: 999px;
}

/* Sections */
.section { padding: 72px 0; }
.section.alt { background: var(--surface); border-top: 1px solid var(--border); border-bottom: 1px solid var(--border); }
h2 { font-size: 1.7rem; line-height: 1.2; letter-spacing: -0.01em; margin: 12px 0 34px; max-width: 22ch; }

.grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 16px; }
.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--r);
  padding: 22px;
  transition: border-color 0.2s, transform 0.2s;
}
.card:hover { border-color: var(--border-2); transform: translateY(-2px); }
.card h3 { font-size: 0.98rem; margin-bottom: 8px; }
.card p { font-size: 0.87rem; color: var(--muted); }

.principles { display: grid; grid-template-columns: repeat(3, 1fr); gap: 28px; }
.principle { display: flex; gap: 14px; }
.pnum { font-size: 0.8rem; font-weight: 700; color: var(--accent); padding-top: 3px; }
.principle h3 { font-size: 0.98rem; margin-bottom: 6px; }
.principle p { font-size: 0.87rem; color: var(--muted); }

/* Download */
.get-grid { display: grid; grid-template-columns: 0.9fr 1.1fr; gap: 40px; align-items: center; }
.dl { display: flex; flex-direction: column; gap: 10px; align-items: flex-start; }
.dl-meta { font-size: 0.8rem; color: var(--muted); }
.dl-meta.faint { font-size: 0.76rem; color: var(--faint); max-width: 40ch; }
.dl-meta a { text-decoration: underline; text-underline-offset: 2px; }
.dl-meta a:hover { color: var(--accent); }
.terminal {
  background: #2b2620;
  border-radius: var(--r);
  overflow: hidden;
  border: 1px solid #1f1b16;
  box-shadow: 0 18px 40px -24px rgba(62, 54, 43, 0.5);
}
.term-bar { display: flex; gap: 7px; padding: 12px 14px; background: #332d26; }
.term-bar i { width: 11px; height: 11px; border-radius: 50%; background: #4d453b; }
.terminal pre {
  margin: 0;
  padding: 18px 20px 22px;
  font-family: var(--font);
  font-size: 0.86rem;
  line-height: 1.9;
  color: #e9e0d0;
  white-space: pre-wrap;
  word-break: break-word;
}
.terminal .c { color: var(--accent); }
.terminal .f { color: #c9b892; }
.terminal .o { color: #9aa06a; }

/* Footer */
.site-footer { padding: 30px 0; border-top: 1px solid var(--border); }
.foot { display: flex; align-items: center; justify-content: space-between; gap: 16px; flex-wrap: wrap; }
.foot-meta { font-size: 0.76rem; color: var(--faint); letter-spacing: 0.03em; }
.site-footer a:hover { color: var(--accent); }

/* Responsive */
@media (max-width: 820px) {
  .hero-grid, .get-grid { grid-template-columns: 1fr; gap: 32px; }
  .grid, .principles { grid-template-columns: 1fr; }
  h1 { font-size: 2.4rem; }
  .nav a:first-child { display: none; }
}
</style>
