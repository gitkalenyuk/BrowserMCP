<script lang="ts">
  // The AI panel's History drawer: closed (archived) chats with a search box. Open brings
  // one back to the tab strip; Delete (asks first) and "Delete all closed" remove them for good.
  import { onMount } from 'svelte';
  import type { AiSession } from '../../api';
  import { filterHistory, lastActive } from '../../ai/sessions';
  import { formatWhen } from '../../i18n/intl';
  import { t, tp } from '../../i18n/index.svelte';
  import { useChrome } from '../../state/chrome.svelte';
  import Icon from '../Icon.svelte';

  let {
    onclose,
    onreopen,
    ondelete,
    ondeleteall,
  }: {
    onclose: () => void;
    onreopen: (s: AiSession) => void;
    ondelete: (s: AiSession) => void;
    ondeleteall: () => void;
  } = $props();

  const ai = useChrome().ai;

  let query = $state('');
  let search = $state<HTMLInputElement>();
  /** The row whose Delete was clicked: it asks before deleting. */
  let confirming = $state<string | null>(null);
  let confirmAll = $state(false);

  const shown = $derived(filterHistory(ai.history, query, (id) => ai.providerName(id)));

  let root = $state<HTMLElement>();

  onMount(() => void ai.loadHistory());

  $effect(() => {
    search?.focus();
  });

  // Esc cancels a confirmation, else closes the drawer.
  $effect(() => {
    const el = root;
    if (!el) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.stopPropagation();
      if (confirming || confirmAll) {
        confirming = null;
        confirmAll = false;
      } else onclose();
    };
    el.addEventListener('keydown', onKey);
    return () => el.removeEventListener('keydown', onKey);
  });

  function meta(s: AiSession): string {
    const when = formatWhen(lastActive(s));
    const provider = ai.providerName(s.provider);
    return s.model ? t('aiChat.history.metaModel', { provider, model: s.model, when }) : t('aiChat.history.meta', { provider, when });
  }
</script>

