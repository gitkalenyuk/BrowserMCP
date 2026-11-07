<script lang="ts">
  import type { BrowserApi } from '../lib/api';
  import type { ConnectionStatus } from '../lib/api/types';
  import Icon, { type IconName } from '../lib/components/Icon.svelte';
  import { onRequest, takeRequest, type LibrarySection } from '../lib/data/library';
  import { BOOKMARKS_BAR } from '../lib/api/types-data';
  import { applyLang, t } from '../lib/i18n/index.svelte';
  import { applyTheme, settings } from '../lib/settings.svelte';
  import { load, save } from '../lib/storage';
  import BookmarksSection from './BookmarksSection.svelte';
  import DownloadsSection from './DownloadsSection.svelte';
  import HistorySection from './HistorySection.svelte';
  import ImportWizard from './ImportWizard.svelte';
  import PasswordsSection from './PasswordsSection.svelte';

  let { api }: { api: BrowserApi } = $props();

  // Labels: `library.nav.<id>`, looked up where shown (t() follows the interface language).
  const SECTIONS: { id: LibrarySection; icon: IconName }[] = [
    { id: 'bookmarks', icon: 'star' },
    { id: 'history', icon: 'clock' },
    { id: 'downloads', icon: 'download' },
    { id: 'passwords', icon: 'key' },
    { id: 'import', icon: 'import' },
  ];

  // The chrome asks for a section through localStorage (the URL has no query); otherwise the
  // page opens where the user left it, Bookmarks the first time.
  const first = takeRequest();
  let section = $state<LibrarySection>(first?.section ?? load<LibrarySection>('library.section', 'bookmarks'));
  let folder = $state<number>(first?.folder ?? BOOKMARKS_BAR);
  let status = $state<ConnectionStatus>('connecting');
  let toast = $state<{ text: string; error: boolean } | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | undefined;

  $effect(() => applyTheme(document.documentElement, settings.value));
  $effect(() => applyLang(document.documentElement));
  $effect(() => api.onStatus((s) => (status = s)));
  $effect(() =>
    onRequest(() => {
      const req = takeRequest();
      if (!req) return;
      section = req.section;
      if (req.folder !== undefined) folder = req.folder;
    }),
  );
  $effect(() => save('library.section', section));
  $effect(() => {
    document.title = t('library.pageTitle', { section: t(`library.nav.${section}`) });
  });

  function notify(text: string, error = false) {
    toast = { text, error };
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (toast = null), error ? 6000 : 3000);
  }
  const fail = (err: unknown) => notify(err instanceof Error ? err.message : String(err), true);
</script>

<div class="lib">
  <nav aria-label={t('library.title')}>
    <h1>{t('library.title')}</h1>
    {#each SECTIONS as s (s.id)}
      <button class="nav-item" aria-current={section === s.id ? 'page' : undefined} onclick={() => (section = s.id)}>
        <Icon name={s.icon} size={16} />
        <span>{t(`library.nav.${s.id}`)}</span>
      </button>
    {/each}
    {#if api.kind === 'ws' && status !== 'open'}
      <p class="conn" role="status">{status === 'connecting' ? t('library.connecting') : t('library.disconnected')}</p>
    {/if}
  </nav>

  <main>
    {#key section}
      {#if section === 'bookmarks'}
        <BookmarksSection {api} bind:folder {notify} {fail} />
      {:else if section === 'history'}
        <HistorySection {api} {notify} {fail} />
      {:else if section === 'downloads'}
        <DownloadsSection {api} {notify} {fail} />
      {:else if section === 'passwords'}
        <PasswordsSection {api} {notify} {fail} />
      {:else}
        <ImportWizard {api} onfinish={() => (section = 'bookmarks')} />
      {/if}
    {/key}
  </main>

  {#if toast}
    <div class="toast" class:error={toast.error} role={toast.error ? 'alert' : 'status'}>
      <span>{toast.text}</span>
      <button class="icon-btn" aria-label={t('library.dismiss')} onclick={() => (toast = null)}><Icon name="x" size={14} /></button>
    </div>
  {/if}
</div>

<style>
  .lib {
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    height: 100vh;
  }
  nav {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 18px 10px;
    border-right: 1px solid var(--border);
    background: var(--toolbar);
  }
  h1 {
    margin: 0 10px 14px;
    font-size: 18px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 34px;
    padding: 0 10px;
    border: 0;
    border-radius: var(--r);
    background: transparent;
    color: var(--fg-muted);
    font-weight: 550;
    text-align: left;
  }
  .nav-item:hover {
    background: var(--hover);
    color: var(--fg);
  }
  .nav-item[aria-current='page'] {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .conn {
    margin: auto 10px 0;
    color: var(--warn);
    font-size: var(--fs-sm);
  }
  main {
    min-width: 0;
    overflow: auto;
    background: var(--surface);
  }
  .toast {
    position: fixed;
    z-index: 50;
    left: 50%;
    bottom: 18px;
    display: flex;
    align-items: center;
    gap: 8px;
    max-width: min(560px, calc(100vw - 32px));
    padding: 6px 6px 6px 14px;
    border-radius: var(--r-lg);
    background: var(--fg);
    color: var(--toolbar);
    font-size: var(--fs-sm);
    box-shadow: var(--shadow-pop);
    transform: translateX(-50%);
  }
  .toast.error {
    background: var(--danger);
    color: var(--on-danger);
  }
  .toast .icon-btn {
    color: inherit;
  }
  @media (max-width: 720px) {
    .lib {
      grid-template-columns: minmax(0, 1fr);
      grid-template-rows: auto minmax(0, 1fr);
    }
    nav {
      flex-direction: row;
      flex-wrap: wrap;
      padding: 10px;
      border-right: 0;
      border-bottom: 1px solid var(--border);
    }
    h1 {
      display: none;
    }
    .nav-item span {
      display: none;
    }
  }
</style>
