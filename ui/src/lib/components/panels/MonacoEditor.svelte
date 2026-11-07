<script lang="ts">
  // Monaco, loaded on first mount with a dynamic import (its own chunks; see scripts/monaco.ts).
  import { untrack } from 'svelte';
  import { t } from '../../i18n/index.svelte';
  import type { EditorKey, ScriptEditor } from '../../scripts/monaco';

  let {
    path,
    value,
    theme,
    breakpoints,
    verified,
    markedLine,
    markKind = 'current',
    onchange,
    onbreakpoints,
    onkey,
    oneditor,
  }: {
    path: string;
    value: string;
    theme: 'dark' | 'light';
    breakpoints: number[];
    /** Lines the debugger confirmed; null = no session (all drawn solid). */
    verified: number[] | null;
    markedLine: number | null;
    markKind?: 'current' | 'error';
    onchange: (value: string) => void;
    onbreakpoints: (lines: number[]) => void;
    onkey: (key: EditorKey) => void;
    oneditor?: (editor: ScriptEditor | null) => void;
  } = $props();

  let host: HTMLDivElement;
  let editor = $state<ScriptEditor | null>(null);
  let loadError = $state<string | null>(null);

  $effect(() => {
    let disposed = false;
    let created: ScriptEditor | null = null;
    import('../../scripts/monaco')
      .then(({ ScriptEditor }) => {
        if (disposed) return;
        created = new ScriptEditor(host, {
          value: untrack(() => value),
          path: untrack(() => path),
          theme: untrack(() => theme),
          onChange: (v) => onchange(v),
          onBreakpoints: (lines) => onbreakpoints(lines),
          onKey: (k) => onkey(k),
        });
        editor = created;
        oneditor?.(created);
      })
      .catch((err) => {
        loadError = err instanceof Error ? err.message : String(err);
      });
    return () => {
      disposed = true;
      oneditor?.(null);
      created?.dispose();
    };
  });

  // Another script (or its reloaded text) -> swap the model; typing round-trips as a no-op.
  $effect(() => editor?.open(path, value));
  $effect(() => editor?.setTheme(theme));
  $effect(() => editor?.setBreakpoints(breakpoints, verified));
  $effect(() => editor?.setMarkedLine(markedLine, markKind));
</script>

<div class="monaco" bind:this={host}>
  {#if !editor && !loadError}
    <div class="note">{t('scripts.editor.loading')}</div>
  {:else if loadError}
    <div class="note error">{t('scripts.editor.failed', { error: loadError })}</div>
  {/if}
</div>

<style>
  .monaco {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  .note {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
  .note.error {
    color: var(--danger);
    padding: 12px;
    text-align: center;
  }
  /* Decorations Monaco renders outside this component's scope. */
  .monaco :global(.bmcp-bp) {
    display: grid;
    place-items: center;
    cursor: pointer;
  }
  .monaco :global(.bmcp-bp::before) {
    content: '';
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: #e5484d;
  }
  .monaco :global(.bmcp-bp-unverified::before) {
    background: transparent;
    box-shadow: inset 0 0 0 2px #e5484d;
  }
  .monaco :global(.bmcp-current-line) {
    background: color-mix(in oklab, #f5d90a 26%, transparent);
  }
  .monaco :global(.bmcp-current-arrow::after) {
    content: '';
    position: absolute;
    left: 3px;
    top: 50%;
    width: 0;
    height: 0;
    margin-top: -5px;
    border-left: 8px solid #d4a72c;
    border-top: 5px solid transparent;
    border-bottom: 5px solid transparent;
  }
  .monaco :global(.bmcp-error-line) {
    background: color-mix(in oklab, #e5484d 18%, transparent);
  }
  .monaco :global(.margin-view-overlays .line-numbers) {
    cursor: pointer;
  }
</style>
