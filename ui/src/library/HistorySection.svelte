<script lang="ts">
  import { untrack } from 'svelte';
  import type { BrowserApi } from '../lib/api';
  import type { HistoryDeleteArgs, HistoryEntry } from '../lib/api/types-data';
  import Favicon from '../lib/components/Favicon.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import { locale, t, tp } from '../lib/i18n/index.svelte';
  import { onDataChanged, onReconnect } from '../lib/data/library';
  import {
    CLEAR_RANGES,
    clearPlan,
    dayLabel,
    groupByDay,
    timeOfDay,
    type ClearRange,
    type DayGroup,
  } from '../lib/data/history';
  import { isSafeLinkUrl } from '../lib/safeurl';
  import { hostOf } from '../lib/urls';
  import Dialog from './Dialog.svelte';

  let { api, notify, fail }: { api: BrowserApi; notify: (text: string) => void; fail: (err: unknown) => void } = $props();

  const PAGE = 300;
  let query = $state('');
  let limit = $state(PAGE);
  let entries = $state<HistoryEntry[]>([]);
  let loaded = $state(false);
  let tick = $state(0);
  const groups = $derived(groupByDay(entries, Date.now()));

  $effect(() => {
    const q = query.trim();
    const n = limit;
    void tick;
    const timer = setTimeout(
      () => {
        api
          .call('history.search', { q: q || null, limit: n })
          .then((r) => {
            if (q !== query.trim()) return;
            entries = r.entries;
            loaded = true;
          })
          .catch(fail);
      },
      untrack(() => loaded) ? 180 : 0,
    );
    return () => clearTimeout(timer);
  });
  $effect(() => onReconnect(api, () => tick++));
  // Deletes and imports by anyone (an agent, another library tab) arrive as history.changed.
  $effect(() => onDataChanged(api, 'history.changed', () => tick++));

  const deletedText = (r: { deleted_visits: number; deleted_urls: number }) =>
    tp('library.hist.deleted', r.deleted_visits) + (r.deleted_urls ? `; ${tp('library.hist.pagesLeft', r.deleted_urls)}` : '');

  /** The day heading in the interface language (lib/data/history names only "Today" and
   * "Yesterday" itself, in English; other days are dates in the given locale). */
  function dayTitle(g: DayGroup): string {
    const label = dayLabel(g.start, Date.now(), locale());
    if (label === 'Today') return t('library.hist.today');
    if (label === 'Yesterday') return t('library.hist.yesterday');
    return label;
  }

  /** One `history.delete`, then re-read the list. */
  async function remove(args: HistoryDeleteArgs, done?: boolean) {
    try {
      const r = await api.call('history.delete', args);
      if (done) notify(deletedText(r));
    } catch (err) {
      fail(err);
    }
    tick++;
  }

  async function removeEntry(e: HistoryEntry) {
    entries = entries.filter((x) => x.url !== e.url);
    await remove({ url: e.url });
  }

  let dayToRemove = $state<DayGroup | null>(null);
  let dayOpen = $state(false);
  async function removeDay() {
    const g = dayToRemove;
    dayOpen = false;
    if (!g) return;
    // Only that day's visits; a page also visited on other days stays under those days.
    await remove({ day: g.key }, true);
  }

  let clearOpen = $state(false);
  let range = $state<ClearRange>('hour');
  let clearing = $state(false);
  const plan = $derived(clearPlan(range, Date.now()));
  async function clear() {
    clearing = true;
    await remove(clearPlan(range, Date.now()), true);
    clearing = false;
    clearOpen = false;
  }
</script>

<header class="head">
  <h2>{t('library.nav.history')}</h2>
  <label class="lib-search">
    <Icon name="search" size={14} />
    <input class="lib-input" type="search" placeholder={t('library.hist.search')} aria-label={t('library.hist.search')} bind:value={query} />
  </label>
  <div class="spacer"></div>
  <button class="btn" onclick={() => (clearOpen = true)}><Icon name="trash" size={14} /> {t('library.hist.clearOpen')}</button>
</header>

