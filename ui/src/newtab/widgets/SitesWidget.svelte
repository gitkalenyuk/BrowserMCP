<script lang="ts">
  import { t } from '../../lib/i18n/index.svelte';
  import type { Bookmark, BrowserApi } from '../../lib/api';
  import Favicon from '../../lib/components/Favicon.svelte';
  import { isSafeLinkUrl } from '../../lib/safeurl';

  let { api, go }: { api: BrowserApi; go: (e: MouseEvent, url: string) => void } = $props();
  type Link = { title: string; url: string };
  let top = $state<Link[]>([]);
  let marks = $state<Link[]>([]);
  // Only http(s) URLs become links: a javascript: bookmark would run in the privileged origin.
  const links = (items: Bookmark[]): Link[] => items.filter((b): b is Link => isSafeLinkUrl(b.url));

  $effect(() => {
    Promise.all([api.call('data.top_sites', {}), api.call('data.bookmarks', {})])
      .then(([t, b]) => {
        top = links(t.sites);
        marks = links(b.bookmarks);
      })
      .catch(() => {});
  });
</script>

<section aria-labelledby="sites-title">
  <h2 id="sites-title">{t('newtab.sites.top')}</h2>
  <div class="grid">
    {#each top as s (s.url)}
      <a class="site" href={s.url} onclick={(e) => go(e, s.url)}>
        <span class="icon"><Favicon url={s.url} size={22} /></span>
        <span class="title">{s.title}</span>
      </a>
    {/each}
  </div>

  {#if marks.length}
    <h2 class="sub">{t('newtab.sites.bookmarks')}</h2>
    <div class="marks">
      {#each marks as b (b.url)}
        <a class="mark" href={b.url} onclick={(e) => go(e, b.url)}>
          <Favicon url={b.url} size={14} />
          <span>{b.title}</span>
        </a>
      {/each}
    </div>
  {/if}
</section>

<style>
  h2 {
    margin: 0 0 12px;
    font-size: var(--fs-lg);
    font-weight: 650;
  }
  h2.sub {
    margin-top: 24px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
    gap: 6px;
  }
  .site {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    padding: 14px 6px 10px;
    border-radius: var(--r-lg);
    color: var(--fg);
    text-decoration: none;
    transition: background-color 120ms;
  }
  .site:hover {
    background: var(--hover);
  }
  .icon {
    display: grid;
    place-items: center;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    background: var(--surface);
    box-shadow: var(--shadow-sm);
  }
  .title {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--fs-sm);
  }
  .marks {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .mark {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 11px 0 9px;
    border: 1px solid var(--border);
    border-radius: 15px;
    background: var(--surface);
    color: var(--fg);
    font-size: var(--fs-sm);
    text-decoration: none;
  }
  .mark:hover {
    border-color: var(--border-strong);
  }
</style>
