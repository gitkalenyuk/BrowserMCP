<script lang="ts">
  import type { Agent, TabGroup, TabInfo } from '../api/types';
  import { useChrome } from '../state/chrome.svelte';
  import { fallbackTitle } from '../urls';
  import Favicon from './Favicon.svelte';
  import Icon from './Icon.svelte';

  let {
    tab,
    active,
    group = null,
    vertical = false,
  }: { tab: TabInfo; active: boolean; group?: TabGroup | null; vertical?: boolean } = $props();

  const chrome = useChrome();
  const agent: Agent | undefined = $derived(chrome.agent(tab.lockedBy));
  const title = $derived(tab.title || fallbackTitle(tab.url));
  const tooltip = $derived(agent ? `${title}\nControlled by ${agent.name}` : `${title}\n${tab.url}`);

  function onAux(e: MouseEvent) {
    if (e.button === 1) {
      e.preventDefault();
      chrome.closeTab(tab.id);
    }
  }
</script>

<div
  class="tab"
  class:vertical
  class:active
  class:grouped={!!group}
  class:locked={!!agent}
  style:--group={group ? `var(--g-${group.color})` : null}
  role="tab"
  tabindex={active ? 0 : -1}
  aria-selected={active}
  title={tooltip}
  data-tab-id={tab.id}
  onclick={() => chrome.activate(tab.id)}
  onmousedown={(e) => e.button === 1 && e.preventDefault()}
  onauxclick={onAux}
  onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && chrome.activate(tab.id)}
>
  <Favicon url={tab.url} src={tab.favicon} loading={tab.loading} />
  <span class="title">{title}</span>
  {#if agent}
    <span class="lock" title="Locked by {agent.name}" style:--agent="var(--g-{agent.color})">
      <Icon name="lock" size={12} stroke={2} />
    </span>
  {/if}
  <button
    class="close"
    aria-label="Close tab"
    title="Close tab (Ctrl+W)"
    onclick={(e) => {
      e.stopPropagation();
      chrome.closeTab(tab.id);
    }}
  >
    <Icon name="x" size={13} stroke={2} />
  </button>
</div>

<style>
  .tab {
    position: relative;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    height: var(--tab-h);
    padding: 0 4px 0 10px;
    border-radius: var(--r);
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    transition: background-color 100ms, color 100ms;
  }
  .tab:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .tab.active {
    color: var(--fg);
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    /* Fade instead of an ellipsis: keeps more of the title readable in narrow tabs. */
    mask-image: linear-gradient(to right, #000 calc(100% - 18px), transparent);
  }
  .lock {
    display: inline-grid;
    place-items: center;
    color: var(--agent);
    flex: none;
  }
  .close {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    color: var(--fg-subtle);
    flex: none;
    opacity: 0;
  }
  .tab:hover .close,
  .tab.active .close,
  .close:focus-visible {
    opacity: 1;
  }
  .close:hover {
    background: var(--press);
    color: var(--fg);
  }

  /* ---- horizontal (strip) ---- */
  .tab:not(.vertical) {
    flex: 1 1 200px;
    max-width: 220px;
    min-width: 44px;
  }
  .tab:not(.vertical).active {
    background: var(--toolbar);
    box-shadow: var(--shadow-tab);
  }
  .tab:not(.vertical).grouped::after {
    content: '';
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: -3px;
    height: 2px;
    border-radius: 2px;
    background: var(--group);
  }
  /* Very narrow tabs: favicon only (the active one keeps its close button). */
  .tab:not(.vertical) {
    container-type: inline-size;
  }
  @container (max-width: 72px) {
    .title,
    .tab:not(.active) .close {
      display: none;
    }
  }

  /* ---- vertical (sidebar) ---- */
  .tab.vertical {
    width: 100%;
    height: 30px;
  }
  .tab.vertical.active {
    background: var(--surface);
    box-shadow: var(--shadow-tab);
  }
</style>
