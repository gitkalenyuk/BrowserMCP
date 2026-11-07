<script lang="ts">
  import { t } from '../../i18n/index.svelte';
  import type { DebugPaused } from '../../api/types-scripts';

  let { paused, onjump }: { paused: DebugPaused | null; onjump: (line: number) => void } = $props();

  /** Collapsed scopes by type (module scopes can be long). */
  let collapsed = $state<Record<string, boolean>>({});
</script>

<div class="vars">
  {#if !paused}
    <p class="hint">{t('scripts.vars.hint')}</p>
  {:else}
    <div class="where">
      {t('scripts.vars.pausedAt')} <button class="link" onclick={() => onjump(paused.line)}>{t('scripts.console.line', { line: paused.line })}</button>
      <span class="reason">{paused.reason}</span>
    </div>
    <div class="section">{t('scripts.vars.callStack')}</div>
    {#each paused.call_frames as f, i (i)}
      <button class="frame" class:top={i === 0} onclick={() => onjump(f.line)} title={f.url}>
        <span class="fn">{f.function}</span><span class="ln">:{f.line}</span>
      </button>
    {/each}
    {#each paused.scopes as scope, i (i)}
      {@const key = `${scope.type}-${i}`}
      <button class="section toggle" onclick={() => (collapsed[key] = !collapsed[key])} aria-expanded={!collapsed[key]}>
        <span class="caret" class:closed={collapsed[key]}>▾</span>{scope.type}
        <span class="count">{scope.variables.length}</span>
      </button>
      {#if !collapsed[key]}
        {#each scope.variables as v (v.name)}
          <div class="var" title="{v.name}: {v.type}">
            <span class="name">{v.name}</span>
            <span class="value t-{v.type}">{v.value}</span>
          </div>
        {:else}
          <div class="var empty">{t('scripts.vars.none')}</div>
        {/each}
      {/if}
    {/each}
  {/if}
</div>

<style>
  .vars {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 6px 0 10px;
    font-size: var(--fs-sm);
  }
  .hint {
    margin: 8px 12px;
    color: var(--fg-subtle);
  }
  .where {
    padding: 2px 12px 6px;
    color: var(--fg-muted);
  }
  .reason {
    margin-left: 6px;
    padding: 0 6px;
    border-radius: 4px;
    background: var(--sunken);
    font-size: var(--fs-xs);
  }
  .link {
    border: 0;
    padding: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .section {
    display: flex;
    align-items: center;
    gap: 4px;
    width: 100%;
    margin-top: 6px;
    padding: 3px 12px;
    border: 0;
    background: none;
    color: var(--fg-subtle);
    font: 650 var(--fs-xs) / 16px var(--font);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    text-align: left;
  }
  .toggle {
    cursor: pointer;
  }
  .toggle:hover {
    color: var(--fg);
  }
  .caret {
    display: inline-block;
    width: 12px;
    transition: transform 120ms;
  }
  .caret.closed {
    transform: rotate(-90deg);
  }
  .count {
    margin-left: auto;
    font-weight: 500;
  }
  .frame {
    display: flex;
    width: 100%;
    padding: 2px 12px 2px 26px;
    border: 0;
    background: none;
    color: var(--fg-muted);
    font: 12px/1.5 var(--font-mono);
    text-align: left;
    cursor: pointer;
  }
  .frame:hover {
    background: var(--hover);
  }
  .frame.top {
    color: var(--fg);
  }
  .ln {
    color: var(--fg-subtle);
  }
  .var {
    display: flex;
    gap: 8px;
    padding: 1px 12px 1px 26px;
    font: 12px/1.5 var(--font-mono);
  }
  .var.empty {
    color: var(--fg-subtle);
  }
  .name {
    flex: none;
    max-width: 40%;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--g-purple);
  }
  .name::after {
    content: ':';
    color: var(--fg-subtle);
  }
  .value {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }
  .t-string {
    color: var(--g-green);
  }
  .t-number,
  .t-boolean,
  .t-bigint {
    color: var(--g-blue);
  }
  .t-undefined,
  .t-null {
    color: var(--fg-subtle);
  }
  .t-function {
    color: var(--g-cyan);
    font-style: italic;
  }
</style>
