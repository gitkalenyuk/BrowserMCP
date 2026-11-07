<script lang="ts">
  import { t } from '../i18n/index.svelte';
  import { untrack } from 'svelte';
  import { chromeBookmarks, isStarrable } from '../data/chrome-bookmarks.svelte';
  import { useChrome } from '../state/chrome.svelte';
  import Icon from './Icon.svelte';

  // The star at the end of the omnibox: bookmark the page (into the bookmarks bar) or remove it.
  const chrome = useChrome();
  const store = chromeBookmarks(chrome.api);
  const url = $derived(chrome.active?.url ?? null);
  const marked = $derived(store.checkedUrl === url && store.marked.length > 0);
  const key = $derived(chrome.keymap['bookmark.toggle'][0]);

  $effect(() => untrack(() => store.use()));
  $effect(() => {
    void store.check(url);
  });
</script>

{#if isStarrable(url)}
  <button
    class="star icon-btn"
    class:on={marked}
    aria-pressed={marked}
    aria-label={marked ? t('components.bookmark.remove') : t('components.bookmark.add')}
    title={(marked ? t('components.bookmark.remove') : t('components.bookmark.add')) + (key ? ` (${key})` : '')}
    disabled={store.busy}
    onmousedown={(e) => e.preventDefault()}
    onclick={() => store.toggle(chrome.active?.url, chrome.active?.title)}
  >
    <Icon name="star" size={15} stroke={1.9} />
  </button>
{/if}

<style>
  .star {
    width: 24px;
    height: 24px;
    border-radius: 12px;
  }
  .star.on,
  .star[aria-pressed='true'] {
    background: transparent;
    color: var(--accent);
  }
  .star.on :global(svg) {
    fill: currentColor;
  }
</style>
