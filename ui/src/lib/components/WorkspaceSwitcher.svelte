<script lang="ts">
  import { useChrome } from '../state/chrome.svelte';
  import Icon from './Icon.svelte';

  let { wide = false }: { wide?: boolean } = $props();

  const chrome = useChrome();
  const current = $derived(chrome.workspaces.find((w) => w.id === chrome.workspaceId));
  let open = $state(false);
  let root: HTMLDivElement;
  let menu = $state<HTMLDivElement>();

  function toggle() {
    open = !open;
    if (open) requestAnimationFrame(() => menu?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus());
  }

  function pick(id: string) {
    chrome.switchWorkspace(id);
    open = false;
  }

  function onMenuKey(e: KeyboardEvent) {
    const items = [...(menu?.querySelectorAll<HTMLElement>('[role="menuitemradio"]') ?? [])];
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const d = e.key === 'ArrowDown' ? 1 : -1;
      items[(i + d + items.length) % items.length]?.focus();
    } else if (e.key === 'Escape') {
      e.stopPropagation();
      open = false;
      root.querySelector<HTMLElement>('.trigger')?.focus();
    } else if (e.key === 'Tab') {
      open = false;
    }
  }
</script>

<svelte:window onpointerdown={(e) => open && !root.contains(e.target as Node) && (open = false)} />

<div class="ws" class:wide bind:this={root}>
  <button
    class="trigger"
    aria-haspopup="menu"
    aria-expanded={open}
    title={current ? `Workspace: ${current.name} · profile ${current.profile} · ${current.proxy}` : 'Workspaces'}
    onclick={toggle}
  >
    <span class="dot" style:--c="var(--g-{current?.color ?? 'grey'})"></span>
    <span class="name">{current?.name ?? 'Workspace'}</span>
    {#if wide && current}
      <span class="meta">{current.profile}</span>
    {/if}
    <Icon name="chevron-down" size={12} stroke={2.25} />
  </button>

  {#if open}
    <div class="menu" role="menu" aria-label="Workspaces" tabindex="-1" bind:this={menu} onkeydown={onMenuKey}>
      {#each chrome.workspaces as w (w.id)}
        <button
          class="item"
          role="menuitemradio"
          aria-checked={w.id === chrome.workspaceId}
          onclick={() => pick(w.id)}
        >
          <span class="dot" style:--c="var(--g-{w.color})"></span>
          <span class="text">
            <span class="title">{w.name}</span>
            <span class="sub">
              <span>Profile {w.profile}</span>
              <span class="sep" aria-hidden="true"></span>
              <span>{w.proxy}</span>
            </span>
          </span>
          {#if w.id === chrome.workspaceId}
            <span class="check"><Icon name="check" size={14} stroke={2.25} /></span>
          {/if}
        </button>
      {/each}
      <div class="foot">Each workspace has its own profile, cookies and proxy.</div>
    </div>
  {/if}
</div>

<style>
  .ws {
    position: relative;
    flex: none;
  }
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 26px;
    max-width: 170px;
    padding: 0 7px 0 8px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--fg);
    font-size: var(--fs-sm);
    font-weight: 600;
  }
  .wide .trigger {
    width: 100%;
    max-width: none;
    height: 30px;
  }
  .trigger:hover,
  .trigger[aria-expanded='true'] {
    background: var(--hover);
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--fg-subtle);
    font-weight: 400;
    text-align: left;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--c);
    flex: none;
  }
  .menu {
    position: absolute;
    z-index: 30;
    top: calc(100% + 6px);
    left: 0;
    width: 280px;
    padding: 5px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--shadow-pop);
    animation: pop 140ms var(--ease-out);
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 7px 8px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    text-align: left;
  }
  .item:hover,
  .item:focus-visible {
    background: var(--hover);
    outline: none;
  }
  .text {
    display: grid;
    flex: 1;
    min-width: 0;
  }
  .title {
    font-weight: 600;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
    overflow: hidden;
  }
  .sep {
    width: 3px;
    height: 3px;
    border-radius: 50%;
    background: currentColor;
    flex: none;
  }
  .check {
    color: var(--accent);
    display: inline-grid;
  }
  .foot {
    margin-top: 4px;
    padding: 7px 8px 4px;
    border-top: 1px solid var(--border);
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(-4px) scale(0.98);
    }
  }
</style>
