<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { Download } from '../lib/api/types-data';
  import Icon from '../lib/components/Icon.svelte';
  import { fileName, formatBytes, stateLabel } from '../lib/data/format';
  import { onReconnect } from '../lib/data/library';
  import { isSafeLinkUrl } from '../lib/safeurl';
  import { hostOf } from '../lib/urls';

  let { api, notify, fail }: { api: BrowserApi; notify: (text: string) => void; fail: (err: unknown) => void } = $props();

  let downloads = $state<Download[]>([]);
  let loaded = $state(false);
  let query = $state('');
  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return q ? downloads.filter((d) => d.path.toLowerCase().includes(q) || d.url.toLowerCase().includes(q)) : downloads;
  });

  async function refresh() {
    try {
      downloads = (await api.call('downloads.list', { limit: 500 })).downloads;
      loaded = true;
    } catch (err) {
      fail(err);
    }
  }
  $effect(() => {
    void refresh();
    // Poll while something is still downloading.
    const t = setInterval(() => downloads.some((d) => d.state === 'in_progress') && void refresh(), 1500);
    const off = onReconnect(api, () => void refresh());
    return () => {
      clearInterval(t);
      off();
    };
  });

  function showInFolder(d: Download) {
    // Stub: the core has no "reveal in Explorer" command yet.
    notify(`Saved to ${d.path}`);
  }
</script>

<header class="head">
  <h2>Downloads</h2>
  <label class="lib-search">
    <Icon name="search" size={14} />
    <input class="lib-input" type="search" placeholder="Search downloads" aria-label="Search downloads" bind:value={query} />
  </label>
</header>

<div class="list">
  {#each shown as d (d.id)}
    {@const pct = d.total_bytes ? Math.min(100, (d.received_bytes / d.total_bytes) * 100) : null}
    <div class="lib-row item">
      <span class="ficon"><Icon name="download" size={18} /></span>
      <div class="grow">
        <span class="title" title={d.path}>{fileName(d.path)}</span>
        <span class="sub">
          {#if isSafeLinkUrl(d.url)}<a href={d.url} title={d.url}>{hostOf(d.url)}</a>{:else}{d.url}{/if}
          · {d.state === 'in_progress'
            ? `${formatBytes(d.received_bytes)} of ${d.total_bytes ? formatBytes(d.total_bytes) : '?'}`
            : formatBytes(d.total_bytes ?? d.received_bytes)}
          · {new Date(d.started).toLocaleString('en-US', { dateStyle: 'medium', timeStyle: 'short' })}
        </span>
        {#if d.state === 'in_progress'}
          <span class="bar" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={pct ?? undefined}>
            <span style:width="{pct ?? 30}%" class:indeterminate={pct === null}></span>
          </span>
        {/if}
      </div>
      <span class="lib-chip" class:ok={d.state === 'complete'} class:bad={d.state === 'cancelled' || d.state === 'interrupted'}>
        {stateLabel(d.state)}
      </span>
      <button class="btn" onclick={() => showInFolder(d)} title="Opening the folder is not available yet; shows the path">
        <Icon name="folder" size={14} /> Show in folder
      </button>
    </div>
  {:else}
    {#if loaded}
      <div class="lib-empty">
        <h3>{query.trim() ? 'No downloads match' : 'No downloads yet'}</h3>
        <p>Files you download appear here.</p>
      </div>
    {/if}
  {/each}
</div>

<style>
  .head {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 24px 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0 8px 0 0;
    font-size: 18px;
    font-weight: 650;
  }
  .list {
    max-width: 920px;
    padding: 12px 18px 48px;
  }
  .item {
    min-height: 56px;
  }
  .ficon {
    display: inline-grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: var(--r);
    background: var(--sunken);
    color: var(--fg-muted);
    flex: none;
  }
  .sub a {
    color: inherit;
  }
  .bar {
    position: relative;
    display: block;
    height: 4px;
    margin-top: 5px;
    overflow: hidden;
    border-radius: 2px;
    background: var(--sunken);
  }
  .bar span {
    position: absolute;
    inset: 0 auto 0 0;
    border-radius: 2px;
    background: var(--accent);
  }
  .bar span.indeterminate {
    animation: slide 1.2s var(--ease-out) infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(350%);
    }
  }
</style>
