<script lang="ts">
  import './record-runs.css';
  import type { RunInfo } from '../../api/types-record-runs';
  import { formatDuration, healOf, isActive, progressSegments } from '../../runs/logic';
  import { runsStore } from '../../runs/store.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import NewRunForm from './NewRunForm.svelte';
  import RunDetail from './RunDetail.svelte';
  import TriggersView from './TriggersView.svelte';

  const chrome = useChrome();
  const store = runsStore(chrome.api);

  let view = $state<'runs' | 'triggers'>('runs');
  let creating = $state(false);
  let openId = $state<string | null>(null);

  const open = $derived(openId ? (store.runs.find((r) => r.id === openId) ?? null) : null);
  const waiting = $derived(store.runs.filter((r) => healOf(r)).length);

  $effect(() => {
    void store.refresh();
  });

  function when(r: RunInfo): string {
    const d = new Date(r.created_ms);
    const same = d.toDateString() === new Date().toDateString();
    const t = d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
    return same ? t : `${d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })} ${t}`;
  }

  function took(r: RunInfo): string {
    return r.finished_ms ? formatDuration(r.finished_ms - r.created_ms) : '';
  }
</script>

<div class="rr">
  {#if open}
    {#key open.id}
      <RunDetail run={open} onclose={() => (openId = null)} />
    {/key}
  {:else}
    <div class="rr-views" role="tablist" aria-label="Runs views">
      <button role="tab" aria-selected={view === 'runs'} onclick={() => (view = 'runs')}>
        Runs
        {#if waiting > 0}<span class="badge" title="Waiting for a patch">{waiting}</span>{/if}
      </button>
      <button role="tab" aria-selected={view === 'triggers'} onclick={() => (view = 'triggers')}>
        Triggers
        {#if store.triggers.length}<span class="n">{store.triggers.length}</span>{/if}
      </button>
    </div>

    {#if store.error || store.notice}
      <div class="rr-note" class:err={!!store.error} role="status">
        <span>{store.error ?? store.notice}</span>
        <button class="icon-btn" aria-label="Dismiss" onclick={() => (store.error ? (store.error = null) : (store.notice = null))}><Icon name="x" size={12} /></button>
      </div>
    {/if}

    {#if view === 'runs'}
      <div class="rr-bar">
        <button class="btn" class:primary={!creating} aria-expanded={creating} onclick={() => (creating = !creating)}>
          <Icon name={creating ? 'x' : 'plus'} size={13} />
          {creating ? 'Close' : 'New run'}
        </button>
        <span class="rr-spacer"></span>
        <span class="count">{store.runs.length} runs</span>
        <button class="icon-btn" aria-label="Refresh runs" title="Refresh" onclick={() => store.refresh()}><Icon name="reload" size={14} /></button>
      </div>
      <div class="rr-scroll">
        {#if creating}
          <NewRunForm
            onstarted={(id) => {
              creating = false;
              openId = id;
            }}
          />
        {/if}
        {#if store.runs.length === 0 && !creating}
          <div class="rr-empty">
            <Icon name="runs" size={20} />
            <p>Run a saved script once per row of a CSV, JSON or Excel file, several at a time, with retries. Progress, failures and patches waiting for you show up here.</p>
            <button class="btn primary" onclick={() => (creating = true)}>New run</button>
          </div>
        {:else}
          <ul class="runs">
            {#each store.runs as r (r.id)}
              {@const heal = healOf(r)}
              <li>
                <button class="run" onclick={() => (openId = r.id)} title={r.id}>
                  <span class="top">
                    <span class="script">{r.script}</span>
                    <span class="rr-pill {r.status}">{heal ? 'needs patch' : r.status}</span>
                  </span>
                  <span class="bar" aria-hidden="true">
                    {#each progressSegments(r) as s (s.kind)}<span class="seg {s.kind}" style:width="{s.pct}%"></span>{/each}
                  </span>
                  <span class="meta">
                    <span>{r.done}/{r.total}</span>
                    {#if r.counts.failed}<span class="bad">{r.counts.failed} failed</span>{/if}
                    {#if r.counts.needs_heal}<span class="warn">{r.counts.needs_heal} need a patch</span>{/if}
                    {#if isActive(r) && r.counts.running}<span>{r.counts.running} running</span>{/if}
                    <span class="rr-spacer"></span>
                    <span>{when(r)}{took(r) ? ` · ${took(r)}` : ''}</span>
                  </span>
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {:else}
      <TriggersView />
    {/if}
  {/if}
</div>

<style>
  .badge {
    min-width: 16px;
    padding: 0 5px;
    border-radius: 8px;
    background: var(--warn);
    color: var(--surface);
    font-size: 10px;
    line-height: 16px;
    text-align: center;
  }
  .n {
    color: var(--fg-subtle);
    font-weight: 500;
  }
  .count {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .runs {
    margin: 0;
    padding: 4px 0;
    list-style: none;
  }
  .run {
    display: flex;
    flex-direction: column;
    gap: 5px;
    width: 100%;
    padding: 8px 10px;
    border: 0;
    border-bottom: 1px solid color-mix(in oklab, var(--border) 60%, transparent);
    background: transparent;
    color: var(--fg);
    text-align: left;
  }
  .run:hover {
    background: var(--hover);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .script {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .bar {
    display: flex;
    height: 5px;
    overflow: hidden;
    border-radius: 3px;
    background: var(--sunken);
  }
  .seg {
    height: 100%;
    transition: width 300ms var(--ease-out);
  }
  .seg.ok {
    background: var(--ok);
  }
  .seg.failed {
    background: var(--danger);
  }
  .seg.needs_heal {
    background: var(--warn);
  }
  .seg.cancelled {
    background: var(--fg-subtle);
  }
  .seg.running {
    background: color-mix(in oklab, var(--accent) 55%, transparent);
  }
  .meta {
    display: flex;
    gap: 8px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .bad {
    color: var(--danger);
  }
  .warn {
    color: var(--warn);
  }
</style>
