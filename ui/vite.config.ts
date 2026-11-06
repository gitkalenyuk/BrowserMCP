import type { Plugin } from 'vite';
import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';

const page = (name: string) => fileURLToPath(new URL(name, import.meta.url));

/**
 * The Scripts panel's Monaco (lazy chunk) only edits TypeScript: replace the registration of
 * every other language (CSS/HTML/JSON workers, ~80 tokenizers) with an empty module, so the
 * build does not carry them (~3 MB less in ui/dist and in browsermcp.exe).
 */
const monacoTypeScriptOnly: Plugin = {
  name: 'bmcp-monaco-typescript-only',
  enforce: 'pre',
  load(id) {
    const path = id.replace(/\\/g, '/');
    if (!path.includes('/monaco-editor/esm/vs/languages/')) return null;
    const unused =
      /\/languages\/features\/(css|html|json)\/register\.js$/.test(path) ||
      /\/languages\/definitions\/(?!typescript\/|javascript\/)[^/]+\/register\.js$/.test(path);
    return unused ? 'export {};' : null;
  },
};

// Multi-page build: the browser chrome (index.html), the Ctrl+K palette overlay
// (palette.html) and the new tab page (newtab.html) are separate entries. In the
// real browser all are served from bmcp://ui/, so relative asset paths (base: './')
// keep working.
export default defineConfig({
  base: './',
  plugins: [svelte(), monacoTypeScriptOnly],
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
      },
    },
  },
  test: {
    include: ['src/**/*.test.ts'],
    environment: 'node',
  },
});
