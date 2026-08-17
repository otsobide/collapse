<script setup>
const { t } = useI18n()

const repo = 'https://github.com/otsobide/collapse'
const releasePage = `${repo}/releases/latest`

// Download links are resolved client-side from the latest GitHub release
// (asset names embed the version, so hardcoded URLs would go stale on every
// release). Until it resolves, and as the no-JS / API-failure fallback,
// available buttons point at the releases page.
const release = ref(null)
onMounted(async () => {
  try {
    const res = await fetch('https://api.github.com/repos/otsobide/collapse/releases/latest')
    if (res.ok) release.value = await res.json()
  } catch {
    // keep the fallback links
  }
})

const version = computed(() => release.value?.tag_name)

function assetUrl(ext) {
  const asset = release.value?.assets?.find((a) => a.name.toLowerCase().endsWith(ext))
  return asset?.browser_download_url
}

// One row per OS. A download without `href` links to the releases page;
// `soon: true` renders as a disabled chip instead of a button. AppImage and
// the Windows installers are derived from the release assets, so each turns
// into a real download the moment such an asset ships, with no page change
// needed. File extensions stay untranslated; only the prose around them is
// localized.
const systems = computed(() => [
  {
    name: 'MacOS',
    detail: t('download.systems.macos'),
    downloads: [{ label: '.dmg', href: assetUrl('.dmg') }],
  },
  {
    name: 'Linux',
    detail: t('download.systems.linux'),
    downloads: [
      { label: '.deb', href: assetUrl('.deb') },
      { label: '.rpm', href: assetUrl('.rpm') },
      { label: 'AppImage', href: assetUrl('.appimage'), soon: !assetUrl('.appimage') },
    ],
  },
  {
    name: 'Windows',
    detail: t('download.systems.windows'),
    downloads: [
      { label: '.msi', href: assetUrl('.msi'), soon: !assetUrl('.msi') },
      { label: '.exe', href: assetUrl('.exe'), soon: !assetUrl('.exe') },
    ],
  },
])

const features = computed(() =>
  ['folders', 'formats', 'both', 'modes', 'interfaces', 'safe'].map((key) => ({
    key,
    title: t(`features.items.${key}.title`),
    body: t(`features.items.${key}.body`),
  })),
)
</script>

<template>
  <div class="glow" />

  <header class="site-header">
    <div class="wrap bar">
      <a class="brand" href="#top">
        <span class="brand-mark" role="img" aria-label="Cervantic">
          <span class="brand-letter">C</span>
        </span>
        <span class="brand-name">Cervantic</span>
      </a>
      <nav class="nav">
        <a href="#download">{{ t('nav.download') }}</a>
        <a href="#features">{{ t('nav.features') }}</a>
        <a :href="repo" target="_blank" rel="noopener">{{ t('nav.github') }}</a>
        <LanguageSwitcher />
      </nav>
    </div>
  </header>

  <main id="top">
    <!-- Hero -->
    <section class="hero">
      <div class="wrap hero-grid">
        <div class="hero-copy">
          <p class="eyebrow">{{ t('hero.eyebrow') }}</p>
          <h1>{{ t('hero.titleLine1') }}<br />{{ t('hero.titleLine2') }}</h1>
          <p class="lead">{{ t('hero.lead') }}</p>
          <div class="cta-row">
            <a class="btn btn-primary" href="#download">{{ t('hero.ctaDownload') }}</a>
            <a class="btn btn-ghost" href="#features">{{ t('hero.ctaFeatures') }}</a>
          </div>
          <p class="tagline">{{ t('hero.tagline') }}</p>
        </div>

        <div class="hero-visual" aria-hidden="true">
          <div class="drop">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 4v14M6 12l6 6 6-6" />
            </svg>
            <p class="drop-label">{{ t('hero.dropLabel') }}</p>
            <p class="drop-hint">{{ t('hero.dropHint') }}</p>
          </div>
          <div class="chips">
            <span>ZIP</span><span>7z</span><span>TAR</span>
          </div>
        </div>
      </div>
    </section>

    <!-- Download -->
    <section id="download" class="section alt">
      <div class="wrap">
        <p class="eyebrow">{{ t('download.eyebrow') }}</p>
        <h2>{{ t('download.title') }}</h2>
        <div class="dl-list">
          <article
            v-for="sys in systems"
            :key="sys.name"
            class="dl-row"
            :class="{ soon: sys.downloads.every((d) => d.soon) }"
          >
            <div class="dl-os">
              <h3>{{ sys.name }}</h3>
              <p class="dl-detail">{{ sys.detail }}</p>
            </div>
            <div class="dl-actions">
              <template v-for="d in sys.downloads" :key="d.label">
                <a v-if="!d.soon" class="btn btn-primary" :href="d.href || releasePage">{{ d.label }}</a>
                <span v-else class="btn btn-soon" aria-disabled="true">{{ d.label }} · {{ t('download.soon') }}</span>
              </template>
            </div>
          </article>
        </div>
        <p class="dl-note">
          <template v-if="version">{{ t('download.latest', { version }) }} · </template>
          {{ t('download.unsigned') }}
          <a :href="releasePage" target="_blank" rel="noopener">{{ t('download.releasePage') }}</a>.
        </p>
      </div>
    </section>

    <!-- Features -->
    <section id="features" class="section">
      <div class="wrap">
        <p class="eyebrow">{{ t('features.eyebrow') }}</p>
        <h2>{{ t('features.title') }}</h2>
        <div class="grid">
          <article v-for="f in features" :key="f.key" class="card">
            <h3>{{ f.title }}</h3>
            <p>{{ f.body }}</p>
          </article>
        </div>
      </div>
    </section>
  </main>

  <footer class="site-footer">
    <div class="wrap foot">
      <span class="brand">Collapse</span>
      <span class="foot-meta">{{ t('footer.meta') }}</span>
      <a :href="repo" target="_blank" rel="noopener">{{ t('nav.github') }}</a>
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
/* The header outranks main/footer instead of tying with them: a tie would let
   the later siblings paint over it, and since `header` is its own stacking
   context, the language menu cannot escape it however high its own z-index is.
   The menu drops into the hero, so a tie left it visible but unclickable. */
