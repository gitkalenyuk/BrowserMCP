<script lang="ts">
  import { FitAddon } from '@xterm/addon-fit';
  import { Terminal, type ITheme } from '@xterm/xterm';
  import '@xterm/xterm/css/xterm.css';
  import { onMount } from 'svelte';
  import { settings } from '../../settings.svelte';
  import type { TerminalStore } from '../../term/store.svelte';

  let { id, store, active }: { id: string; store: TerminalStore; active: boolean } = $props();

  const DARK: ITheme = {
    background: '#0d0f12',
    foreground: '#c9d1d9',
    cursor: '#c9d1d9',
    selectionBackground: '#264f78',
    black: '#484f58',
    red: '#ff7b72',
    green: '#7ee787',
    yellow: '#e3b341',
    blue: '#79c0ff',
    magenta: '#d2a8ff',
    cyan: '#56d4dd',
    white: '#b1bac4',
    brightBlack: '#6e7681',
    brightRed: '#ffa198',
    brightGreen: '#56d364',
    brightYellow: '#f2cc60',
    brightBlue: '#a5d6ff',
    brightMagenta: '#e2c5ff',
    brightCyan: '#b3f0ff',
    brightWhite: '#f0f6fc',
  };
  const LIGHT: ITheme = {
    background: '#ffffff',
    foreground: '#1f2328',
    cursor: '#1f2328',
    selectionBackground: '#b6d7ff',
    black: '#24292f',
    red: '#cf222e',
    green: '#116329',
    yellow: '#7d4e00',
    blue: '#0969da',
    magenta: '#8250df',
    cyan: '#1b7c83',
    white: '#6e7781',
    brightBlack: '#57606a',
    brightRed: '#a40e26',
    brightGreen: '#1a7f37',
    brightYellow: '#633c01',
    brightBlue: '#218bff',
    brightMagenta: '#a475f9',
    brightCyan: '#3192aa',
    brightWhite: '#8c959f',
  };

  /** ANSI colours by light / dark; background, text, cursor and selection from the palette (reactive). */
  function themeNow(): ITheme {
    const tk = settings.tokens;
    const ansi = settings.effectiveTheme === 'dark' ? DARK : LIGHT;
    return { ...ansi, background: tk['code-bg'], foreground: tk['code-fg'], cursor: tk['code-fg'], selectionBackground: tk.selection };
  }

  let host: HTMLDivElement;
  let term: Terminal | undefined;
  let fit: FitAddon | undefined;
  let sent = { cols: 0, rows: 0 };
  let resizeTimer: ReturnType<typeof setTimeout> | null = null;

  /** Fit to the box and tell the PTY (debounced: dragging the panel edge fires a lot). */
  function refit() {
    if (!term || !fit || !host.offsetParent) return; // hidden: no size to measure
    try {
      fit.fit();
    } catch {
      return;
    }
    if (term.cols === sent.cols && term.rows === sent.rows) return;
    if (resizeTimer) clearTimeout(resizeTimer);
    resizeTimer = setTimeout(() => {
      if (!term) return;
      sent = { cols: term.cols, rows: term.rows };
      store.resize(id, term.cols, term.rows);
    }, 80);
  }

  onMount(() => {
    const t = new Terminal({
      fontFamily: "'Cascadia Mono', 'Cascadia Code', Consolas, 'SF Mono', Menlo, monospace",
      fontSize: 12.5,
      lineHeight: 1.15,
      cursorBlink: true,
      scrollback: 5000,
      allowProposedApi: false,
      theme: themeNow(),
    });
    const f = new FitAddon();
    t.loadAddon(f);
    t.open(host);
    term = t;
    fit = f;
    // Keystrokes (and xterm's own answers to the shell's terminal queries) go to the PTY.
    const input = t.onData((data) => store.write(id, data));
    const sink = (data: string) => t.write(data);
    const past = store.attach(id, sink);
    if (past) t.write(past);
    const ro = new ResizeObserver(() => refit());
    ro.observe(host);
    requestAnimationFrame(refit);
    return () => {
      ro.disconnect();
      input.dispose();
      store.detach(id, sink);
      if (resizeTimer) clearTimeout(resizeTimer);
      t.dispose();
      term = undefined;
    };
  });

  $effect(() => {
    const theme = themeNow();
    if (term) term.options.theme = theme;
  });

  $effect(() => {
    if (!active) return;
    requestAnimationFrame(() => {
      refit();
      term?.focus();
    });
  });
</script>

<div class="xterm-host" class:hidden={!active} bind:this={host}></div>

<style>
  .xterm-host {
    position: absolute;
    inset: 6px 4px 4px 10px;
  }
  .hidden {
    display: none;
  }
  .xterm-host :global(.xterm) {
    height: 100%;
  }
  .xterm-host :global(.xterm-viewport) {
    scrollbar-width: thin;
  }
</style>
