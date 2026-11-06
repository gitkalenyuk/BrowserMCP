<script lang="ts">
  import { useChrome } from '../state/chrome.svelte';
  import Icon from './Icon.svelte';

  const chrome = useChrome();
  const tab = $derived(chrome.active);
  const agent = $derived(chrome.agent(tab?.lockedBy ?? null));
</script>

{#if tab && agent}
  <div class="banner" role="status" style:--agent="var(--g-{agent.color})">
    <span class="live" aria-hidden="true"></span>
    <span class="who">{agent.name} is controlling this tab</span>
    {#if agent.task}
      <span class="task">{agent.task}</span>
    {/if}
    <span class="note">Your input is blocked until you take over.</span>
    <button class="btn takeover" onclick={() => chrome.takeOver(tab.id)}>
      <Icon name="hand" size={14} stroke={2} />
      Take over
    </button>
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
