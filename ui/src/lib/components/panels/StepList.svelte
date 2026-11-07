<script lang="ts">
  import { t } from '../../i18n/index.svelte';
  import { tick } from 'svelte';
  import type { RecordStep } from '../../api/types-record-runs';
  import { describeStep, visibleSteps } from '../../record/logic';

  let { steps, follow = false, startMs = null }: { steps: RecordStep[]; follow?: boolean; startMs?: number | null } = $props();

  const shown = $derived(visibleSteps(steps));
  const t0 = $derived(startMs ?? shown[0]?.ts_ms ?? 0);
  let list = $state<HTMLOListElement>();

  // Keep the newest step in view while recording.
  $effect(() => {
    void shown.length;
    if (!follow || !list) return;
    void tick().then(() => list && (list.scrollTop = list.scrollHeight));
  });

  function rel(ms: number): string {
    const s = Math.max(0, (ms - t0) / 1000);
    return s < 60 ? `+${s.toFixed(1)}s` : `+${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;
  }
</script>

<ol class="steps" bind:this={list} aria-label={t('record.steps.label')}>
  {#each shown as s (s.seq)}
    {@const v = describeStep(s)}
    <li class="step {v.kind}" class:failed={v.failed} class:pending={s.source === 'api' && s.command && s.command.ok === undefined}>
      <span class="n">{s.seq}</span>
      <span class="what">
        <span class="verb">{v.verb}</span>
        {#if v.target}<span class="target" title={v.target}>{v.target}</span>{/if}
        {#if v.value}<span class="value" class:secret={s.secret} title={v.value}>{v.value}</span>{/if}
      </span>
      <span class="src" title={s.source === 'api' ? t('record.source.api') : s.source === 'human' ? t('record.source.human') : t('record.source.recorder')}>{s.source}</span>
      <span class="t">{rel(s.ts_ms)}</span>
    </li>
  {:else}
    <li class="none">{t('record.steps.none')}</li>
  {/each}
</ol>

<style>
  .steps {
    margin: 0;
    padding: 2px 0;
    list-style: none;
    overflow-y: auto;
    font-size: var(--fs-xs);
  }
  .step {
    display: grid;
    grid-template-columns: 24px minmax(0, 1fr) auto 44px;
    gap: 6px;
    align-items: baseline;
    padding: 3px 8px;
    border-bottom: 1px solid color-mix(in oklab, var(--border) 50%, transparent);
  }
  .step:last-child {
    border-bottom: 0;
  }
  .n,
  .t {
    color: var(--fg-subtle);
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
  .t {
    text-align: right;
  }
  .what {
    display: flex;
    flex-wrap: wrap;
    gap: 0 5px;
    min-width: 0;
    line-height: 1.45;
  }
  .verb {
    color: var(--fg);
    font-weight: 600;
  }
  .target {
    color: var(--fg-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .nav .target,
  .system .target {
    font-family: var(--font-mono);
  }
  .value {
    padding: 0 4px;
    border-radius: 3px;
    background: var(--accent-soft);
    color: var(--accent);
    font-family: var(--font-mono);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 100%;
  }
  .value.secret {
    background: color-mix(in oklab, var(--warn) 16%, transparent);
    color: var(--warn);
  }
  .src {
    color: var(--fg-subtle);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .command .verb {
    font-family: var(--font-mono);
    font-weight: 500;
  }
  .failed .verb,
  .failed .value {
    color: var(--danger);
  }
  .pending {
    opacity: 0.6;
  }
  .none {
    padding: 14px 10px;
    color: var(--fg-subtle);
    text-align: center;
  }
</style>
