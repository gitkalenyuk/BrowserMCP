<script lang="ts">
  import type { GroupColor, Workspace } from '../api/types';
  import { useChrome } from '../state/chrome.svelte';
  import Icon from './Icon.svelte';

  // Workspaces are profiles (profile.list): own cookies, cache, proxy and identity. The strip
  // shows the tabs of the workspace picked here; new tabs open in it.
  let { wide = false }: { wide?: boolean } = $props();

  const COLORS: GroupColor[] = ['blue', 'purple', 'green', 'orange', 'pink', 'cyan', 'red', 'yellow', 'grey'];

  const chrome = useChrome();
  const current = $derived(chrome.current);
  let open = $state(false);
  /** Inline form: null, a new workspace, or renaming one. */
  let editing = $state<{ id: string | null; name: string; color: GroupColor } | null>(null);
  let confirmDelete = $state<string | null>(null);
  let root: HTMLDivElement;
  let menu = $state<HTMLDivElement>();

  // Inside the browser the page is hidden while the menu is open (it would cover it).
  $effect(() => {
    if (!open) return;
    chrome.dropdown(true);
    return () => chrome.dropdown(false);
  });

  function toggle() {
    open = !open;
    editing = null;
    confirmDelete = null;
    if (open) {
      void chrome.refreshWorkspaces().catch(() => {});
      requestAnimationFrame(() => menu?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus());
    }
  }

  function pick(id: string) {
    chrome.switchWorkspace(id);
    open = false;
  }

  function startNew() {
    confirmDelete = null;
    editing = { id: null, name: '', color: COLORS[chrome.workspaces.length % COLORS.length] };
    requestAnimationFrame(() => menu?.querySelector<HTMLInputElement>('input.name')?.focus());
  }

  function startRename(w: Workspace) {
    confirmDelete = null;
    editing = { id: w.id, name: w.name, color: w.color };
    requestAnimationFrame(() => menu?.querySelector<HTMLInputElement>('input.name')?.select());
  }

  async function save() {
    if (!editing || !editing.name.trim()) return;
    const { id, name, color } = editing;
    editing = null;
    if (id === null) {
      open = false;
      await chrome.createWorkspace(name, color);
    } else {
      await chrome.renameWorkspace(id, name, color);
    }
  }

  async function remove(id: string) {
    confirmDelete = null;
    open = false;
    await chrome.deleteWorkspace(id);
  }

  function tabCount(id: string): number {
    return chrome.allTabs.tabs.filter((t) => !t.offscreen && (t.profile ?? chrome.launchProfile) === id).length;
  }

  function onMenuKey(e: KeyboardEvent) {
    if (editing && (e.target as HTMLElement).tagName === 'INPUT') {
      if (e.key === 'Enter') {
        e.preventDefault();
        void save();
      } else if (e.key === 'Escape') {
        e.stopPropagation();
        editing = null;
      }
      return;
    }
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
        {#if editing?.id === w.id}
          {@render form()}
        {:else}
          <div class="row">
            <button
              class="item"
              role="menuitemradio"
              aria-checked={w.id === chrome.workspaceId}
              data-workspace={w.id}
              onclick={() => pick(w.id)}
            >
              <span class="dot" style:--c="var(--g-{w.color})"></span>
              <span class="text">
                <span class="title">{w.name} <span class="count">{tabCount(w.id)}</span></span>
                <span class="sub">
                  <span>Profile {w.profile}</span>
                  <span class="sep" aria-hidden="true"></span>
                  <span>{w.proxy}</span>
                  {#if w.identity}
                    <span class="sep" aria-hidden="true"></span>
                    <span title="Stealth identity">{w.identity}</span>
                  {/if}
                </span>
              </span>
              {#if w.id === chrome.workspaceId}
                <span class="check"><Icon name="check" size={14} stroke={2.25} /></span>
              {/if}
            </button>
            <button class="act" aria-label="Rename {w.name}" title="Rename / color" onclick={() => startRename(w)}>
              <Icon name="settings" size={13} />
            </button>
            {#if !w.launch}
              <button
                class="act danger"
                aria-label="Delete {w.name}"
                title="Delete workspace (closes its tabs, deletes its cookies)"
                onclick={() => (confirmDelete = confirmDelete === w.id ? null : w.id)}
              >
                <Icon name="trash" size={13} />
              </button>
            {/if}
          </div>
          {#if confirmDelete === w.id}
            <div class="confirm" role="alert">
              <span>Close {tabCount(w.id)} tab(s) and delete “{w.name}” with its cookies?</span>
              <button class="danger-btn" data-confirm-delete={w.id} onclick={() => remove(w.id)}>Delete</button>
              <button onclick={() => (confirmDelete = null)}>Cancel</button>
            </div>
          {/if}
        {/if}
      {/each}
      {#if editing?.id === null}
        {@render form()}
      {:else}
        <button class="item new" onclick={startNew}>
          <Icon name="plus" size={14} stroke={2.25} />
          <span>New workspace…</span>
        </button>
      {/if}
      <div class="foot">Each workspace has its own profile, cookies and proxy.</div>
    </div>
  {/if}
</div>

{#snippet form()}
  {#if editing}
    <div class="form">
      <input class="name" placeholder="Workspace name" maxlength="40" aria-label="Workspace name" bind:value={editing.name} />
      <div class="colors" role="radiogroup" aria-label="Color">
        {#each COLORS as c (c)}
          <button
            class="swatch"
            role="radio"
            aria-checked={editing.color === c}
            aria-label={c}
            style:--c="var(--g-{c})"
            onclick={() => editing && (editing.color = c)}
          ></button>
        {/each}
      </div>
      <div class="buttons">
        <button class="primary" disabled={!editing.name.trim()} onclick={save}>{editing.id === null ? 'Create' : 'Save'}</button>
        <button onclick={() => (editing = null)}>Cancel</button>
      </div>
    </div>
  {/if}
{/snippet}

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
    width: 320px;
    padding: 5px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--shadow-pop);
    animation: pop 140ms var(--ease-out);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: 1;
    min-width: 0;
    width: 100%;
    padding: 7px 8px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    text-align: left;
  }
  .item:hover,
  .item:focus-visible,
  .act:hover {
    background: var(--hover);
    outline: none;
  }
  .item.new {
    color: var(--fg-muted);
    font-size: var(--fs-sm);
  }
  .act {
    display: inline-grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--fg-subtle);
    flex: none;
  }
  .act.danger:hover {
    color: var(--danger, #d33);
  }
  .text {
    display: grid;
    flex: 1;
    min-width: 0;
  }
  .title {
    font-weight: 600;
  }
  .count {
    color: var(--fg-subtle);
    font-weight: 400;
    font-size: var(--fs-xs);
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
  .confirm,
  .form {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin: 2px 4px 6px;
    padding: 8px;
    border-radius: var(--r);
    background: var(--field);
    font-size: var(--fs-sm);
  }
  .confirm span {
    flex: 1 1 100%;
  }
  .form input {
    flex: 1 1 100%;
    height: 28px;
    padding: 0 8px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
  }
  .colors {
    display: flex;
    gap: 5px;
    flex: 1 1 100%;
  }
  .swatch {
    width: 16px;
    height: 16px;
    padding: 0;
    border: 2px solid transparent;
    border-radius: 5px;
    background: var(--c);
  }
  .swatch[aria-checked='true'] {
    border-color: var(--fg);
  }
  .buttons {
    display: flex;
    gap: 6px;
    margin-left: auto;
  }
  .form button:not(.swatch),
  .confirm button {
    height: 26px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg);
    font-size: var(--fs-sm);
  }
  .primary {
    background: var(--accent) !important;
    border-color: transparent !important;
    color: white !important;
  }
  .primary:disabled {
    opacity: 0.5;
  }
  .danger-btn {
    background: var(--danger, #d33) !important;
    border-color: transparent !important;
    color: white !important;
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
