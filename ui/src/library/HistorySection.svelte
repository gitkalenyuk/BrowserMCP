<script lang="ts">
  import { untrack } from 'svelte';
  import type { BrowserApi } from '../lib/api';
  import type { HistoryEntry } from '../lib/api/types-data';
  import Favicon from '../lib/components/Favicon.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import { bumpDataRev, onReconnect } from '../lib/data/library';
  import {
    CLEAR_RANGES,
    clearPlan,
    eachLimited,
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
    const t = setTimeout(
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
    return () => clearTimeout(t);
  });
  $effect(() => onReconnect(api, () => tick++));

  async function removeUrls(urls: string[]): Promise<number> {
    const res = await eachLimited(urls, 6, (url) => api.call('history.delete', { url }));
    const failed = res.filter((r) => r.status === 'rejected');
    if (failed.length) fail((failed[0] as PromiseRejectedResult).reason);
    return res.length - failed.length;
  }

  async function removeEntry(e: HistoryEntry) {
    entries = entries.filter((x) => x.url !== e.url);
    await removeUrls([e.url]);
    bumpDataRev();
    tick++;
  }

  let dayToRemove = $state<DayGroup | null>(null);
  let dayOpen = $state(false);
  async function removeDay() {
    const g = dayToRemove;
    dayOpen = false;
    if (!g) return;
    const n = await removeUrls(g.entries.map((e) => e.url));
    notify(`Removed ${n} page${n === 1 ? '' : 's'} from history`);
    bumpDataRev();
    tick++;
  }

  let clearOpen = $state(false);
  let range = $state<ClearRange>('hour');
  let clearing = $state(false);
  const plan = $derived(clearPlan(range, Date.now()));
  async function clear() {
    clearing = true;
    try {
      const p = clearPlan(range, Date.now());
      if ('before' in p) {
        const r = await api.call('history.delete', { before: p.before });
        notify(`Deleted ${r.deleted_visits} visit${r.deleted_visits === 1 ? '' : 's'} of ${r.deleted_urls} page${r.deleted_urls === 1 ? '' : 's'}`);
      } else {
        const { entries: recent } = await api.call('history.search', { since: p.since, limit: 5000 });
        const n = await removeUrls(recent.map((e) => e.url));
        notify(`Removed ${n} page${n === 1 ? '' : 's'} from history`);
      }
    } catch (err) {
      fail(err);
    } finally {
      clearing = false;
      clearOpen = false;
      bumpDataRev();
      tick++;
    }
  }
</script>

<header class="head">
  <h2>History</h2>
  <label class="lib-search">
    <Icon name="search" size={14} />
    <input class="lib-input" type="search" placeholder="Search history" aria-label="Search history" bind:value={query} />
  </label>
  <div class="spacer"></div>
  <button class="btn" onclick={() => (clearOpen = true)}><Icon name="trash" size={14} /> Clear browsing history…</button>
</header>

<div class="days">
  {#each groups as g (g.key)}
    <section class="day" aria-labelledby="day-{g.key}">
      <div class="day-head">
        <h3 id="day-{g.key}">{g.label}</h3>
        <span class="n">{g.entries.length} page{g.entries.length === 1 ? '' : 's'}</span>
        <button class="btn ghost" onclick={() => ((dayToRemove = g), (dayOpen = true))}>Remove day</button>
      </div>
      {#each g.entries as e (e.url)}
        <div class="lib-row">
          <time class="time" datetime={new Date(e.last_visit).toISOString()}>{timeOfDay(e.last_visit)}</time>
          <Favicon url={e.url} size={16} />
          <div class="grow">
            {#if isSafeLinkUrl(e.url)}
              <a class="title" href={e.url} title={e.url}>{e.title || e.url}</a>
            {:else}
              <span class="title" title={e.url}>{e.title || e.url}</span>
            {/if}
            <span class="sub">{hostOf(e.url) || e.url}{e.visit_count > 1 ? ` · ${e.visit_count} visits` : ''}</span>
          </div>
          <div class="actions">
            <button class="icon-btn" aria-label="Remove {e.title || e.url} from history" title="Remove from history" onclick={() => removeEntry(e)}>
              <Icon name="x" size={15} />
            </button>
          </div>
        </div>
      {/each}
    </section>
  {:else}
    {#if loaded}
      <div class="lib-empty">
        <h3>{query.trim() ? 'Nothing in history matches' : 'No history yet'}</h3>
        <p>{query.trim() ? 'Try a different word or part of the address.' : 'Pages you visit show up here, grouped by day.'}</p>
      </div>
    {/if}
  {/each}
  {#if entries.length >= limit && limit < 5000}
    <div class="more"><button class="btn" onclick={() => (limit = Math.min(5000, limit + PAGE))}>Show more</button></div>
  {/if}
</div>

<Dialog bind:open={dayOpen} title="Remove {dayToRemove?.label ?? 'this day'}?" onsubmit={removeDay}>
  <p>
    {dayToRemove?.entries.length} page{dayToRemove?.entries.length === 1 ? '' : 's'} will be removed from history, each with all
    of its visits (also those on other days).
  </p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (dayOpen = false)}>Cancel</button>
    <button type="submit" class="btn primary danger">Remove</button>
  {/snippet}
</Dialog>

<Dialog bind:open={clearOpen} title="Clear browsing history" onsubmit={clear}>
  <label class="lib-field">
    Time range
    <select class="lib-input" bind:value={range}>
      {#each CLEAR_RANGES as r (r.id)}<option value={r.id}>{r.label}</option>{/each}
    </select>
  </label>
  <p class="hint">
    {#if 'before' in plan}
      {range === 'all' ? 'Every visit is deleted.' : 'Visits older than four weeks are deleted; newer ones stay.'}
    {:else}
      Pages visited in this range are removed from history, each with all of its visits.
    {/if}
    Bookmarks, passwords and cookies are not touched.
  </p>
  {#snippet actions()}
    <button type="button" class="btn" onclick={() => (clearOpen = false)}>Cancel</button>
    <button type="submit" class="btn primary danger" disabled={clearing}>{clearing ? 'Clearing…' : 'Clear history'}</button>
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
