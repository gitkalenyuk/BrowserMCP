<script lang="ts">
  import type { ExportFormat, RowStatus, RunInfo, RunRow } from '../../api/types-record-runs';
  import {
    compact,
    filteredTotal,
    firstPage,
    formatDuration,
    healOf,
    isActive,
    nextPage,
    pageCount,
    pageCursor,
    pageRange,
    prevPage,
    progressSegments,
    type Pager,
    type StatusFilter,
  } from '../../runs/logic';
  import { runsStore } from '../../runs/store.svelte';
  import { untrack } from 'svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import HealProposals from './HealProposals.svelte';

  let { run, onclose }: { run: RunInfo; onclose: () => void } = $props();

  const store = runsStore(useChrome().api);
  const api = store.api;
  const LIMIT = 50;

  let status = $state<StatusFilter>('all');
  let pager = $state<Pager>(firstPage());
  let rows = $state.raw<RunRow[]>([]);
  let next = $state<number | null>(null);
  let loading = $state(false);
  let expanded = $state<number | null>(null);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state(false);

  // export
  let exportOpen = $state(false);
  let exportTo = $state<ExportFormat>('csv');
  let exportStatus = $state<RowStatus | ''>('');
  let webhook = $state('');

  // heal
  let patch = $state<string | null>(null);
  let patchFor = $state<string | null>(null);

  const heal = $derived(healOf(run));
  const total = $derived(filteredTotal(run, status));
  const pages = $derived(pageCount(total, LIMIT));
  const FILTERS: { id: StatusFilter; label: string }[] = [
    { id: 'all', label: 'All' },
    { id: 'ok', label: 'OK' },
    { id: 'failed', label: 'Failed' },
    { id: 'needs_heal', label: 'Needs patch' },
    { id: 'running', label: 'Running' },
    { id: 'pending', label: 'Pending' },
    { id: 'cancelled', label: 'Cancelled' },
  ];

  async function load() {
    loading = true;
    try {
      const r = await api.call('run.results', {
        id: untrack(() => run.id),
        limit: LIMIT,
        cursor: pageCursor(pager) ?? undefined,
        status: status === 'all' ? undefined : status,
      });
      rows = r.rows;
      next = r.next_cursor ?? null;
      error = null;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  // Load on page / filter change.
  $effect(() => {
    void pager;
    void status;
    void load();
  });

  // Live: reload the page when progress arrives (at most once a second).
  let lastTick = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  $effect(() => {
    void store.tick;
    if (!isActive(run) && run.status !== 'finished') return;
    if (timer) return;
    const wait = Math.max(0, 1000 - (Date.now() - lastTick));
    timer = setTimeout(() => {
      timer = null;
      lastTick = Date.now();
      if (!loading) void load();
    }, wait);
    return () => {
      if (timer) clearTimeout(timer);
      timer = null;
    };
  });

  function setStatus(s: StatusFilter) {
    status = s;
    pager = firstPage();
    expanded = null;
  }

  async function control(action: 'pause' | 'resume' | 'cancel') {
    busy = true;
    error = null;
    try {
      await store.control(run.id, action);
      notice = action === 'cancel' ? 'Run cancelled.' : action === 'pause' ? 'Paused: running rows finish, no new ones start.' : 'Resumed.';
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  async function doExport() {
    busy = true;
    error = null;
    notice = null;
    try {
      const r = await api.call('run.export', {
        id: run.id,
        to: exportTo,
        status: exportStatus || undefined,
        url: exportTo === 'webhook' ? webhook.trim() : undefined,
      });
      notice = r.path ? `Exported ${r.rows} rows to ${r.path}` : `Sent ${r.rows} rows in ${r.batches ?? 0} webhook requests`;
      exportOpen = false;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  async function startPatch() {
    if (!heal) return;
    busy = true;
    error = null;
    try {
      const s = await api.call('script.get', { name: heal.script });
      patch = s.code;
      patchFor = heal.script;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  async function applyPatch() {
    if (patch === null) return;
    busy = true;
    error = null;
    try {
      const r = await api.call('run.heal_apply', { id: run.id, patched_code: patch });
      notice = `Continuing with ${r.script}: ${r.requeued} row${r.requeued === 1 ? '' : 's'} back in the queue.`;
      patch = null;
      patchFor = null;
      await store.refreshRun(run.id);
      void store.refreshScripts();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  /** The script lines around the failing one, for the heal box. */
  const context = $derived.by(() => {
    if (!heal?.line || patch === null) return null;
    const lines = patch.split('\n');
    const at = heal.line - 1;
    return lines.slice(Math.max(0, at - 2), at + 3).map((text, i) => ({ n: Math.max(0, at - 2) + i + 1, text, hit: Math.max(0, at - 2) + i === at }));
  });

  const took = $derived(run.finished_ms ? formatDuration(run.finished_ms - run.created_ms) : null);
</script>

<div class="rr-bar">
  <button class="icon-btn" aria-label="Back to runs" title="Back" onclick={onclose}><Icon name="arrow-left" size={15} /></button>
  <h3 class="rr-mono" title={run.id}>{run.script}</h3>
  <span class="rr-pill {run.status}">{run.status}</span>
  {#if run.status === 'running'}
    <button class="icon-btn" title="Pause" aria-label="Pause run" disabled={busy} onclick={() => control('pause')}><Icon name="pause" size={14} /></button>
  {:else if run.status === 'paused'}
    <button class="icon-btn" title={heal ? 'Resume without a patch (waiting rows count as failed)' : 'Resume'} aria-label="Resume run" disabled={busy} onclick={() => control('resume')}><Icon name="play" size={13} /></button>
  {/if}
  {#if isActive(run)}
    <button class="icon-btn" title="Cancel the run" aria-label="Cancel run" disabled={busy} onclick={() => control('cancel')}><Icon name="stop" size={14} /></button>
  {/if}
  <button class="icon-btn" title="Export results" aria-label="Export results" aria-pressed={exportOpen} onclick={() => (exportOpen = !exportOpen)}><Icon name="download" size={15} /></button>
</div>

{#if error || notice}
  <div class="rr-note" class:err={!!error} class:ok={!error} role="status">
    <span>{error ?? notice}</span>
    <button class="icon-btn" aria-label="Dismiss" onclick={() => (error ? (error = null) : (notice = null))}><Icon name="x" size={12} /></button>
  </div>
{/if}

<div class="rr-scroll">
  <div class="rr-section head">
    <div class="bar" aria-label="{run.done} of {run.total} rows done">
      {#each progressSegments(run) as s (s.kind)}<span class="seg {s.kind}" style:width="{s.pct}%"></span>{/each}
    </div>
    <div class="nums">
      <span><b>{run.done}</b> / {run.total} done</span>
      <span class="ok">{run.counts.ok} ok</span>
      {#if run.counts.failed}<span class="bad">{run.counts.failed} failed</span>{/if}
      {#if run.counts.needs_heal}<span class="warn">{run.counts.needs_heal} need a patch</span>{/if}
      {#if run.counts.cancelled}<span>{run.counts.cancelled} cancelled</span>{/if}
    </div>
    <p class="rr-hint">
      {new Date(run.created_ms).toLocaleString()}{took ? ` · took ${took}` : ''} · parallel {run.options.parallel}{run.options.parallel_requested !== run.options.parallel
        ? ` (asked ${run.options.parallel_requested})`
        : ''} · retries {run.options.retries} · timeout {run.options.timeout_secs} s{run.options.stop_on_error ? ' · stops on error' : ''}
    </p>
    {#if run.note && !heal}<p class="rr-hint">{run.note}</p>{/if}
    {#if !heal}<HealProposals {run} paused={false} onedit={() => {}} />{/if}
  </div>

  {#if exportOpen}
    <div class="rr-section rr-form">
      <h4>Export results</h4>
      <div class="rr-row">
        <label class="rr-field grow">
          Format
          <select bind:value={exportTo}>
            <option value="csv">CSV</option>
            <option value="xlsx">Excel (.xlsx)</option>
            <option value="json">JSON</option>
            <option value="sqlite">SQLite</option>
            <option value="webhook">Webhook (POST batches)</option>
          </select>
        </label>
        <label class="rr-field grow">
          Rows
          <select bind:value={exportStatus}>
            <option value="">All</option>
            <option value="ok">OK only</option>
            <option value="failed">Failed only</option>
          </select>
        </label>
      </div>
      {#if exportTo === 'webhook'}
        <label class="rr-field">Webhook URL <input type="url" class="mono" bind:value={webhook} placeholder="https://example.test/hook" spellcheck="false" /></label>
      {:else}
        <p class="rr-hint">Written to <span class="rr-mono">data/runs/{run.id}/results.{exportTo}</span></p>
      {/if}
      <div class="rr-row"><button class="btn primary" disabled={busy || (exportTo === 'webhook' && !webhook.trim())} onclick={doExport}>Export</button></div>
    </div>
  {/if}

  {#if heal}
    <div class="rr-section heal" role="region" aria-label="Patch needed">
      <h4><Icon name="patch" size={13} /> Waiting for a patch</h4>
      <p class="what">
        Row {heal.row + 1} broke{heal.step ? ` in ${heal.step}` : ''}{heal.line ? ` at line ${heal.line}` : ''} of
        <span class="rr-mono">{heal.script}</span>.
      </p>
      {#if heal.error}
        <p class="err-line"><b>{heal.error.code}</b> {heal.error.message}</p>
      {/if}
      <HealProposals {run} paused onedit={(code, script) => ((patch = code), (patchFor = script))} />
      {#if patch === null}
        <div class="rr-row">
          <button class="btn" disabled={busy} onclick={startPatch}><Icon name="code" size={13} /> Write a patch yourself</button>
          <button class="btn" disabled={busy} onclick={() => control('resume')} title="Rows waiting for a patch count as failed">Skip and resume</button>
        </div>
      {:else}
        {#if context}
          <pre class="ctx">{#each context as l (l.n)}<span class:hit={l.hit}><i>{l.n}</i>{l.text}
</span>{/each}</pre>
        {/if}
        <label class="rr-field">
          <span>Fixed script <span class="opt">saved as the next version of {patchFor}</span></span>
          <textarea class="patch" bind:value={patch} rows="14" wrap="off" spellcheck="false"></textarea>
        </label>
        <div class="rr-row">
          <button class="btn primary" disabled={busy || !patch.trim()} onclick={applyPatch}>Apply patch & continue</button>
          <button class="btn" disabled={busy} onclick={() => ((patch = null), (patchFor = null))}>Discard</button>
        </div>
      {/if}
    </div>
  {/if}

  <div class="results">
    <div class="filters rr-chips" role="group" aria-label="Row status">
      {#each FILTERS as f (f.id)}
        {@const n = f.id === 'all' ? run.total : run.counts[f.id]}
        {#if f.id === 'all' || n > 0 || status === f.id}
          <button class="rr-chip" aria-pressed={status === f.id} onclick={() => setStatus(f.id)}>{f.label} <span class="c">{n}</span></button>
        {/if}
      {/each}
    </div>
    <table>
      <thead>
        <tr><th class="r">#</th><th>Status</th><th>Inputs</th><th>Result</th><th class="r">Time</th></tr>
      </thead>
      <tbody>
        {#each rows as row (row.row)}
          <tr class:open={expanded === row.row} class="st-{row.status}" onclick={() => (expanded = expanded === row.row ? null : row.row)}>
            <td class="r rr-mono">{row.row + 1}</td>
            <td><span class="rr-pill {row.status}">{row.status === 'needs_heal' ? 'patch' : row.status}</span>{#if row.attempts > 1}<span class="att" title="attempts">×{row.attempts}</span>{/if}</td>
            <td class="cell rr-mono" title={compact(row.inputs, 2000)}>{compact(row.inputs, 80)}</td>
            <td class="cell rr-mono" class:bad={!!row.error} title={row.error ? row.error.message : compact(row.result, 2000)}>
              {row.error ? `${row.error.code}: ${row.error.message}` : compact(row.result, 80)}
            </td>
            <td class="r rr-mono">{formatDuration(row.duration_ms)}</td>
          </tr>
          {#if expanded === row.row}
            <tr class="more">
              <td colspan="5">
                {#if row.error}
                  <p class="err-line">
                    <b>{row.error.code}</b>
                    {#if row.error.command}<span class="rr-mono">{row.error.command}</span>{/if}
                    {row.error.message}
                    {#if row.error.line}<span class="rr-hint">line {row.error.line}</span>{/if}
                  </p>
                {/if}
                <div class="kv"><span>Inputs</span><pre>{JSON.stringify(row.inputs, null, 2)}</pre></div>
                {#if row.result !== null && row.result !== undefined}<div class="kv"><span>Result</span><pre>{JSON.stringify(row.result, null, 2)}</pre></div>{/if}
                {#if row.stdout_tail}<div class="kv"><span>Output</span><pre>{row.stdout_tail}</pre></div>{/if}
                {#if row.script}<p class="rr-hint">Script {row.script} · {row.attempts} attempt{row.attempts === 1 ? '' : 's'}</p>{/if}
              </td>
            </tr>
          {/if}
        {:else}
          <tr><td colspan="5" class="none">{loading ? 'Loading…' : 'No rows.'}</td></tr>
        {/each}
      </tbody>
    </table>
    <div class="pager">
      <button class="icon-btn" aria-label="Previous page" disabled={!prevPage(pager)} onclick={() => (pager = prevPage(pager) ?? pager)}><Icon name="arrow-left" size={13} /></button>
      <span>{pageRange(pager, LIMIT, rows.length, total)} · page {pager.index + 1} of {pages}</span>
      <button class="icon-btn" aria-label="Next page" disabled={next === null} onclick={() => (pager = nextPage(pager, next) ?? pager)}><Icon name="arrow-right" size={13} /></button>
    </div>
  </div>
</div>

<style>
  .head {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .bar {
    display: flex;
    height: 7px;
    overflow: hidden;
    border-radius: 4px;
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
  .nums {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .ok {
    color: var(--ok);
  }
  .bad {
    color: var(--danger);
  }
  .warn {
    color: var(--warn);
  }
  .heal {
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: color-mix(in oklab, var(--warn) 7%, var(--toolbar));
  }
  .heal h4 {
    margin: 0;
    color: var(--warn);
  }
  .what {
    margin: 0;
    line-height: 1.45;
  }
  .err-line {
    margin: 0;
    color: var(--danger);
    font-size: var(--fs-xs);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }
  .ctx {
    margin: 0;
    padding: 4px 0;
    overflow-x: auto;
    border-radius: var(--r-sm);
    background: var(--sunken);
    font: 11px / 1.5 var(--font-mono);
  }
  .ctx span {
    display: block;
    padding: 0 8px;
    white-space: pre;
  }
  .ctx span.hit {
    background: color-mix(in oklab, var(--danger) 16%, transparent);
  }
  .ctx i {
    display: inline-block;
    width: 3ch;
    margin-right: 8px;
    color: var(--fg-subtle);
    font-style: normal;
    text-align: right;
  }
  .patch {
    min-height: 180px;
  }
  .results {
    padding: 8px 0 0;
  }
  .filters {
    padding: 0 8px 6px;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    table-layout: fixed;
    font-size: var(--fs-xs);
  }
  th {
    padding: 4px 6px;
    border-block: 1px solid var(--border);
    color: var(--fg-subtle);
    font-weight: 600;
    text-align: left;
  }
  th:nth-child(1) {
    width: 34px;
  }
  th:nth-child(2) {
    width: 76px;
  }
  th:nth-child(5) {
    width: 50px;
  }
  td {
    padding: 4px 6px;
    border-bottom: 1px solid color-mix(in oklab, var(--border) 50%, transparent);
    vertical-align: top;
  }
  tbody tr:not(.more) {
    cursor: pointer;
  }
  tbody tr:not(.more):hover {
    background: var(--hover);
  }
  tr.open {
    background: var(--accent-soft);
  }
  .r {
    text-align: right;
  }
  .cell {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .att {
    margin-left: 3px;
    color: var(--fg-subtle);
  }
  .more td {
    background: var(--surface);
  }
  .kv {
    display: grid;
    grid-template-columns: 52px minmax(0, 1fr);
    gap: 6px;
    margin-top: 6px;
  }
  .kv span {
    color: var(--fg-subtle);
  }
  .kv pre {
    max-height: 140px;
    margin: 0;
    overflow: auto;
    font: 11px / 1.45 var(--font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    user-select: text;
  }
  .none {
    padding: 14px;
    color: var(--fg-subtle);
    text-align: center;
  }
  .pager {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 6px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .pager .icon-btn {
    width: 26px;
    height: 26px;
  }
</style>
