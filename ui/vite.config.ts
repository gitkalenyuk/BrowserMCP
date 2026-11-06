import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';

const page = (name: string) => fileURLToPath(new URL(name, import.meta.url));

// Multi-page build: the browser chrome (index.html), the Ctrl+K palette overlay
// (palette.html), the new tab page (newtab.html) and the library (library.html: bookmarks,
// history, downloads, passwords, import) are separate entries. In the
// real browser all are served from bmcp://ui/, so relative asset paths (base: './')
// keep working.
export default defineConfig({
  base: './',
  plugins: [svelte()],
  server: { port: 5173, strictPort: false },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'chrome120',
    rollupOptions: {
      input: {
        chrome: page('./index.html'),
        palette: page('./palette.html'),
        newtab: page('./newtab.html'),
        library: page('./library.html'),
      },
    },
  },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
