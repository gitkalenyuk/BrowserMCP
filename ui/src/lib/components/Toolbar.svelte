<script lang="ts">
  import { t } from '../i18n/index.svelte';
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

  const PANELS: { id: PanelId; icon: IconName }[] = [
    { id: 'traffic', icon: 'traffic' },
    { id: 'recording', icon: 'record' },
    { id: 'ai', icon: 'sparkles' },
    { id: 'terminal', icon: 'terminal' },
    { id: 'scripts', icon: 'code' },
    { id: 'runs', icon: 'runs' },
  ];
</script>

<div class="toolbar">
  <div class="nav">
    <button class="icon-btn" aria-label={t('components.back')} title="{t('components.back')} (Alt+Left)" disabled={!tab?.canGoBack} onclick={chrome.back}>
      <Icon name="arrow-left" size={17} />
    </button>
    <button
      class="icon-btn"
      aria-label={t('components.forward')}
      title="{t('components.forward')} (Alt+Right)"
      disabled={!tab?.canGoForward}
      onclick={chrome.forward}
    >
      <Icon name="arrow-right" size={17} />
    </button>
    {#if tab?.loading}
      <button class="icon-btn" aria-label={t('components.stop')} title={t('components.stop')} onclick={chrome.stopLoading}>
        <Icon name="x" size={17} />
      </button>
    {:else}
      <button class="icon-btn" aria-label={t('components.reload')} title="{t('components.reload')} (Ctrl+R)" disabled={!tab} onclick={chrome.reload}>
        <Icon name="reload" size={16} />
      </button>
    {/if}
  </div>

  <Omnibox />

  <div class="panels" role="group" aria-label={t('components.sidePanels')}>
    {#each PANELS as p (p.id)}
      <button
        class="icon-btn"
        aria-label={t('components.panelLabel', { name: t(`components.panel.${p.id}`) })}
        title={keyHint(t(`components.panel.${p.id}`), `panel.${p.id}`)}
        aria-pressed={settings.value.panel === p.id}
        onclick={() => settings.togglePanel(p.id)}
      >
        <Icon name={p.icon} size={16} />
      </button>
    {/each}
  </div>

  <button class="icon-btn" aria-label={t('components.library')} title={keyHint(t('components.libraryHint'), 'library.bookmarks')} onclick={() => chrome.run('library.bookmarks')}>
    <Icon name="folder" size={16} />
  </button>
  <span class="divider" aria-hidden="true"></span>

  <button
    class="icon-btn"
    aria-label={settings.value.verticalTabs ? t('components.useHorizontalTabs') : t('components.useVerticalTabs')}
    title="{settings.value.verticalTabs ? t('components.horizontalTabs') : t('components.verticalTabs')} (Ctrl+Shift+E)"
    onclick={() => chrome.run('ui.toggleVerticalTabs')}
  >
    <Icon name={settings.value.verticalTabs ? 'tabs-horizontal' : 'tabs-vertical'} size={16} />
  </button>
  <button class="icon-btn" aria-label={t('palette.title')} title="{t('palette.title')} (Ctrl+K)" onclick={() => chrome.run('palette.open')}>
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
