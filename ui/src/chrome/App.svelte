<script lang="ts">
  import AgentBanner from '../lib/components/AgentBanner.svelte';
  import BookmarksBar from '../lib/components/BookmarksBar.svelte';
  import CommandPalette from '../lib/components/CommandPalette.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import PageSlot from '../lib/components/PageSlot.svelte';
  import SidePanel from '../lib/components/SidePanel.svelte';
  import TabSidebar from '../lib/components/TabSidebar.svelte';
  import TabStrip from '../lib/components/TabStrip.svelte';
  import Toolbar from '../lib/components/Toolbar.svelte';
  import { applyTheme, settings } from '../lib/settings.svelte';
  import { provideChrome, type ChromeController } from '../lib/state/chrome.svelte';

  let { chrome }: { chrome: ChromeController } = $props();
  // svelte-ignore state_referenced_locally
  provideChrome(chrome);

  $effect(() => applyTheme(document.documentElement, settings.value));
  $effect(() => {
    document.body.classList.toggle('embedded', chrome.api.kind === 'ws');
  });
  $effect(() => {
    const t = chrome.active?.title;
    document.title = t ? `${t} — BrowserMCP` : 'BrowserMCP';
  });

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (chrome.handleKey(e)) {
      e.preventDefault();
      e.stopPropagation();
    }
  }
</script>

<svelte:window onkeydowncapture={onKeydown} />

<div class="chrome" class:vertical={settings.value.verticalTabs}>
  {#if !settings.value.verticalTabs}
    <TabStrip />
  {/if}
  <Toolbar />
  {#if settings.value.bookmarksBar}<BookmarksBar />{/if}
  {#if chrome.active?.loading}
    <div class="progress" aria-hidden="true"></div>
  {/if}
  <AgentBanner />
  <div class="body">
    {#if settings.value.verticalTabs}
      <TabSidebar />
    {/if}
    <PageSlot />
    {#if settings.value.panel}
      <SidePanel panel={settings.value.panel} />
    {/if}
  </div>
  {#if chrome.lastError}
    <div class="toast" role="alert">
      {chrome.lastError}
      <button class="icon-btn" aria-label="Dismiss" onclick={() => (chrome.lastError = null)}><Icon name="x" size={14} /></button>
    </div>
  {/if}
  {#if !chrome.host}
    <!-- Inside the browser the palette is its own overlay view (palette.html). -->
    <CommandPalette />
  {/if}
</div>

<style>
  .chrome {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .body {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  .chrome.vertical .body {
    background: var(--frame);
  }
  .progress {
    position: relative;
    height: 2px;
    margin-top: -2px;
    overflow: hidden;
    z-index: 2;
  }
  .progress::after {
    content: '';
    position: absolute;
    inset: 0;
    width: 40%;
    background: var(--accent);
    border-radius: 2px;
    animation: sweep 1.1s var(--ease-out) infinite;
  }
  @keyframes sweep {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(250%);
    }
  }
  .toast {
    position: fixed;
    z-index: 50;
    left: 50%;
    bottom: 16px;
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: min(560px, calc(100vw - 32px));
    padding: 6px 6px 6px 14px;
    border-radius: var(--r-lg);
    background: var(--fg);
    color: var(--toolbar);
    font-size: var(--fs-sm);
    box-shadow: var(--shadow-pop);
    transform: translateX(-50%);
  }
  .toast .icon-btn {
    color: inherit;
  }
  :global(body.embedded) {
    background: transparent;
  }
  /* Inside the browser the page covers everything below the toolbar, so the toast sits
     over the tab strip instead of at the bottom. */
  :global(body.embedded) .toast {
    top: 4px;
    bottom: auto;
  }
</style>
