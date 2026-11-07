<script lang="ts">
  import { t } from '../i18n/index.svelte';
  import { settings } from '../settings.svelte';
  import { useChrome } from '../state/chrome.svelte';
  import { groupRuns } from '../state/tabs';
  import GroupLabel from './GroupLabel.svelte';
  import Icon from './Icon.svelte';
  import ResizeHandle from './ResizeHandle.svelte';
  import TabItem from './TabItem.svelte';
  import WorkspaceSwitcher from './WorkspaceSwitcher.svelte';

  const chrome = useChrome();
  const runs = $derived(groupRuns(chrome.tabs));
  let list: HTMLDivElement;

  $effect(() => {
    const id = chrome.tabs.activeId;
    if (id === null || !list) return;
    const frame = requestAnimationFrame(() =>
      list?.querySelector(`[data-tab-id="${id}"]`)?.scrollIntoView({ block: 'nearest' }),
    );
    return () => cancelAnimationFrame(frame);
  });
</script>

<aside class="sidebar" style:width="{settings.value.sidebarWidth}px" aria-label={t('components.tabs')}>
  <div class="head">
    <WorkspaceSwitcher wide />
  </div>
  <div class="list" role="tablist" aria-orientation="vertical" bind:this={list}>
    {#each runs as run, i (run.group?.id ?? `loose-${i}-${run.tabs[0].id}`)}
      {#if run.group}
        {@const collapsed = !!chrome.collapsed[run.group.id]}
        <div class="group" style:--group="var(--g-{run.group.color})">
          <GroupLabel
            vertical
            group={run.group}
            tabs={run.tabs}
            {collapsed}
            ontoggle={() => chrome.toggleGroup(run.group!.id)}
          />
          {#each run.tabs as tab (tab.id)}
            {#if !collapsed || tab.id === chrome.tabs.activeId}
              <TabItem vertical {tab} group={run.group} active={tab.id === chrome.tabs.activeId} />
            {/if}
          {/each}
        </div>
      {:else}
        {#each run.tabs as tab (tab.id)}
          <TabItem vertical {tab} active={tab.id === chrome.tabs.activeId} />
        {/each}
      {/if}
    {/each}
    <button class="new" onclick={() => chrome.newTab()} title="{t('components.newTab')} (Ctrl+T)">
      <Icon name="plus" size={15} stroke={2} />
      {t('components.newTab')}
    </button>
  </div>
  <ResizeHandle
    edge="right"
    value={settings.value.sidebarWidth}
    min={180}
    max={420}
    onchange={(w) => settings.update({ sidebarWidth: w })}
  />
</aside>

<style>
  .sidebar {
    position: relative;
    display: flex;
    flex-direction: column;
    flex: none;
    min-height: 0;
    background: var(--frame);
  }
  .head {
    padding: 6px 8px 4px;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 1px;
    flex: 1;
    min-height: 0;
    padding: 2px 8px 12px;
    overflow-y: auto;
    scrollbar-width: thin;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin: 4px 0;
    padding: 0 3px 3px;
    border-radius: var(--r-lg);
    background: color-mix(in oklab, var(--group) 9%, transparent);
  }
  .new {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 30px;
    margin-top: 2px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    text-align: left;
  }
  .new:hover {
    background: var(--hover);
    color: var(--fg);
  }
</style>
