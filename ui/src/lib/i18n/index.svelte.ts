/**
 * UI translations: `t('area.key', { name })` → the string in the chosen UI language.
 *
 * * Language: `settings.value.language` (`system` = the browser's `navigator.languages`),
 *   shared by every page of the UI through localStorage, so all pages switch at once.
 * * Dictionaries: one file per UI area in `en/` and `uk/` (`en/ai.ts`, `uk/ai.ts`…), each a
 *   `default` export of flat `key: string` pairs. Files are picked up automatically
 *   (`import.meta.glob`): adding an area never touches a shared file. English is the source
 *   of truth; a key missing in Ukrainian falls back to English (the i18n test fails on it).
 * * Keys: `<area>.<what>`, e.g. `ai.send`, `settings.nav.general`. Placeholders: `{name}`.
 * * Plurals: `tp('tabs.count', n)` reads `tabs.count.one|few|many|other` (Intl.PluralRules).
 *
 * `t()` reads reactive state, so it re-renders when the language changes: call it in markup
 * or in `$derived`, never once at module level (a constant would keep the first language).
 */
import { settings } from '../settings.svelte';

export type Locale = 'en' | 'uk';
export type LanguagePref = 'system' | Locale;
export type Dict = Record<string, string>;

/** Languages offered in Settings → General, in their own names. */
export const LOCALES: { id: Locale; name: string }[] = [
  { id: 'en', name: 'English' },
  { id: 'uk', name: 'Українська' },
];

function collect(files: Record<string, { default: Dict }>): Dict {
  const out: Dict = {};
  for (const file of Object.values(files)) Object.assign(out, file.default);
  return out;
}

export const DICTS: Record<Locale, Dict> = {
  en: collect(import.meta.glob<{ default: Dict }>('./en/*.ts', { eager: true })),
  uk: collect(import.meta.glob<{ default: Dict }>('./uk/*.ts', { eager: true })),
};

/** `uk` when the browser prefers Ukrainian, else `en`. */
export function systemLocale(languages: readonly string[] = typeof navigator === 'undefined' ? [] : navigator.languages): Locale {
  for (const lang of languages) {
    const base = lang.toLowerCase().split('-')[0];
    if (base === 'uk') return 'uk';
    if (base === 'en') return 'en';
  }
  return 'en';
}

export function resolveLocale(pref: LanguagePref | undefined): Locale {
  return pref === 'en' || pref === 'uk' ? pref : systemLocale();
}

/** The UI language in effect now (reactive). */
export function locale(): Locale {
  return resolveLocale(settings.value.language);
}

export function format(text: string, params?: Record<string, string | number>): string {
  if (!params) return text;
  return text.replace(/\{(\w+)\}/g, (whole, name: string) => (name in params ? String(params[name]) : whole));
}

/** Translate `key` (falls back to English, then to the key itself). */
export function t(key: string, params?: Record<string, string | number>): string {
  return format(DICTS[locale()][key] ?? DICTS.en[key] ?? key, params);
}

/** Plural form of `key` for `n` (`{n}` is filled in): `key.one`, `key.few`, `key.many`, `key.other`. */
export function tp(key: string, n: number, params?: Record<string, string | number>): string {
  const loc = locale();
  const dict = DICTS[loc];
  const text =
    dict[`${key}.${new Intl.PluralRules(loc).select(n)}`] ??
    dict[`${key}.other`] ??
    DICTS.en[`${key}.${new Intl.PluralRules('en').select(n)}`] ??
    DICTS.en[`${key}.other`] ??
    key;
  return format(text, { n, ...params });
}

/** Keep `<html lang>` in sync with the UI language. Call inside an `$effect` on each page. */
export function applyLang(root: HTMLElement): void {
  root.lang = locale();
}
