// Adding a language is two steps and nothing else: drop an `i18n/locales/
// <code>.json` next to the others, then add one entry here. Routing, the
// prerendered routes, the navbar menu and the hreflang tags all derive from
// this list. `name` is what the language menu shows, so write it in the
// language itself.
const DEFAULT_LOCALE = 'en'

const LOCALES = [
  { code: 'en', language: 'en', name: 'English', file: 'en.json' },
  { code: 'es', language: 'es', name: 'Español', file: 'es.json' },
]

// https://nuxt.com/docs/api/configuration/nuxt-config
export default defineNuxtConfig({
  compatibilityDate: '2025-01-01',
  devtools: { enabled: false },
  ssr: true,
  css: ['~/assets/main.css'],
  modules: ['@nuxtjs/i18n'],

  // The default locale is served at `/`, every other one under `/<code>/`.
  // Page titles, descriptions and the <html lang> come from the locale files
  // via useLocaleHead (see app.vue), so there is no static title here.
  i18n: {
    defaultLocale: DEFAULT_LOCALE,
    locales: LOCALES,
    strategy: 'prefix_except_default',
    // hreflang alternates and the canonical link must be absolute URLs, so
    // the production origin is baked in; override it (e.g. for the staging
    // subdomain) with NUXT_PUBLIC_SITE_URL.
    baseUrl: process.env.NUXT_PUBLIC_SITE_URL || 'https://collapse.cervantic.com',
    // First visit to `/` follows the browser's language, then remembers the
    // choice in a cookie so the switcher always wins afterwards.
    detectBrowserLanguage: {
      useCookie: true,
      cookieKey: 'i18n_redirected',
      redirectOn: 'root',
    },
  },

  // Every locale is prerendered explicitly, so a generate never depends on the
  // crawler finding the language menu's links.
  nitro: {
    prerender: {
      routes: LOCALES.map((l) => (l.code === DEFAULT_LOCALE ? '/' : `/${l.code}`)),
    },
  },

  app: {
    head: {
      link: [
        { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' },
        // Serif "C" of the Cervantic brand mark (same font cervantic.com
        // uses); text=C subsets the file down to that single glyph.
        { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
        { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' },
        {
          rel: 'stylesheet',
          href: 'https://fonts.googleapis.com/css2?family=Cormorant+Garamond:wght@600&text=C&display=swap',
        },
      ],
      meta: [
        { charset: 'utf-8' },
        { name: 'viewport', content: 'width=device-width, initial-scale=1' },
      ],
    },
  },
})
