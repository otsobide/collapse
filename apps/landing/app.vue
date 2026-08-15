<script setup>
// Site shell. The page itself lives in pages/index.vue so @nuxtjs/i18n can
// route it per locale (`/` English, `/es` Spanish).
const { t } = useI18n()

// useLocaleHead supplies <html lang> and the hreflang alternates; the title
// and description come from the active locale's file.
const localeHead = useLocaleHead()

useHead(() => ({
  htmlAttrs: localeHead.value.htmlAttrs,
  link: localeHead.value.link,
  meta: [
    ...(localeHead.value.meta ?? []),
    { name: 'description', content: t('seo.description') },
  ],
  title: t('seo.title'),
}))
</script>

<template>
  <NuxtPage />
</template>