<div class="days">
  {#each groups as g (g.key)}
    <section class="day" aria-labelledby="day-{g.key}">
      <div class="day-head">
        <h3 id="day-{g.key}">{dayTitle(g)}</h3>
        <span class="n">{tp('library.hist.pages', g.entries.length)}</span>
        <button class="btn ghost" onclick={() => ((dayToRemove = g), (dayOpen = true))}>{t('library.hist.removeDay')}</button>
      </div>
      {#each g.entries as e (e.url)}
        <div class="lib-row">
          <time class="time" datetime={new Date(e.last_visit).toISOString()}>{timeOfDay(e.last_visit, locale())}</time>
          <Favicon url={e.url} size={16} />
          <div class="grow">
            {#if isSafeLinkUrl(e.url)}
              <a class="title" href={e.url} title={e.url}>{e.title || e.url}</a>
            {:else}
              <span class="title" title={e.url}>{e.title || e.url}</span>
            {/if}
            <span class="sub">{hostOf(e.url) || e.url}{e.visit_count > 1 ? ` · ${tp('library.hist.visits', e.visit_count)}` : ''}</span>
          </div>
          <div class="actions">
            <button class="icon-btn" aria-label={t('library.hist.removeEntry', { name: e.title || e.url })} title={t('library.hist.removeEntryHint')} onclick={() => removeEntry(e)}>
              <Icon name="x" size={15} />
            </button>
          </div>
        </div>
      {/each}
    </section>
  {:else}
    {#if loaded}
      <div class="lib-empty">
        <h3>{query.trim() ? t('library.hist.noMatch') : t('library.hist.none')}</h3>
        <p>{query.trim() ? t('library.hist.noMatchHelp') : t('library.hist.noneHelp')}</p>
      </div>
    {/if}
  {/each}
  {#if entries.length >= limit && limit < 5000}
    <div class="more"><button class="btn" onclick={() => (limit = Math.min(5000, limit + PAGE))}>{t('library.hist.more')}</button></div>
  {/if}
</div>

<Dialog bind:open={dayOpen} title={t('library.hist.removeDayTitle', { day: dayToRemove ? dayTitle(dayToRemove) : t('library.hist.thisDay') })} onsubmit={removeDay}>
  <p>{t('library.hist.removeDayText', { pages: tp('library.hist.pages', dayToRemove?.entries.length ?? 0) })}</p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (dayOpen = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn primary danger">{t('library.hist.remove')}</button>
  {/snippet}
</Dialog>

<Dialog bind:open={clearOpen} title={t('library.hist.clearTitle')} onsubmit={clear}>
  <label class="lib-field">
    {t('library.hist.range')}
    <select class="lib-input" bind:value={range}>
      {#each CLEAR_RANGES as r (r.id)}<option value={r.id}>{t(`library.hist.range.${r.id}`)}</option>{/each}
    </select>
  </label>
  <p class="hint">
    {#if 'before' in plan}
      {range === 'all' ? t('library.hist.clearAll') : t('library.hist.clearOlder')}
    {:else}
      {t('library.hist.clearRange')}
    {/if}
    {t('library.hist.untouched')}
  </p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (clearOpen = false)}>{t('common.cancel')}</button>
    <button type="submit" class="btn primary danger" disabled={clearing}>{clearing ? t('library.hist.clearing') : t('library.hist.clear')}</button>
  {/snippet}
</Dialog>

<style>
  .head {
    position: sticky;
    top: 0;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 16px 24px 12px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  h2 {
    margin: 0 8px 0 0;
    font-size: 18px;
    font-weight: 650;
  }
  .spacer {
    flex: 1;
  }
  .days {
    max-width: 920px;
    padding: 8px 18px 48px;
  }
  .day {
    margin-top: 14px;
  }
  .day-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 0 6px 4px 10px;
  }
  h3 {
    margin: 0;
    font-size: var(--fs);
    font-weight: 650;
  }
  .n {
    flex: 1;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
  }
  .ghost {
    height: 24px;
    border-color: transparent;
    background: transparent;
    color: var(--fg-muted);
  }
  .time {
    width: 62px;
    white-space: nowrap;
    color: var(--fg-subtle);
    font-size: var(--fs-sm);
    font-variant-numeric: tabular-nums;
    flex: none;
  }
  .more {
    display: flex;
    justify-content: center;
    padding: 18px;
  }
  .hint {
    margin: 0;
    font-size: var(--fs-sm);
  }
  @media (max-width: 720px) {
    .head {
      flex-wrap: wrap;
    }
  }
</style>
