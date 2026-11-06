<script lang="ts">
  import { settings, type PanelId } from '../settings.svelte';
  import { useChrome } from '../state/chrome.svelte';
  import type { CommandId } from '../keymap';
  import Icon, { type IconName } from './Icon.svelte';
  import Omnibox from './Omnibox.svelte';
  import SettingsMenu from './SettingsMenu.svelte';

  const chrome = useChrome();
  const tab = $derived(chrome.active);
  const keyHint = (label: string, id: CommandId) => {
    const k = chrome.keymap[id][0];
    return k ? `${label} (${k})` : label;
  };

  const PANELS: { id: PanelId; icon: IconName; label: string }[] = [
    { id: 'traffic', icon: 'traffic', label: 'Traffic' },
    { id: 'recording', icon: 'record', label: 'Recording' },
    { id: 'ai', icon: 'sparkles', label: 'AI' },
    { id: 'terminal', icon: 'terminal', label: 'Terminal' },
    { id: 'scripts', icon: 'code', label: 'Scripts' },
    { id: 'runs', icon: 'runs', label: 'Runs' },
  ];
</script>

<div class="toolbar">
  <div class="nav">
    <button class="icon-btn" aria-label="Back" title="Back (Alt+Left)" disabled={!tab?.canGoBack} onclick={chrome.back}>
      <Icon name="arrow-left" size={17} />
    </button>
    <button
      class="icon-btn"
      aria-label="Forward"
      title="Forward (Alt+Right)"
      disabled={!tab?.canGoForward}
      onclick={chrome.forward}
    >
      <Icon name="arrow-right" size={17} />
    </button>
    {#if tab?.loading}
      <button class="icon-btn" aria-label="Stop loading" title="Stop loading" onclick={chrome.stopLoading}>
        <Icon name="x" size={17} />
      </button>
    {:else}
      <button class="icon-btn" aria-label="Reload" title="Reload (Ctrl+R)" disabled={!tab} onclick={chrome.reload}>
        <Icon name="reload" size={16} />
      </button>
    {/if}
  </div>

  <Omnibox />

  <div class="panels" role="group" aria-label="Side panels">
    {#each PANELS as p (p.id)}
      <button
        class="icon-btn"
        aria-label="{p.label} panel"
        title={keyHint(p.label, `panel.${p.id}`)}
        aria-pressed={settings.value.panel === p.id}
        onclick={() => settings.togglePanel(p.id)}
      >
        <Icon name={p.icon} size={16} />
      </button>
    {/each}
  </div>

  <button class="icon-btn" aria-label="Library" title={keyHint('Bookmarks, history and passwords', 'library.bookmarks')} onclick={() => chrome.run('library.bookmarks')}>
    <Icon name="folder" size={16} />
  </button>
  <span class="divider" aria-hidden="true"></span>

  <button
    class="icon-btn"
    aria-label={settings.value.verticalTabs ? 'Use horizontal tabs' : 'Use vertical tabs'}
    title={settings.value.verticalTabs ? 'Horizontal tabs (Ctrl+Shift+E)' : 'Vertical tabs (Ctrl+Shift+E)'}
    onclick={() => chrome.run('ui.toggleVerticalTabs')}
  >
    <Icon name={settings.value.verticalTabs ? 'tabs-horizontal' : 'tabs-vertical'} size={16} />
  </button>
  <button class="icon-btn" aria-label="Command palette" title="Command palette (Ctrl+K)" onclick={() => chrome.run('palette.open')}>
    <Icon name="command" size={15} />
  </button>
  <SettingsMenu />
</div>

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    height: var(--toolbar-h);
    padding: 0 6px;
    background: var(--toolbar);
    border-bottom: 1px solid var(--border);
  }
  .nav,
  .panels {
    display: flex;
    align-items: center;
    gap: 1px;
  }
  .divider {
    width: 1px;
    height: 18px;
    background: var(--border);
    margin: 0 2px;
  }
  @media (max-width: 720px) {
    .panels,
    .divider {
      display: none;
    }
  }
</style>
