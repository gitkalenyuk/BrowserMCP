<script lang="ts">
  import { useChrome } from '../state/chrome.svelte';
  import { lockState } from '../state/tabs';
  import Icon from './Icon.svelte';

  const chrome = useChrome();
  const tab = $derived(chrome.active);
  // The tab's agent lock: active (agent drives), handoff (agent waits for you), paused (you took over).
  const state = $derived(tab ? lockState(tab) : null);
  const agent = $derived(chrome.agent(tab?.lock?.owner ?? tab?.lockedBy ?? null));
</script>

{#if tab && agent && state === 'handoff'}
  <div class="banner handoff" role="alert" style:--agent="var(--g-{agent.color})">
    <Icon name="hand" size={14} stroke={2} />
    <span class="who">{agent.name} needs you</span>
    <span class="task">{tab.lock?.reason ?? 'Finish this step in the page.'}</span>
    <span class="note">Click Done when you are finished; {agent.name} continues.</span>
    <button class="btn takeover" onclick={() => chrome.finishHandoff(tab.id)}>Done</button>
  </div>
{:else if tab && agent && state === 'active'}
  <div class="banner" role="status" style:--agent="var(--g-{agent.color})">
    <span class="live" aria-hidden="true"></span>
    <span class="who">{agent.name} is controlling this tab</span>
    {#if agent.task}
      <span class="task">{agent.task}</span>
    {/if}
    <span class="note">Other agents are locked out. Take over to pause it.</span>
    <button class="btn takeover" onclick={() => chrome.takeOver(tab.id)}>
      <Icon name="hand" size={14} stroke={2} />
      Take over
    </button>
  </div>
{:else if tab && agent && state === 'paused'}
  <div class="banner paused" role="status" style:--agent="var(--g-{agent.color})">
    <span class="who">You took over from {agent.name}</span>
    <span class="note">{agent.name} cannot change this tab until you give it back.</span>
    <button class="btn takeover" onclick={() => chrome.giveBack(tab.id)}>Give back</button>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 34px;
    padding: 0 6px 0 14px;
    background: color-mix(in oklab, var(--agent) 12%, var(--toolbar));
    border-bottom: 1px solid color-mix(in oklab, var(--agent) 30%, var(--border));
    font-size: var(--fs-sm);
    white-space: nowrap;
    overflow: hidden;
  }
  .banner.paused {
    background: var(--toolbar);
  }
  .banner.handoff {
    background: color-mix(in oklab, var(--agent) 22%, var(--toolbar));
  }
  .live {
    position: relative;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--agent);
    flex: none;
  }
  .live::after {
    content: '';
    position: absolute;
    inset: -4px;
    border-radius: 50%;
    border: 2px solid var(--agent);
    opacity: 0;
    animation: ping 1.8s var(--ease-out) infinite;
  }
  .who {
    font-weight: 650;
    color: var(--fg);
  }
  .task {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--fg-muted);
  }
  .note {
    margin-left: auto;
    color: var(--fg-subtle);
  }
  .takeover {
    height: 26px;
    border-color: color-mix(in oklab, var(--agent) 50%, transparent);
    color: var(--fg);
  }
  .takeover:hover {
    background: color-mix(in oklab, var(--agent) 16%, var(--surface)) !important;
  }
  @keyframes ping {
    0% {
      transform: scale(0.6);
      opacity: 0.8;
    }
    100% {
      transform: scale(1.5);
      opacity: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .live::after {
      animation: none;
    }
  }
  @media (max-width: 820px) {
    .note {
      display: none;
    }
    .takeover {
      margin-left: auto;
    }
  }
</style>
