import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// In development the app runs on its own port while the backend runs on
// another, so the API calls are proxied here. In production nginx does the
// same job (see nginx.conf): the app always talks to its own origin, which is
// why the backend needs no CORS layer.
const BACKEND = process.env.COLLAPSE_BACKEND || 'http://127.0.0.1:8000'
const proxied = ['/health', '/compress', '/jobs', '/openapi.json', '/docs']

export default defineConfig({
  plugins: [vue()],
  server: {
    port: 5174,
    strictPort: true,
    proxy: Object.fromEntries(proxied.map((path) => [path, { target: BACKEND, changeOrigin: true }])),
  },
})
