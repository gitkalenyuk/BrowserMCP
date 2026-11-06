<script lang="ts">
  import './record-runs.css';
  import { recordStore } from '../../record/store.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import { hostOf } from '../../urls';
  import Icon from '../Icon.svelte';
  import RecordingDetail from './RecordingDetail.svelte';
  import StepList from './StepList.svelte';

  const chrome = useChrome();
  const rec = recordStore(chrome.api);

  let name = $state('');
  let video = $state(false);
  let traffic = $state(false);
  let showOptions = $state(false);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let openId = $state<string | null>(null);
  let now = $state(Date.now());

  const tab = $derived(chrome.tabs.activeId);
  const activeTab = $derived(chrome.active);
  const active = $derived(rec.active);
  const past = $derived(rec.recordings.filter((r) => !r.active));

  $effect(() => {
    void rec.refresh();
  });

  // Elapsed time while recording.
  $effect(() => {
    if (!active) return;
    const t = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(t);
  });

  function elapsed(from: number): string {
    const s = Math.max(0, Math.floor((now - from) / 1000));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  }

  async function toggle() {
    busy = true;
    error = null;
    notice = null;
    try {
      if (active) {
        const id = await rec.stopRecording();
        notice = 'Recording saved.';
        if (id) openId = id;
      } else {
        if (tab === null) throw new Error('No tab to record');
        await rec.start({ tab, name: name.trim(), video, traffic });
        now = Date.now();
      }
    } catch (err) {
      error = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  function when(ms: number): string {
    const d = new Date(ms);
    const today = new Date();
    const time = d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
    if (d.toDateString() === today.toDateString()) return time;
    return `${d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })} ${time}`;
  }
</script>

<div class="rr">
  {#if openId}
    {#key openId}
      <RecordingDetail id={openId} onclose={() => (openId = null)} />
    {/key}
  {:else}
    <div class="rr-bar">
      <button
        class="btn rec-btn"
        class:on={!!active}
        disabled={busy || (!active && tab === null)}
        aria-pressed={!!active}
        title={active ? 'Stop and save the recording' : 'Record what happens in the current tab'}
        onclick={toggle}
      >
        <span class="dot" aria-hidden="true"></span>
        {active ? 'Stop' : 'Record'}
      </button>
      {#if active}
        <span class="rr-pill rec">REC {elapsed(active.startedMs)}</span>
        <span class="count">{active.steps.length} steps</span>
      {:else}
        <input class="rr-input name" bind:value={name} placeholder="Name (e.g. login)" aria-label="Recording name" spellcheck="false" />
      {/if}
      <span class="rr-spacer"></span>
      {#if !active}
        <button class="icon-btn" aria-label="Recording options" title="Video and traffic options" aria-pressed={showOptions} onclick={() => (showOptions = !showOptions)}>
          <Icon name="settings" size={15} />
        </button>
      {/if}
    </div>

    {#if showOptions && !active}
      <div class="rr-section options">
        <div class="rr-row">
          <label class="rr-check"><input type="checkbox" bind:checked={video} /> Video</label>
          <label class="rr-check"><input type="checkbox" bind:checked={traffic} /> Network traffic</label>
        </div>
        <p class="rr-hint">Video needs ffmpeg (winget install Gyan.FFmpeg). Traffic goes to traffic.jsonl in the recording folder.</p>
      </div>
    {/if}

    {#if error || rec.error || notice}
      <div class="rr-note" class:err={!!(error || rec.error)} role="status">
        <span>{error ?? rec.error ?? notice}</span>
        <button class="icon-btn" aria-label="Dismiss" onclick={() => (error ? (error = null) : rec.error ? (rec.error = null) : (notice = null))}><Icon name="x" size={12} /></button>
      </div>
    {/if}

    {#if active}
      <div class="live">
        <div class="live-head">
          <span class="rr-h">Recording “{active.name}”</span>
          <span class="where" title={active.startUrl}>tab {active.tab}{activeTab && activeTab.id === active.tab ? ` · ${activeTab.title}` : ''}</span>
        </div>
        <StepList steps={active.steps} follow startMs={active.startedMs} />
        {#if active.video}<p class="rr-hint vid">Video: <span class="rr-mono">{active.video}</span></p>{/if}
      </div>
    {/if}

    <div class="rr-scroll" class:shrink={!!active}>
      <div class="list-head">
        <span class="rr-h">Recordings</span>
        <span class="rr-spacer"></span>
        <button class="icon-btn sm" aria-label="Refresh recordings" title="Refresh" onclick={() => rec.refresh()}><Icon name="reload" size={13} /></button>
      </div>
      {#if past.length === 0}
        <div class="rr-empty">
          <Icon name="record" size={20} />
          <p>Record clicks, typing and navigation in this tab (optionally with video). A recording turns into a TypeScript script you can replay with other data.</p>
        </div>
      {:else}
        <ul class="recs">
          {#each past as r (r.id)}
            <li>
              <button class="rec" onclick={() => (openId = r.id)} title={r.id}>
                <span class="nm">{r.name}</span>
                <span class="meta">{when(r.started_ms)} · {r.steps} steps · {hostOf(r.start_url) || r.start_url}</span>
                <Icon name="chevron-right" size={14} />
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
</div>

<style>
  .rec-btn {
    gap: 7px;
    padding: 0 11px;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--danger);
  }
  .rec-btn.on {
    border-color: color-mix(in oklab, var(--danger) 45%, transparent);
    color: var(--danger);
  }
  .rec-btn.on .dot {
    border-radius: 2px;
  }
  .name {
    flex: 1;
    max-width: 220px;
  }
  .count {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }
  .options {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .live {
    display: flex;
    flex-direction: column;
    flex: 1 1 60%;
    min-height: 120px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .live-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 8px 10px 4px;
  }
  .live-head .rr-h {
    margin: 0;
  }
  .where {
    min-width: 0;
    overflow: hidden;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .live :global(.steps) {
    flex: 1;
    min-height: 0;
  }
  .vid {
    padding: 4px 10px 6px;
  }
  .rr-scroll.shrink {
    flex: 0 1 40%;
  }
  .list-head {
    display: flex;
    align-items: center;
    padding: 8px 4px 0 10px;
  }
  .list-head .rr-h {
    margin: 0;
  }
  .icon-btn.sm {
    width: 24px;
    height: 24px;
  }
  .recs {
    margin: 0;
    padding: 4px 0;
    list-style: none;
  }
  .rec {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    grid-template-rows: auto auto;
    column-gap: 8px;
    align-items: center;
    width: 100%;
    padding: 6px 8px 6px 10px;
    border: 0;
    background: transparent;
    color: var(--fg);
    text-align: left;
  }
  .rec:hover {
    background: var(--hover);
  }
  .rec :global(svg) {
    grid-row: 1 / 3;
    grid-column: 2;
    color: var(--fg-subtle);
  }
  .nm {
    overflow: hidden;
    font-weight: 600;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    overflow: hidden;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
