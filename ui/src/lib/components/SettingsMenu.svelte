<script lang="ts">
  import { t } from '../i18n/index.svelte';
  import { ACCENTS, settings, type ThemePref } from '../settings.svelte';
  import { useChrome } from '../state/chrome.svelte';
  import type { CommandId } from '../keymap';
  import Icon from './Icon.svelte';

  const chrome = useChrome();
  let open = $state(false);
  let root: HTMLDivElement;
  // Inside the browser the page is hidden while the menu is open (it would cover it).
  $effect(() => {
    if (!open) return;
    chrome.dropdown(true);
    return () => chrome.dropdown(false);
  });

  // Labels are keys (`components.menu.<id>`), translated in the markup.
  const LIBRARY: CommandId[] = ['library.bookmarks', 'library.history', 'library.downloads', 'library.passwords', 'library.import'];
  const THEMES: ThemePref[] = ['system', 'light', 'dark'];
  const statusLabel = $derived(
    chrome.api.kind === 'mock'
      ? t('components.menu.status.mock')
      : chrome.status === 'open'
        ? t('components.menu.status.open')
        : chrome.status === 'connecting'
          ? t('components.menu.status.connecting')
          : t('components.menu.status.closed'),
  );
</script>

<svelte:window
  onpointerdown={(e) => open && !root.contains(e.target as Node) && (open = false)}
  onkeydown={(e) => open && e.key === 'Escape' && (open = false)}
/>

<div class="wrap" bind:this={root}>
  <button
    class="icon-btn"
    aria-label={t('components.menu.label')}
    title={t('components.menu.appearance')}
    aria-haspopup="dialog"
    aria-expanded={open}
    onclick={() => (open = !open)}
  >
    <Icon name="menu" size={18} stroke={2.5} />
  </button>
  {#if open}
    <div class="pop" role="dialog" aria-label={t('components.menu.appearance')}>
      <section>
        <h3>{t('components.menu.theme')}</h3>
        <div class="seg" role="radiogroup" aria-label={t('components.menu.theme')}>
          {#each THEMES as theme (theme)}
            <button
              role="radio"
              aria-checked={settings.value.theme === theme}
              onclick={() => settings.update({ theme })}>{t(`components.menu.theme.${theme}`)}</button
            >
          {/each}
        </div>
      </section>
      <section>
        <h3>{t('components.menu.accent')}</h3>
        <div class="swatches" role="radiogroup" aria-label={t('components.menu.accentColor')}>
          {#each ACCENTS as c (c)}
            <button
              class="swatch"
              role="radio"
              aria-checked={settings.value.accent === c}
              aria-label={t('components.menu.accentSwatch', { color: c })}
              style:--c={c}
              onclick={() => settings.update({ accent: c })}
            ></button>
          {/each}
          <label class="swatch custom" title={t('components.menu.customColor')}>
            <input
              type="color"
              value={settings.value.accent}
              oninput={(e) => settings.update({ accent: e.currentTarget.value })}
              aria-label={t('components.menu.customAccent')}
            />
            <Icon name="plus" size={12} stroke={2.5} />
          </label>
        </div>
      </section>
      <section>
        <h3>{t('components.tabs')}</h3>
        <div class="seg" role="radiogroup" aria-label={t('components.menu.tabLayout')}>
          <button role="radio" aria-checked={!settings.value.verticalTabs} onclick={() => settings.update({ verticalTabs: false })}>
            {t('components.menu.horizontal')}
          </button>
          <button role="radio" aria-checked={settings.value.verticalTabs} onclick={() => settings.update({ verticalTabs: true })}>
            {t('components.menu.vertical')}
          </button>
        </div>
      </section>
      <section>
        <h3>{t('components.library')}</h3>
        <div class="menu">
          {#each LIBRARY as id (id)}
            <button onclick={() => ((open = false), chrome.run(id))}>
              <span>{t(`components.menu.${id}`)}</span>
              {#if chrome.keymap[id][0]}<kbd>{chrome.keymap[id][0]}</kbd>{/if}
            </button>
          {/each}
          <button role="menuitemcheckbox" aria-checked={settings.value.bookmarksBar} onclick={() => chrome.run('ui.toggleBookmarksBar')}>
            <span>{settings.value.bookmarksBar ? t('components.menu.hideBar') : t('components.menu.showBar')}</span>
            <kbd>{chrome.keymap['ui.toggleBookmarksBar'][0]}</kbd>
          </button>
        </div>
      </section>
      <section>
        <div class="menu">
          <button onclick={() => ((open = false), chrome.run('settings.open'))}>
            <span>{t('components.menu.allSettings')}</span>
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
