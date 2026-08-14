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
      meta: [
        { charset: 'utf-8' },
        { name: 'viewport', content: 'width=device-width, initial-scale=1' },
        {
          name: 'description',
          content:
            'Collapse is a small, fast, open-source file compressor. Turn files and folders into 7z, ZIP or tar archives — and back — from your desktop or your terminal. Local-first, no tracking.',
        },
      ],
    },
  },
})
