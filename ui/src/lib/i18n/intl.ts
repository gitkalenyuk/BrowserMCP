/**
 * Dates, times, lists and "5 min ago" in the UI language, built on `Intl` so the grammar
 * (Ukrainian cases and plurals, English commas) comes from the platform, not from us.
 *
 * Like `t()`, these read the reactive language setting: call them in markup or `$derived`.
 */
import { locale, type Locale } from './index.svelte';

const FALLBACK: Record<Locale, string> = { en: 'en-US', uk: 'uk-UA' };

/**
 * BCP 47 tag for `Intl` in the UI language. Keeps the system's region when it speaks the same
 * language (English UI on an en-GB system → `en-GB` dates), else `en-US` / `uk-UA`.
 */
export function intlLocale(
  loc: Locale = locale(),
  languages: readonly string[] = typeof navigator === 'undefined' ? [] : navigator.languages,
): string {
  return languages.find((l) => l.toLowerCase().split('-')[0] === loc && l.includes('-')) ?? FALLBACK[loc];
}

/** "a, b and c" / "a, b і c". */
export function formatList(parts: readonly string[], type: 'conjunction' | 'disjunction' = 'conjunction'): string {
  return new Intl.ListFormat(intlLocale(), { type }).format(parts);
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ['second', 60],
  ['minute', 60],
  ['hour', 24],
  ['day', 7],
  ['week', 4.35],
  ['month', 12],
  ['year', Infinity],
];

/**
 * `ms` relative to `now`: "just now", "5 min. ago", "in 2 hr.", "yesterday" ("щойно",
 * "5 хв тому", "через 2 год", "учора"). Under 10 seconds counts as now.
 */
export function relativeTime(ms: number, now: number = Date.now(), style: 'long' | 'short' = 'short'): string {
  const fmt = new Intl.RelativeTimeFormat(intlLocale(), { numeric: 'auto', style });
  let value = (ms - now) / 1000;
  if (Math.abs(value) < 10) return fmt.format(0, 'second');
  for (const [unit, size] of UNITS) {
    if (Math.abs(value) < size) return fmt.format(Math.round(value), unit);
    value /= size;
  }
  return fmt.format(Math.round(value), 'year');
}

/** "14:05" (or "2:05 PM" where that is the local habit). */
export function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(intlLocale(), { hour: '2-digit', minute: '2-digit' });
}

/** "14:05" for today, else "7 Nov, 14:05" / "7 лист., 14:05" (list rows: short and enough). */
export function formatWhen(ms: number, now: number = Date.now()): string {
  const d = new Date(ms);
  if (d.toDateString() === new Date(now).toDateString()) return formatTime(ms);
  return d.toLocaleString(intlLocale(), { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
}

/** Date and time, e.g. "7 Nov 2026, 14:05" / "7 лист. 2026 р., 14:05". */
export function formatDateTime(ms: number, opts: Intl.DateTimeFormatOptions = { dateStyle: 'medium', timeStyle: 'short' }): string {
  return new Date(ms).toLocaleString(intlLocale(), opts);
}

/** A date with custom fields, in the UI language. */
export function formatDate(ms: number | Date, opts: Intl.DateTimeFormatOptions): string {
  return (ms instanceof Date ? ms : new Date(ms)).toLocaleDateString(intlLocale(), opts);
}