<section class="history" aria-label={t('aiChat.history.title')} bind:this={root}>
  <header>
    <Icon name="clock" size={14} />
    <h3>{t('aiChat.history.title')}</h3>
    <span class="spacer"></span>
    <button class="icon-btn" aria-label={t('aiChat.history.close')} title={t('aiChat.history.close')} onclick={onclose}>
      <Icon name="x" size={14} />
    </button>
  </header>
  <label class="search">
    <Icon name="search" size={13} />
    <span class="sr-only">{t('aiChat.history.search')}</span>
    <input bind:this={search} bind:value={query} type="search" placeholder={t('aiChat.history.search')} spellcheck="false" />
  </label>

  {#if ai.historyError}
    <p class="note err" role="alert">{t('aiChat.history.loadFailed', { message: ai.historyError })}</p>
  {/if}

  <ul class="list" aria-label={t('aiChat.history.title')}>
    {#each shown as s (s.id)}
      <li class="item" class:confirming={confirming === s.id}>
        <span class="dot" style:background={s.color} aria-hidden="true"></span>
        <div class="text">
          <span class="name" title={ai.title(s)}>{ai.title(s)}</span>
          <span class="meta">{meta(s)}</span>
        </div>
        {#if confirming === s.id}
          <div class="ask" role="alertdialog" aria-label={t('aiChat.deleteConfirm', { name: ai.title(s) })}>
            <span>{t('aiChat.deleteConfirm', { name: ai.title(s) })}</span>
            <button class="btn danger" onclick={() => ((confirming = null), ondelete(s))}>{t('aiChat.deleteConfirmYes')}</button>
            <button class="btn" onclick={() => (confirming = null)}>{t('common.cancel')}</button>
          </div>
        {:else}
          <button class="btn" title={t('aiChat.history.reopenTitle')} onclick={() => onreopen(s)}>{t('aiChat.history.reopen')}</button>
          <button
            class="icon-btn del"
            aria-label={`${t('aiChat.history.delete')}: ${ai.title(s)}`}
            title={t('aiChat.history.delete')}
            onclick={() => (confirming = s.id)}
          >
            <Icon name="trash" size={13} />
          </button>
        {/if}
      </li>
    {:else}
      {#if ai.historyLoaded && !ai.historyError}
        <li class="note">{query.trim() ? t('aiChat.history.noMatch', { query: query.trim() }) : t('aiChat.history.empty')}</li>
      {/if}
    {/each}
  </ul>

  {#if ai.history.length > 0}
    <footer>
      {#if confirmAll}
        <div class="ask" role="alertdialog" aria-label={tp('aiChat.history.deleteAllConfirm', ai.history.length)}>
          <span>{tp('aiChat.history.deleteAllConfirm', ai.history.length)}</span>
          <button class="btn danger" onclick={() => ((confirmAll = false), ondeleteall())}>{t('aiChat.deleteConfirmYes')}</button>
          <button class="btn" onclick={() => (confirmAll = false)}>{t('common.cancel')}</button>
        </div>
      {:else}
        <button class="btn danger" onclick={() => (confirmAll = true)}>
          <Icon name="trash" size={12} />
          {t('aiChat.history.deleteAll')}
        </button>
      {/if}
    </footer>
  {/if}
</section>

<style>
  .history {
    position: absolute;
    inset: 0;
    z-index: 5;
    display: flex;
    flex-direction: column;
    background: var(--toolbar);
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 44px;
    padding: 6px 6px 6px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--fg-muted);
  }
  h3 {
    margin: 0;
    color: var(--fg);
    font-size: var(--fs);
    font-weight: 650;
  }
  .spacer {
    flex: 1;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 10px 10px 6px;
    padding: 0 9px;
    height: 30px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r);
    background: var(--surface);
    color: var(--fg-subtle);
  }
  .search:focus-within {
    border-color: var(--accent-line);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .search input {
    flex: 1;
    min-width: 0;
    height: 100%;
    border: 0;
    background: transparent;
    color: var(--fg);
    font-size: var(--fs-sm);
    outline: none;
    user-select: text;
  }
  .list {
    flex: 1;
    min-height: 0;
    margin: 0;
    padding: 4px 6px 8px;
    overflow-y: auto;
    list-style: none;
  }
  .item {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    padding: 7px 6px 7px 8px;
    border-radius: var(--r);
  }
  .item:hover,
  .item:focus-within {
    background: var(--hover);
  }
  .item.confirming {
    background: color-mix(in oklab, var(--danger) 8%, transparent);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex: none;
  }
  .text {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }
  .name {
    overflow: hidden;
    font-size: var(--fs-sm);
    font-weight: 600;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .meta {
    overflow: hidden;
    color: var(--fg-subtle);
    font-size: var(--fs-xs);
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .btn {
    height: 24px;
    padding: 0 9px;
    font-size: var(--fs-xs);
  }
  .btn.danger {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    border-color: color-mix(in oklab, var(--danger) 40%, transparent);
    color: var(--danger);
  }
  .del {
    width: 24px;
    height: 24px;
    color: var(--fg-subtle);
  }
  .del:hover {
    color: var(--danger);
  }
  .ask {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    flex-basis: 100%;
    font-size: var(--fs-xs);
  }
  .ask span {
    flex: 1 1 160px;
  }
  .note {
    padding: 18px 12px;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
    line-height: 1.5;
    text-align: center;
  }
  .note.err {
    margin: 0 10px;
    padding: 6px 9px;
    border-radius: var(--r);
    background: color-mix(in oklab, var(--danger) 12%, transparent);
    color: var(--danger);
    font-size: var(--fs-xs);
    text-align: left;
  }
  footer {
    padding: 8px 10px 10px;
    border-top: 1px solid var(--border);
  }
</style>
