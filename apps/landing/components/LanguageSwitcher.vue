<script setup>
// Language menu for the navbar. A dropdown rather than inline links so the
// header stays the same width no matter how many locales exist: the list
// scrolls once it outgrows the menu's max height.
const { t, locale, locales } = useI18n()
const switchLocalePath = useSwitchLocalePath()

const open = ref(false)
const root = ref(null)

const current = computed(() => locales.value.find((l) => l.code === locale.value))

// The menu is a plain list of links, so it is left in the DOM (hidden with
// v-show) and stays crawlable even while closed.
function close() {
  open.value = false
}

function onDocumentClick(event) {
  if (root.value && !root.value.contains(event.target)) close()
}

function onKeydown(event) {
  if (event.key === 'Escape') close()
}

onMounted(() => {
  document.addEventListener('click', onDocumentClick)
  document.addEventListener('keydown', onKeydown)
})

onBeforeUnmount(() => {
  document.removeEventListener('click', onDocumentClick)
  document.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <div ref="root" class="lang">
    <button
      type="button"
      class="lang-btn"
      :aria-label="t('nav.language')"
      aria-haspopup="true"
      :aria-expanded="open ? 'true' : 'false'"
      aria-controls="lang-menu"
      @click="open = !open"
    >
      <svg class="lang-globe" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <circle cx="12" cy="12" r="9" />
        <path d="M3 12h18" />
        <path d="M12 3c2.5 2.6 3.8 5.7 3.8 9S14.5 18.4 12 21c-2.5-2.6-3.8-5.7-3.8-9S9.5 5.6 12 3z" />
      </svg>
      <span class="lang-code">{{ current?.code.toUpperCase() }}</span>
      <svg class="lang-caret" :class="{ up: open }" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <path d="M6 9l6 6 6-6" />
      </svg>
    </button>

    <ul v-show="open" id="lang-menu" class="lang-menu">
      <li v-for="l in locales" :key="l.code">
        <NuxtLink
          :to="switchLocalePath(l.code)"
          :hreflang="l.language"
          :lang="l.language"
          class="lang-item"
          :class="{ active: l.code === locale }"
          :aria-current="l.code === locale ? 'true' : undefined"
          @click="close"
        >
          <span class="lang-name">{{ l.name }}</span>
          <span class="lang-tag">{{ l.code.toUpperCase() }}</span>
        </NuxtLink>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.lang { position: relative; }

.lang-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 5px 9px;
  border: 1px solid var(--border-2);
  border-radius: 999px;
  background: transparent;
  color: var(--muted);
  font-family: var(--font);
  font-size: 0.74rem;
  font-weight: 700;
  letter-spacing: 0.06em;
  cursor: pointer;
  transition: color 0.2s, border-color 0.2s, background 0.2s;
}
.lang-btn:hover, .lang-btn[aria-expanded='true'] {
  color: var(--accent);
  border-color: var(--accent);
  background: var(--accent-dim);
}
.lang-globe { width: 14px; height: 14px; }
.lang-caret { width: 12px; height: 12px; transition: transform 0.2s; }
.lang-caret.up { transform: rotate(180deg); }

.lang-menu {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  z-index: 20;
  min-width: 168px;
  /* Room for roughly six languages; the rest scroll instead of growing the
     menu past the viewport. */
  max-height: 264px;
  overflow-y: auto;
  padding: 6px;
  margin: 0;
  list-style: none;
  background: var(--cream);
  border: 1px solid var(--border-2);
  border-radius: var(--r-sm);
  box-shadow: 0 10px 28px rgba(62, 54, 43, 0.12);
}

.lang-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 14px;
  padding: 8px 10px;
  border-radius: 8px;
  font-size: 0.82rem;
  color: var(--muted);
  white-space: nowrap;
  transition: color 0.2s, background 0.2s;
}
.lang-item:hover { color: var(--accent); background: var(--accent-dim); }
.lang-item.active { color: var(--accent); font-weight: 700; }

.lang-tag {
  font-size: 0.68rem;
  font-weight: 700;
  letter-spacing: 0.06em;
  color: var(--faint);
}
.lang-item:hover .lang-tag, .lang-item.active .lang-tag { color: var(--accent); }
</style>
