import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vite'
import preact from '@preact/preset-vite'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig(({ isSsrBuild }) => ({
  plugins: [preact(), tailwindcss()],
  resolve: {
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) }
  },
  build: {
    // The SSR build shares dist/ with the client build and must not wipe
    // index.html — the SSR step emits no HTML of its own.
    emptyOutDir: !isSsrBuild
  },
  ssr: {
    // Bundle deps into the SSR build so the preact/compat alias applies to
    // them too — externalized motion/lucide would import real react instead.
    noExternal: true
  }
}))
