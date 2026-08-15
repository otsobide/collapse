// https://nuxt.com/docs/api/configuration/nuxt-config
export default defineNuxtConfig({
  compatibilityDate: '2025-01-01',
  devtools: { enabled: false },
  ssr: true,
  css: ['~/assets/main.css'],
  app: {
    head: {
      htmlAttrs: { lang: 'en' },
      title: 'Collapse — a small, fast file compressor',
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
        {
          name: 'description',
          content:
            'Collapse is a small, fast, open-source file compressor for macOS, Windows and Linux. Turn files and folders into 7z, ZIP or tar archives, and back. Local-first, no tracking.',
        },
      ],
    },
  },
})
