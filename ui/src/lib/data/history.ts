// History helpers for the library page: grouping by local day and the "clear" ranges.

import type { HistoryEntry } from '../api/types-data';
import { t } from '../i18n/index.svelte';
import { intlLocale } from '../i18n/intl';

export interface DayGroup {
  /** Local date, `YYYY-MM-DD`. */
  key: string;
  /** "Today", "Yesterday" or "Tuesday, November 3, 2026". */
  label: string;
  /** Local midnight that starts the day (Unix ms). */
  start: number;
  entries: HistoryEntry[];
}

/** Local midnight of the day `ts` falls in. */
export function startOfDay(ts: number): number {
  const d = new Date(ts);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

function dayKey(ts: number): string {
  const d = new Date(ts);
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** Previous local midnight (handles 23/25-hour DST days). */
function previousDay(start: number): number {
  const d = new Date(start);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() - 1).getTime();
}

export function dayLabel(start: number, now: number, locale = intlLocale()): string {
  const today = startOfDay(now);
  if (start === today) return t('components.day.today');
  if (start === previousDay(today)) return t('components.day.yesterday');
  return new Date(start).toLocaleDateString(locale, {
    weekday: 'long',
    month: 'long',
    day: 'numeric',
    year: 'numeric',
  });
}

/**
 * Group entries by the local day of their last visit, newest day first; inside a day the
 * newest visit comes first. (The core returns one row per URL with its newest visit.)
 */
export function groupByDay(entries: readonly HistoryEntry[], now: number, locale = intlLocale()): DayGroup[] {
  const groups = new Map<string, DayGroup>();
  for (const e of entries) {
    const key = dayKey(e.last_visit);
    let g = groups.get(key);
    if (!g) {
      const start = startOfDay(e.last_visit);
      g = { key, label: dayLabel(start, now, locale), start, entries: [] };
      groups.set(key, g);
    }
    g.entries.push(e);
  }
  const list = [...groups.values()].sort((a, b) => b.start - a.start);
  for (const g of list) g.entries.sort((a, b) => b.last_visit - a.last_visit);
  return list;
}

/** "14:05" style local time. */
export function timeOfDay(ts: number, locale = intlLocale()): string {
  return new Date(ts).toLocaleTimeString(locale, { hour: 'numeric', minute: '2-digit' });
}

export type ClearRange = 'hour' | 'day' | 'week' | 'month' | 'older_month' | 'all';

/** The ranges in "Clear history"; `label` is a getter in the UI language. */
export const CLEAR_RANGES: { readonly id: ClearRange; readonly label: string }[] = (
  ['hour', 'day', 'week', 'month', 'older_month', 'all'] as const
).map((id) => ({
  id,
  get label() {
    return t(`components.clear.${id}`);
  },
}));

const HOUR = 3_600_000;

/**
 * The `history.delete` arguments that clear a range: the visits *since* a time ("last hour" …
 * "last 4 weeks") or *before* one ("older than 4 weeks", "all time"). Only the visits in the
 * range go; a page keeps its visits outside it.
 */
export function clearPlan(range: ClearRange, now: number): { before: number } | { since: number } {
  switch (range) {
    case 'hour':
      return { since: now - HOUR };
    case 'day':
      return { since: now - 24 * HOUR };
    case 'week':
      return { since: now - 7 * 24 * HOUR };
    case 'month':
      return { since: now - 28 * 24 * HOUR };
    case 'older_month':
      return { before: now - 28 * 24 * HOUR };
    case 'all':
      return { before: now + 1 };
  }
}
