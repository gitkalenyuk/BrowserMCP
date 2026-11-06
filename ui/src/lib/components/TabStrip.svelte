<script lang="ts">
  import { useChrome } from '../state/chrome.svelte';
  import { groupRuns } from '../state/tabs';
  import GroupLabel from './GroupLabel.svelte';
  import Icon from './Icon.svelte';
  import TabItem from './TabItem.svelte';
  import WorkspaceSwitcher from './WorkspaceSwitcher.svelte';

  const chrome = useChrome();
  const runs = $derived(groupRuns(chrome.tabs));
  let scroller: HTMLDivElement;

  // Keep the active tab visible when it changes (keyboard switching, new tabs).
  $effect(() => {
    const id = chrome.tabs.activeId;
    if (id === null || !scroller) return;
    const frame = requestAnimationFrame(() => {
      scroller?.querySelector(`[data-tab-id="${id}"]`)?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    });
    return () => cancelAnimationFrame(frame);
  });

  function onWheel(e: WheelEvent) {
    if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
      scroller.scrollLeft += e.deltaY;
      e.preventDefault();
    }
  }
</script>

<div class="strip">
  <WorkspaceSwitcher />
  <div class="tabs" role="tablist" aria-label="Tabs" bind:this={scroller} onwheel={onWheel}>
    {#each runs as run, i (run.group?.id ?? `loose-${i}-${run.tabs[0].id}`)}
      {#if run.group}
        {@const collapsed = !!chrome.collapsed[run.group.id]}
        <GroupLabel group={run.group} tabs={run.tabs} {collapsed} ontoggle={() => chrome.toggleGroup(run.group!.id)} />
        {#each run.tabs as tab (tab.id)}
          {#if !collapsed || tab.id === chrome.tabs.activeId}
            <TabItem {tab} group={run.group} active={tab.id === chrome.tabs.activeId} />
          {/if}
        {/each}
      {:else}
        {#each run.tabs as tab (tab.id)}
          <TabItem {tab} active={tab.id === chrome.tabs.activeId} />
        {/each}
      {/if}
    {/each}
    <button class="icon-btn new" aria-label="New tab" title="New tab (Ctrl+T)" onclick={() => chrome.newTab()}>
      <Icon name="plus" size={16} stroke={2} />
    </button>
  </div>
</div>

<style>
  .strip {
    display: flex;
    align-items: center;
    gap: 6px;
    height: var(--strip-h);
    padding: 0 8px 0 6px;
    background: var(--frame);
  }
  .tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: 1;
    min-width: 0;
    height: 100%;
    padding: 3px 2px 0;
    overflow-x: auto;
    overflow-y: hidden;
    scrollbar-width: none;
  }
  .tabs > :global(.group) {
    margin: 0 2px 0 6px;
  }
  .new {
    width: 28px;
    height: 28px;
    margin-left: 2px;
  }
</style>
