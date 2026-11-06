<script lang="ts">
  import { tick } from 'svelte';
  import { RpcError } from '../../api';
  import { filterRows, TYPE_CHIPS, toggleChip, type TypeChip } from '../../net/logic';
  import { mergeRows, nextPollCursor, parseNetList, type NetRow } from '../../net/parse';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';
  import NetBreakpoints from './NetBreakpoints.svelte';
  import NetDetails from './NetDetails.svelte';
  import NetRules from './NetRules.svelte';

  const chrome = useChrome();
  const net = chrome.net;

  const ROW_H = 22;
  const POLL_MS = 1000;

  let view = $state<'requests' | 'rules' | 'breakpoints'>('requests');
  // Up to 5000 rows: raw state, replaced as a whole on every poll.
  let rows = $state.raw<NetRow[]>([]);
  let captured = $state(0);
  let dropped = $state(0);
  let listError = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state(false);
  let query = $state('');
  let chips = $state<TypeChip[]>(['all']);
  let selectedId = $state<number | null>(null);
  let clearedBefore = $state(0);
  let scrollTop = $state(0);
  let viewportH = $state(300);
  let list = $state<HTMLDivElement>();
  let stick = true;

  let tabFor: number | null = null;
  let cursor = 0;
  let inFlight = false;

  const tab = $derived(chrome.tabs.activeId);
  const capturing = $derived(tab !== null && net.capturing[tab] === true);
  const visible = $derived(filterRows(clearedBefore ? rows.filter((r) => r.id >= clearedBefore) : rows, { text: query, chips }));
  const first = $derived(Math.max(0, Math.floor(scrollTop / ROW_H) - 8));
  const slice = $derived(visible.slice(first, first + Math.ceil(viewportH / ROW_H) + 16));
  const selected = $derived(rows.find((r) => r.id === selectedId) ?? null);
  const pausedCount = $derived(net.paused.length);

  function reset(next: number | null) {
    tabFor = next;
    rows = [];
    cursor = 0;
    captured = 0;
    dropped = 0;
    selectedId = null;
    clearedBefore = 0;
    listError = null;
  }

  async function poll() {
    const t = tab;
    if (t === null || inFlight) return;
    if (t !== tabFor) reset(t);
    inFlight = true;
    try {
      let c = cursor;
      let end = cursor;
      for (let page = 0; page < 10; page++) {
        const r = await chrome.api.call('net.list', { tab: t, cursor: c, limit: 500 });
        if (tabFor !== t) return;
        const parsed = parseNetList(r.text);
        if (parsed.header) {
          net.capturing[t] = parsed.header.capturing;
          captured = parsed.header.captured;
          dropped = parsed.header.dropped;
        }
        rows = mergeRows(rows, parsed.rows);
        if (parsed.rows.length && notice?.startsWith('Capturing')) notice = null;
        end = r.next_cursor;
        if (!r.more) break;
        c = r.next_cursor;
      }
      cursor = nextPollCursor(rows, end);
      listError = null;
    } catch (err) {
      if (err instanceof RpcError && err.code === 'NOT_FOUND') {
        // Nothing captured on this tab (yet).
        net.capturing[t] = false;
        listError = null;
      } else {
        listError = (err as Error).message;
      }
    } finally {
      inFlight = false;
    }
  }

  // Poll while the panel is open; right away when the active tab changes.
  $effect(() => {
    void tab;
    void poll();
    const timer = setInterval(() => {
      if (view === 'requests' || capturing) void poll();
    }, POLL_MS);
    return () => clearInterval(timer);
  });

  $effect(() => {
    void net.refreshPaused().catch(() => {});
  });

  // Follow new requests while scrolled to the bottom.
  $effect(() => {
    void visible.length;
    // Not while a request is open: the list would move under the pointer.
    if (!stick || !list || selectedId !== null) return;
    void tick().then(() => list && (list.scrollTop = list.scrollHeight));
  });

  // Jump to the breakpoints tab when something gets paused.
  let lastPaused = 0;
  $effect(() => {
    if (pausedCount > lastPaused && view !== 'breakpoints') view = 'breakpoints';
    lastPaused = pausedCount;
  });

  function onScroll() {
    if (!list) return;
    scrollTop = list.scrollTop;
    stick = list.scrollHeight - list.scrollTop - list.clientHeight < ROW_H * 2;
  }

  async function toggleCapture() {
    if (tab === null) return;
    busy = true;
    notice = null;
    try {
      await net.setCapture(tab, !capturing);
      if (net.capturing[tab] && rows.length === 0) notice = 'Capturing. Reload the page to see its first requests.';
      await poll();
    } catch (err) {
      listError = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  async function exportHar() {
    if (tab === null) return;
    busy = true;
    notice = null;
    try {
      const stamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
      const r = await chrome.api.call('net.har_export', { tab, path: `tab${tab}-${stamp}.har` });
      notice = `HAR: ${r.entries} requests (${Math.round(r.bytes / 1024)} kB) saved to ${r.path ?? 'memory'}`;
    } catch (err) {
      listError = (err as Error).message;
    } finally {
      busy = false;
    }
  }

  function clearView() {
    clearedBefore = cursor || (rows[rows.length - 1]?.id ?? 0) + 1;
    selectedId = null;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    const i = visible.findIndex((r) => r.id === selectedId);
    const next = visible[Math.max(0, Math.min(visible.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
    if (!next) return;
    selectedId = next.id;
    const top = visible.indexOf(next) * ROW_H;
    if (list && (top < list.scrollTop || top > list.scrollTop + list.clientHeight - ROW_H)) list.scrollTop = top - list.clientHeight / 2;
  }

  function statusClass(r: NetRow): string {
    if (r.failed) return 'bad';
    if (r.pending || r.status === null) return 'pending';
    if (r.status >= 400) return 'bad';
    if (r.status >= 300) return 'redir';
    return 'ok';
  }

  /** What a narrow column should show first: the path (the host is in the tooltip). */
  function shortUrl(url: string): string {
    const m = /^[a-z]+:\/\/([^/?#]+)(.*)$/i.exec(url);
    if (!m) return url;
    return m[2] && m[2] !== '/' ? m[2] : m[1];
  }
</script>

<div class="traffic">
  <div class="bar">
    <button
      class="btn cap"
      class:on={capturing}
      disabled={tab === null || busy}
      aria-pressed={capturing}
      title={capturing ? 'Stop recording this tab' : 'Record the requests of the current tab'}
      onclick={toggleCapture}
    >
      <span class="rec" aria-hidden="true"></span>
      {capturing ? 'Capturing' : 'Capture'}
    </button>
    <span class="count" title="{captured} in the core's buffer{dropped ? `, ${dropped} oldest dropped` : ''}">
      {visible.length}{visible.length !== rows.length ? ` / ${rows.length}` : ''} requests
    </span>
    <span class="spacer"></span>
    <button class="icon-btn" title="Clear the list (the core keeps them)" aria-label="Clear list" disabled={rows.length === 0} onclick={clearView}>
      <Icon name="trash" size={14} />
    </button>
    <button class="icon-btn" title="Export HAR" aria-label="Export HAR" disabled={tab === null || busy || rows.length === 0} onclick={exportHar}>
      <Icon name="download" size={15} />
    </button>
  </div>

  <div class="views" role="tablist" aria-label="Traffic views">
    <button role="tab" aria-selected={view === 'requests'} onclick={() => (view = 'requests')}>Requests</button>
    <button role="tab" aria-selected={view === 'rules'} onclick={() => (view = 'rules')}>Rules</button>
    <button role="tab" aria-selected={view === 'breakpoints'} onclick={() => (view = 'breakpoints')}>
      Breakpoints
      {#if pausedCount > 0}<span class="badge">{pausedCount}</span>{/if}
    </button>
  </div>

  {#if notice || listError || net.warning}
    <div class="note" class:err={!!listError} role="status">
      <span>{listError ?? net.warning ?? notice}</span>
      <button
        class="icon-btn"
        aria-label="Dismiss"
        onclick={() => {
          if (listError) listError = null;
          else if (net.warning) net.warning = null;
          else notice = null;
        }}><Icon name="x" size={12} /></button
      >
    </div>
  {/if}

  {#if view === 'requests'}
    <div class="filters">
      <label class="search">
        <Icon name="search" size={13} />
        <span class="sr-only">Filter requests</span>
        <input bind:value={query} placeholder="Filter: text, method:post, status:4xx, -word" spellcheck="false" />
      </label>
      <div class="chips" role="group" aria-label="Request types">
        {#each TYPE_CHIPS as c (c.id)}
          <button
            class="chip"
            aria-pressed={chips.includes(c.id)}
            title="{c.label}{c.id === 'all' ? '' : ' (Ctrl+click to combine)'}"
            onclick={(e) => (chips = toggleChip(chips, c.id, e.ctrlKey || e.metaKey))}>{c.label}</button
          >
        {/each}
      </div>
    </div>

    <div class="cols" aria-hidden="true">
      <span>Status</span><span>Method</span><span>URL</span><span>Type</span><span class="r">Size</span><span class="r">Time</span>
    </div>
    <div
      class="list"
      class:with-details={selected !== null}
      bind:this={list}
      bind:clientHeight={viewportH}
      onscroll={onScroll}
      onkeydown={onKey}
      tabindex="0"
      role="listbox"
      aria-label="Captured requests"
    >
      {#if visible.length === 0}
        <div class="empty">
          {#if tab === null}
            No tab is open.
          {:else if !capturing && rows.length === 0}
            <Icon name="traffic" size={20} />
            <p>Record this tab's requests, response bodies and WebSocket frames.</p>
            <button class="btn primary" disabled={busy} onclick={toggleCapture}>Start capture</button>
          {:else if rows.length === 0}
            Waiting for requests… reload the page or use it.
          {:else}
            No request matches the filter.
          {/if}
        </div>
      {:else}
        <div class="spacer-rows" style:height="{visible.length * ROW_H}px">
          {#each slice as r, i (r.id)}
            <div
              class="row"
              class:sel={r.id === selectedId}
              class:failed={r.failed}
              class:odd={(first + i) % 2 === 1}
              style:top="{(first + i) * ROW_H}px"
              role="option"
              aria-selected={r.id === selectedId}
              tabindex="-1"
              title={r.url + (r.error ? `\n${r.error}` : '') + (r.notes ? `\n[${r.notes}]` : '')}
              onclick={() => (selectedId = selectedId === r.id ? null : r.id)}
              onkeydown={() => {}}
            >
              <span class="st {statusClass(r)}">{r.failed ? 'ERR' : r.pending ? '…' : r.status}</span>
              <span class="m">{r.method}</span>
              <span class="u">{shortUrl(r.url)}{#if r.notes}<span class="tag">{r.notes}</span>{/if}</span>
              <span class="t">{r.type}</span>
              <span class="r">{r.size}</span>
              <span class="r">{r.time}</span>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    {#if selected && tab !== null}
      {#key selected.id}
        <NetDetails {tab} row={selected} onclose={() => (selectedId = null)} />
      {/key}
    {/if}
  {:else if view === 'rules'}
    <NetRules {tab} />
  {:else}
    <NetBreakpoints {tab} />
  {/if}
</div>

<style>
  .traffic {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    font-size: var(--fs-sm);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 40px;
    padding: 5px 6px 5px 8px;
    border-bottom: 1px solid var(--border);
  }
  .cap {
    gap: 7px;
    padding: 0 10px;
  }
  .rec {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    border: 1.5px solid var(--fg-muted);
  }
  .cap.on {
    border-color: color-mix(in oklab, var(--danger) 45%, transparent);
    color: var(--danger);
  }
  .cap.on .rec {
    border-color: var(--danger);
    background: var(--danger);
    animation: pulse 1.6s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.4;
    }
  }
  .count {
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
  }
  .spacer {
    flex: 1;
  }
  .views {
    display: flex;
    gap: 2px;
    padding: 0 8px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .views button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 30px;
    padding: 0 9px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .views button:hover {
    color: var(--fg);
  }
  .views button[aria-selected='true'] {
    border-bottom-color: var(--accent);
    color: var(--fg);
  }
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
  .note {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 4px 3px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--sunken);
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    overflow-wrap: anywhere;
  }
  .note.err {
    color: var(--danger);
  }
  .note span {
    flex: 1;
  }
  .note .icon-btn {
    width: 22px;
    height: 22px;
    color: inherit;
  }
  .filters {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 7px 8px;
    border-bottom: 1px solid var(--border);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
    padding: 0 8px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--field-focus);
    color: var(--fg-subtle);
  }
  .search:focus-within {
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .search input {
    flex: 1;
    min-width: 0;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--fg);
    font-size: var(--fs-sm);
    user-select: text;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 3px;
  }
  .chip {
    height: 20px;
    padding: 0 7px;
    border: 0;
    border-radius: 10px;
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .chip:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .chip[aria-pressed='true'] {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .cols,
  .row {
    display: grid;
    grid-template-columns: 34px 40px minmax(0, 1fr) 54px 44px 42px;
    gap: 6px;
    align-items: center;
    padding: 0 8px;
  }
  .cols {
    height: 22px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    font-weight: 600;
    flex: none;
  }
  .r {
    text-align: right;
  }
  .list {
    position: relative;
    flex: 1;
    min-height: 60px;
    overflow-y: auto;
    outline: none;
    background: var(--surface);
  }
  .list.with-details {
    flex: 1 1 40%;
  }
  .spacer-rows {
    position: relative;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: 22px;
    font: var(--fs-xs) / 22px var(--font-mono);
    white-space: nowrap;
    color: var(--fg);
  }
  .row.odd {
    background: color-mix(in oklab, var(--sunken) 45%, transparent);
  }
  .row:hover {
    background: var(--hover);
  }
  .row.sel {
    background: var(--accent-soft);
  }
  .row > span {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .row.failed .u {
    color: var(--danger);
  }
  .st.ok {
    color: var(--ok);
  }
  .st.redir {
    color: var(--fg-muted);
  }
  .st.bad {
    color: var(--danger);
    font-weight: 700;
  }
  .st.pending {
    color: var(--fg-subtle);
  }
  .m {
    color: var(--fg-muted);
  }
  .t {
    color: var(--fg-subtle);
  }
  .tag {
    margin-left: 6px;
    padding: 0 4px;
    border-radius: 3px;
    background: color-mix(in oklab, var(--warn) 18%, transparent);
    color: var(--warn);
    font-family: var(--font);
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    height: 100%;
    padding: 24px;
    color: var(--fg-muted);
    text-align: center;
  }
  .empty p {
    max-width: 30ch;
    margin: 0;
    line-height: 1.5;
  }
</style>
