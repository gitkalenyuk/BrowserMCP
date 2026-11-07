<script lang="ts">
  import { t } from '../i18n/index.svelte';
  import { untrack } from 'svelte';
  import { OTHER_BOOKMARKS } from '../api/types-data';
  import { chromeBookmarks, type BarItem } from '../data/chrome-bookmarks.svelte';
  import { openLibrary } from '../data/library';
  import { isSafeLinkUrl } from '../safeurl';
  import { useChrome } from '../state/chrome.svelte';
  import Favicon from './Favicon.svelte';
  import Icon from './Icon.svelte';

  // The bookmarks bar under the toolbar (Ctrl+Shift+B). Folders open in the library: a dropdown
  // would reach into the page area, where the page view covers the chrome.
  const chrome = useChrome();
  const store = chromeBookmarks(chrome.api);
  const items = $derived(store.bar.filter((b) => b.kind === 'folder' || isSafeLinkUrl(b.url)));

  $effect(() => untrack(() => store.use()));

  function open(e: MouseEvent, b: BarItem) {
    if (b.kind === 'folder') return openLibrary(chrome, 'bookmarks', b.id);
    if (!isSafeLinkUrl(b.url)) return;
    if (e.button === 1 || e.ctrlKey || e.metaKey) {
      void chrome.api.call('tabs.open', { url: b.url, background: !e.shiftKey }).catch(() => {});
    } else {
      chrome.navigate(b.url);
    }
  }
</script>

<div class="bar" role="toolbar" aria-label={t('components.bookmarksBar')}>
  <div class="items">
    {#each items as b (b.id)}
      <button
        class="item"
        title={b.kind === 'url' ? `${b.title}\n${b.url}` : t('components.bookmarksBar.folderHint', { title: b.title })}
        onclick={(e) => open(e, b)}
        onauxclick={(e) => e.button === 1 && open(e, b)}
      >
        {#if b.kind === 'folder'}
          <Icon name="folder" size={14} />
        {:else}
          <Favicon url={b.url} size={14} />
        {/if}
        <span>{b.title || (b.kind === 'url' ? b.url : t('components.bookmarksBar.folder'))}</span>
      </button>
    {:else}
      <span class="hint">
        {t('components.bookmarksBar.hintBefore')} <Icon name="star" size={12} /> {t('components.bookmarksBar.hintAfter')}
        <button class="link" onclick={() => openLibrary(chrome, 'import')}>{t('components.bookmarksBar.import')}</button>
      </span>
    {/each}
  </div>
  <button class="item other" title={t('components.bookmarksBar.other')} onclick={() => openLibrary(chrome, 'bookmarks', OTHER_BOOKMARKS)}>
    <Icon name="folder" size={14} />
    <span>{t('components.bookmarksBar.other')}</span>
  </button>
</div>

<style>
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 30px;
    padding: 0 8px;
    background: var(--toolbar);
    border-bottom: 1px solid var(--border);
    overflow: hidden;
  }
  .items {
    display: flex;
    align-items: center;
    gap: 2px;
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }
  .item {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 180px;
    height: 24px;
    padding: 0 8px;
    border: 0;
    border-radius: 12px;
    background: transparent;
    color: var(--fg-muted);
    font-size: var(--fs-sm);
    flex: none;
  }
  .item:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .item:active {
    background: var(--press);
  }
  .item span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .item :global(svg) {
    flex: none;
    color: var(--fg-subtle);
  }
  .other {
    border-left: 1px solid var(--border);
  }
  .hint {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 0 6px;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
    white-space: nowrap;
  }
  .link {
    margin-left: 6px;
    padding: 0;
    border: 0;
    background: transparent;
    color: var(--accent);
    font-size: inherit;
  }
  .link:hover {
    text-decoration: underline;
  }
</style>
