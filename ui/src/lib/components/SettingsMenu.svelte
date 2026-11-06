<script lang="ts">
  import { ACCENTS, settings, type ThemePref } from '../settings.svelte';
  import { useChrome } from '../state/chrome.svelte';
  import type { CommandId } from '../keymap';
  import Icon from './Icon.svelte';

  const chrome = useChrome();
  let open = $state(false);
  let root: HTMLDivElement;

  const LIBRARY: { id: CommandId; label: string }[] = [
    { id: 'library.bookmarks', label: 'Bookmarks' },
    { id: 'library.history', label: 'History' },
    { id: 'library.downloads', label: 'Downloads' },
    { id: 'library.passwords', label: 'Passwords' },
    { id: 'library.import', label: 'Import from Chrome…' },
  ];
  const THEMES: { id: ThemePref; label: string }[] = [
    { id: 'system', label: 'System' },
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
  ];
  const statusLabel = $derived(
    chrome.api.kind === 'mock'
      ? 'Mock core (in-memory)'
      : chrome.status === 'open'
        ? 'Connected to core'
        : chrome.status === 'connecting'
          ? 'Connecting to core…'
          : 'Disconnected',
  );
</script>

<svelte:window
  onpointerdown={(e) => open && !root.contains(e.target as Node) && (open = false)}
  onkeydown={(e) => open && e.key === 'Escape' && (open = false)}
/>

<div class="wrap" bind:this={root}>
  <button
    class="icon-btn"
    aria-label="Appearance and settings"
    title="Appearance"
    aria-haspopup="dialog"
    aria-expanded={open}
    onclick={() => (open = !open)}
  >
    <Icon name="menu" size={18} stroke={2.5} />
  </button>
  {#if open}
    <div class="pop" role="dialog" aria-label="Appearance">
      <section>
        <h3>Theme</h3>
        <div class="seg" role="radiogroup" aria-label="Theme">
          {#each THEMES as t (t.id)}
            <button
              role="radio"
              aria-checked={settings.value.theme === t.id}
              onclick={() => settings.update({ theme: t.id })}>{t.label}</button
            >
          {/each}
        </div>
      </section>
      <section>
        <h3>Accent</h3>
        <div class="swatches" role="radiogroup" aria-label="Accent color">
          {#each ACCENTS as c (c)}
            <button
              class="swatch"
              role="radio"
              aria-checked={settings.value.accent === c}
              aria-label="Accent {c}"
              style:--c={c}
              onclick={() => settings.update({ accent: c })}
            ></button>
          {/each}
          <label class="swatch custom" title="Custom color">
            <input
              type="color"
              value={settings.value.accent}
              oninput={(e) => settings.update({ accent: e.currentTarget.value })}
              aria-label="Custom accent color"
            />
            <Icon name="plus" size={12} stroke={2.5} />
          </label>
        </div>
      </section>
      <section>
        <h3>Tabs</h3>
        <div class="seg" role="radiogroup" aria-label="Tab layout">
          <button role="radio" aria-checked={!settings.value.verticalTabs} onclick={() => settings.update({ verticalTabs: false })}>
            Horizontal
          </button>
          <button role="radio" aria-checked={settings.value.verticalTabs} onclick={() => settings.update({ verticalTabs: true })}>
            Vertical
          </button>
        </div>
      </section>
      <section>
        <h3>Library</h3>
        <div class="menu">
          {#each LIBRARY as item (item.id)}
            <button onclick={() => ((open = false), chrome.run(item.id))}>
              <span>{item.label}</span>
              {#if chrome.keymap[item.id][0]}<kbd>{chrome.keymap[item.id][0]}</kbd>{/if}
            </button>
          {/each}
          <button role="menuitemcheckbox" aria-checked={settings.value.bookmarksBar} onclick={() => chrome.run('ui.toggleBookmarksBar')}>
            <span>{settings.value.bookmarksBar ? 'Hide' : 'Show'} bookmarks bar</span>
            <kbd>{chrome.keymap['ui.toggleBookmarksBar'][0]}</kbd>
          </button>
        </div>
      </section>
      <section>
        <div class="menu">
          <button onclick={() => ((open = false), chrome.run('settings.open'))}>
            <span>All settings…</span>
            {#if chrome.keymap['settings.open'][0]}<kbd>{chrome.keymap['settings.open'][0]}</kbd>{/if}
          </button>
        </div>
      </section>
      <footer>
        <span class="status" class:ok={chrome.status === 'open'}></span>
        {statusLabel}
      </footer>
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
  }
  .pop {
    position: absolute;
    z-index: 30;
    top: calc(100% + 6px);
    right: 0;
    width: 284px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--shadow-pop);
    animation: pop 140ms var(--ease-out);
  }
  section + section {
    margin-top: 14px;
  }
  h3 {
    margin: 0 0 7px;
    color: var(--fg-muted);
    font-size: var(--fs-xs);
    font-weight: 600;
  }
  .seg {
    display: flex;
    padding: 2px;
    border-radius: var(--r);
    background: var(--sunken);
  }
  .seg button {
    flex: 1;
    height: 26px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
  }
  .seg button:hover {
    color: var(--fg);
  }
  .seg button[aria-checked='true'] {
    background: var(--surface);
    color: var(--fg);
    font-weight: 600;
    box-shadow: var(--shadow-tab);
  }
  .swatches {
    display: flex;
    flex-wrap: wrap;
    gap: 7px;
  }
  .swatch {
    position: relative;
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: var(--c);
  }
  .swatch[aria-checked='true'] {
    box-shadow: 0 0 0 2px var(--surface), 0 0 0 4px var(--c);
  }
  .custom {
    background: var(--sunken);
    color: var(--fg-muted);
    border: 1px dashed var(--border-strong);
  }
  .custom input {
    position: absolute;
    inset: 0;
    opacity: 0;
    width: 100%;
    height: 100%;
    cursor: pointer;
  }
  .menu {
    display: flex;
    flex-direction: column;
    margin: 0 -6px;
  }
  .menu button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 28px;
    padding: 0 6px;
    border: 0;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--fg);
    font-size: var(--fs-sm);
    text-align: left;
  }
  .menu button:hover {
    background: var(--hover);
  }
  footer {
    display: flex;
    align-items: center;
    gap: 7px;
    margin-top: 14px;
    padding-top: 10px;
    border-top: 1px solid var(--border);
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
  }
  .status {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--warn);
  }
  .status.ok {
    background: var(--ok);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(-4px) scale(0.98);
    }
  }
</style>
