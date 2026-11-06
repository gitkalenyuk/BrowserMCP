<script lang="ts">
  import type { TabGroup, TabInfo } from '../api/types';
  import { useChrome } from '../state/chrome.svelte';
  import Icon from './Icon.svelte';

  let {
    group,
    tabs,
    collapsed,
    ontoggle,
    vertical = false,
  }: {
    group: TabGroup;
    tabs: TabInfo[];
    collapsed: boolean;
    ontoggle: () => void;
    vertical?: boolean;
  } = $props();

  const chrome = useChrome();
  const agent = $derived(chrome.agent(group.agentId));
  const locked = $derived(!!agent && tabs.some((t) => t.lockedBy === agent.id));
  const label = $derived(
    `${group.name} · ${tabs.length} tab${tabs.length === 1 ? '' : 's'}` +
      (locked && agent ? ` · controlled by ${agent.name}${agent.task ? `: ${agent.task}` : ''}` : ''),
  );
</script>

<div class="group" class:vertical style:--group="var(--g-{group.color})">
  <button class="chip" class:locked aria-expanded={!collapsed} title={label} onclick={ontoggle}>
    {#if vertical}
      <span class="chev" class:collapsed><Icon name="chevron-down" size={12} stroke={2.25} /></span>
    {/if}
    {#if agent}
      <Icon name="bot" size={13} stroke={2} />
    {/if}
    <span class="name">{group.name}</span>
    {#if locked}
      <Icon name="lock" size={11} stroke={2.25} />
    {/if}
    {#if collapsed}
      <span class="count">{tabs.length}</span>
    {/if}
  </button>
  {#if locked && agent && vertical}
    <button class="takeover" onclick={() => chrome.takeOverAgent(agent.id)} title="Take control of {agent.name}'s tabs">
      <Icon name="hand" size={12} stroke={2} />
      Take over
    </button>
  {/if}
</div>

<style>
  .group {
    display: flex;
    align-items: center;
    flex: none;
    gap: 4px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 22px;
    padding: 0 8px;
    border: 0;
    border-radius: 6px;
    background: color-mix(in oklab, var(--group) 16%, transparent);
    color: var(--group);
    font-size: var(--fs-xs);
    font-weight: 650;
    letter-spacing: 0.01em;
    white-space: nowrap;
  }
  .chip:hover {
    background: color-mix(in oklab, var(--group) 24%, transparent);
  }
  .chip.locked {
    background: var(--group);
    color: var(--on-group);
  }
  .chip.locked:hover {
    background: color-mix(in oklab, var(--group) 88%, var(--fg));
  }
  .count {
    padding: 0 5px;
    border-radius: 8px;
    background: color-mix(in oklab, currentColor 18%, transparent);
    font-variant-numeric: tabular-nums;
  }
  .chev {
    display: inline-grid;
    transition: transform 150ms var(--ease-out);
  }
  .chev.collapsed {
    transform: rotate(-90deg);
  }

  .group.vertical {
    justify-content: space-between;
    padding: 6px 4px 4px 6px;
  }
  .group.vertical .chip {
    max-width: 100%;
    overflow: hidden;
  }
  .takeover {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 22px;
    padding: 0 7px;
    border: 1px solid color-mix(in oklab, var(--group) 45%, transparent);
    border-radius: 6px;
    background: transparent;
    color: var(--group);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
  }
  .takeover:hover {
    background: color-mix(in oklab, var(--group) 14%, transparent);
  }
</style>
