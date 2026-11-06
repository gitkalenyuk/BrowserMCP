<script lang="ts">
  import type { Rect } from '../api/types';
  import type { KeyLike } from '../keymap';
  import { useChrome } from '../state/chrome.svelte';
  import { isLibraryUrl } from '../data/library';
  import { fallbackTitle, isNewTab } from '../urls';
  import { SPLIT_GAP, splitLeftWidth, splitRatioAt } from '../state/workspaces';
  import Favicon from './Favicon.svelte';

  // The page area. The chrome never draws web content: the core places the CEF
  // content view over this element's bounds, which we report via `ui.layout`.
  const chrome = useChrome();
  const mock = chrome.api.kind === 'mock';
  const tab = $derived(chrome.active);
  let slot: HTMLDivElement;
  let rect = $state<Rect>({ x: 0, y: 0, w: 0, h: 0 });

  // Split view: the two page views leave a SPLIT_GAP wide gap; this page draws the divider
  // there and drags it (bmcp://cmd/split while dragging, announced when released).
  const split = $derived(tab?.split ?? null);
  let dragRatio = $state<number | null>(null);
  const ratio = $derived(dragRatio ?? split?.ratio ?? 0.5);
  const dividerX = $derived(splitLeftWidth(rect.w, ratio));
  let dragFrame = 0;

  function onDividerDown(e: PointerEvent) {
    if (!split) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    dragRatio = split.ratio;
  }
  function onDividerMove(e: PointerEvent) {
    if (dragRatio === null) return;
    const r = slot.getBoundingClientRect();
    dragRatio = splitRatioAt(e.clientX - r.left, r.width);
    cancelAnimationFrame(dragFrame);
    const value = dragRatio;
    dragFrame = requestAnimationFrame(() => chrome.host?.splitRatio(value, false));
  }
  function onDividerUp() {
    if (dragRatio === null || !split || !tab) return;
    const value = dragRatio;
    dragRatio = null;
    cancelAnimationFrame(dragFrame);
    if (chrome.host) chrome.host.splitRatio(value, true);
    else {
      const [left, right] = split.side === 'left' ? [tab.id, split.other] : [split.other, tab.id];
      void chrome.api.call('tabs.split', { left, right, ratio: value }).catch(() => {});
    }
  }

  $effect(() => {
    let frame = 0;
    let last = '';
    const report = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const r = slot.getBoundingClientRect();
        const next: Rect = { x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) };
        const key = `${next.x},${next.y},${next.w},${next.h},${devicePixelRatio},${window.innerWidth},${window.innerHeight}`;
        if (key === last) return;
        last = key;
        rect = next;
        // Inside the browser the window places the page (it knows which window we are).
        if (chrome.host) chrome.host.layout(next, { w: window.innerWidth, h: window.innerHeight });
        else void chrome.api.call('ui.layout', { ...next, dpr: devicePixelRatio }).catch(() => {});
      });
    };
    const ro = new ResizeObserver(report);
    ro.observe(slot);
    // Position can change without a size change (banner appears, tabs move to the side).
    ro.observe(document.body);
    window.addEventListener('resize', report);
    report();
    return () => {
      ro.disconnect();
      window.removeEventListener('resize', report);
      cancelAnimationFrame(frame);
    };
  });

  // Dev only: the embedded new tab page asks the chrome to navigate.
  $effect(() => {
    if (!mock) return;
    const onMessage = (e: MessageEvent) => {
      if (e.origin !== location.origin) return;
      const data = e.data as { type?: string; url?: string; key?: KeyLike } | null;
      if (data?.type === 'bmcp:navigate' && typeof data.url === 'string') chrome.navigate(data.url);
      // Shortcuts pressed inside the iframe (the real core intercepts them before the page).
      if (data?.type === 'bmcp:key' && data.key) chrome.handleKey(data.key);
    };
    window.addEventListener('message', onMessage);
    return () => window.removeEventListener('message', onMessage);
  });
</script>

<div class="page-slot" class:mock bind:this={slot}>
  {#if split}
    <!-- Over the gap between the two page views (they cover the rest of the slot). -->
    <div
      class="divider"
      class:dragging={dragRatio !== null}
      style:left="{dividerX}px"
      style:width="{SPLIT_GAP}px"
      role="separator"
      aria-orientation="vertical"
      aria-label="Split view divider"
      aria-valuenow={Math.round(ratio * 100)}
      onpointerdown={onDividerDown}
      onpointermove={onDividerMove}
      onpointerup={onDividerUp}
      onpointercancel={onDividerUp}
    ></div>
  {/if}
  {#if mock && tab}
    {#if isNewTab(tab.url)}
      <iframe class="newtab" src="./newtab.html" title="New tab"></iframe>
    {:else if isLibraryUrl(tab.url)}
      <iframe class="newtab" src="./library.html" title="Library"></iframe>
    {:else}
      <div class="placeholder">
        <Favicon url={tab.url} src={tab.favicon} loading={tab.loading} size={28} />
        <div class="title">{tab.title || fallbackTitle(tab.url)}</div>
        <div class="url">{tab.url}</div>
        <p class="note">
          The page renders here in the real browser. Reported bounds:
          <span class="mono">{rect.w}×{rect.h} at {rect.x},{rect.y}</span>
        </p>
      </div>
    {/if}
  {/if}
</div>

<style>
  .page-slot {
    position: relative;
    flex: 1;
    min-width: 0;
    min-height: 0;
    /* Transparent in the real browser: the content view shows through. */
    background: transparent;
  }
  .page-slot.mock {
    background: var(--surface);
  }
  .divider {
    position: absolute;
    top: 0;
    bottom: 0;
    z-index: 2;
    cursor: col-resize;
    background: var(--border);
  }
  .divider:hover,
  .divider.dragging {
    background: var(--accent);
  }
  .newtab {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    border: 0;
  }
  .placeholder {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    padding: 24px;
    text-align: center;
  }
  .title {
    margin-top: 8px;
    font-size: var(--fs-lg);
    font-weight: 600;
    max-width: 60ch;
  }
  .url {
    max-width: 70ch;
    color: var(--fg-muted);
    overflow-wrap: anywhere;
    user-select: text;
  }
  .note {
    margin: 18px 0 0;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
  .mono {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
  }
</style>
