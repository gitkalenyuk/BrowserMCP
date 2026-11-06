<script lang="ts">
  import type { ConsoleEntry } from '../../scripts/logic';

  let { entries, onjump }: { entries: ConsoleEntry[]; onjump: (line: number) => void } = $props();

  let box: HTMLDivElement;
  // Follow the tail unless the user scrolled up.
  let stick = true;
  $effect(() => {
    void entries.length;
    if (stick && box) queueMicrotask(() => (box.scrollTop = box.scrollHeight));
  });
</script>

<div
  class="console"
  bind:this={box}
  onscroll={() => (stick = box.scrollTop + box.clientHeight >= box.scrollHeight - 8)}
  role="log"
  aria-label="Script output"
>
  {#each entries as e, i (i)}
    <div class="row {e.kind}">
      {#if e.kind === 'result'}<span class="tag">result</span>{/if}
      <span class="text">{e.text}</span>
      {#if e.line}
        <button class="jump" onclick={() => onjump(e.line!)} title="Show line {e.line} in the editor">line {e.line}</button>
      {/if}
    </div>
  {:else}
    <div class="row info">Run (▶) or debug (F5) the script; its output appears here.</div>
  {/each}
</div>

<style>
  .console {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 6px 0;
    font: 12px/1.45 var(--font-mono);
    user-select: text;
  }
  .row {
    display: flex;
    gap: 8px;
    align-items: baseline;
    padding: 1px 10px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  .stderr {
    color: var(--warn);
  }
  .error {
    color: var(--danger);
    background: color-mix(in oklab, var(--danger) 9%, transparent);
  }
  .info {
    color: var(--fg-subtle);
    font-family: var(--font);
  }
  .result {
    color: var(--ok);
  }
  .tag {
    flex: none;
    padding: 0 5px;
    border-radius: 4px;
    background: color-mix(in oklab, var(--ok) 16%, transparent);
    font: 600 10px/16px var(--font);
    text-transform: uppercase;
  }
  .jump {
    flex: none;
    border: 0;
    padding: 0 6px;
    border-radius: 4px;
    background: color-mix(in oklab, var(--danger) 16%, transparent);
    color: inherit;
    font: 600 11px/18px var(--font);
    cursor: pointer;
  }
  .jump:hover {
    text-decoration: underline;
  }
</style>