header { position: relative; z-index: 10; }
main, footer { position: relative; z-index: 1; }

/* Header */
.site-header { padding: 22px 0; }
.bar { display: flex; align-items: center; justify-content: space-between; }
.brand { font-weight: 700; letter-spacing: -0.01em; }

/* Cervantic brand lockup (mirrors cervantic.com: terracotta rounded square
   with a serif C, wordmark in mono at weight 500). */
.site-header .brand {
  display: inline-flex;
  align-items: center;
  gap: 0.6rem;
  color: var(--accent);
}
.brand-mark {
  width: 30px;
  height: 30px;
  border-radius: 8px;
  background: currentColor;
  display: inline-grid;
  place-items: center;
  flex: none;
  user-select: none;
}
.brand-letter {
  font-family: 'Cormorant Garamond', Garamond, 'Times New Roman', serif;
  font-weight: 600;
  font-size: 20px;
  line-height: 1;
  color: var(--cream);
  transform: translateY(0.02em);
}
.brand-name {
  font-size: 0.98rem;
  font-weight: 500;
  letter-spacing: -0.01em;
  color: var(--text);
}
.nav { display: flex; align-items: center; gap: 22px; font-size: 0.86rem; color: var(--muted); }
.nav a:hover { color: var(--accent); }

/* Hero */
.hero { padding: 56px 0 72px; }
.hero-grid { display: grid; grid-template-columns: 1.15fr 0.85fr; gap: 48px; align-items: center; }
h1 { font-size: 3rem; line-height: 1.08; letter-spacing: -0.02em; margin: 14px 0 20px; }
.lead { color: var(--muted); font-size: 1.02rem; max-width: 46ch; }
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
h2 { font-size: 1.7rem; line-height: 1.2; letter-spacing: -0.01em; margin: 12px 0 34px; max-width: 30ch; }

/* Download */
.dl-list {
  display: flex;
  flex-direction: column;
  background: var(--cream);
  border: 1px solid var(--border);
  border-radius: var(--r);
}
.dl-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 24px;
  padding: 22px 24px;
}
.dl-row + .dl-row { border-top: 1px solid var(--border); }
.dl-row h3 { font-size: 1.05rem; }
.dl-detail { font-size: 0.8rem; color: var(--muted); margin-top: 4px; }
.dl-row.soon h3, .dl-row.soon .dl-detail { color: var(--faint); }
.dl-actions { display: flex; gap: 10px; flex-wrap: wrap; justify-content: flex-end; }
.btn-soon {
  background: transparent;
  color: var(--faint);
  border: 1px dashed var(--border-2);
  cursor: default;
}
.dl-note { margin-top: 18px; font-size: 0.78rem; color: var(--faint); }
.dl-note a { text-decoration: underline; text-underline-offset: 2px; }
.dl-note a:hover { color: var(--accent); }

/* Features */
.grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 16px; }
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

/* Footer */
.site-footer { padding: 30px 0; border-top: 1px solid var(--border); }
.foot { display: flex; align-items: center; justify-content: space-between; gap: 16px; flex-wrap: wrap; }
.foot-meta { font-size: 0.76rem; color: var(--faint); letter-spacing: 0.03em; }
.site-footer a:hover { color: var(--accent); }

/* Responsive */
@media (max-width: 820px) {
  .hero-grid { grid-template-columns: 1fr; gap: 32px; }
  .grid { grid-template-columns: 1fr; }
  .dl-row { flex-direction: column; align-items: flex-start; gap: 14px; }
  .dl-actions { justify-content: flex-start; }
  h1 { font-size: 2.4rem; }
  .nav { gap: 16px; }
  .nav > a:first-child { display: none; }
}
</style>
