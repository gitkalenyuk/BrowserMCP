// Settings → General → Language: website languages (Accept-Language / navigator.languages),
// Chromium's own locale and spellcheck (crates/languages, `browser.languages_get|set`). Pure
// helpers for the list editor, unit-tested; the card applies every change at once.
//
// Precedence (same rule as crates/languages `accept_language_for`): a profile with a language
// of its own (its stealth identity, fitted to the proxy's country, or profile.create
// {accept_language}) keeps it; the list edited here applies to every other profile.

/** Languages offered in the "Add a language" box (any other BCP 47 code can be typed). */
export const COMMON_LANGUAGES = [
  'uk', 'en', 'en-US', 'en-GB', 'de', 'fr', 'es', 'it', 'pl', 'pt', 'pt-BR', 'nl', 'cs', 'sk', 'ro', 'hu', 'bg', 'el', 'tr',
  'sv', 'da', 'nb', 'fi', 'et', 'lv', 'lt', 'hr', 'sr', 'sl', 'be', 'ka', 'hy', 'az', 'kk', 'he', 'ar', 'fa', 'hi', 'ja', 'ko',
  'zh-CN', 'zh-TW', 'vi', 'th', 'id',
] as const;

/**
 * A BCP 47 tag in its usual spelling (`EN-us` → `en-US`, `zh-hant` → `zh-Hant`, `uk_UA` →
 * `uk-UA`), or null. Same rules as crates/languages `normalize_tag`.
 */
export function normalizeTag(input: string): string | null {
  const parts = input.trim().replace(/_/g, '-').split('-');
  const [language, ...rest] = parts;
  if (!/^[a-zA-Z]{2,3}$/.test(language)) return null;
  const out = [language.toLowerCase()];
  for (const part of rest) {
    if (!/^[a-zA-Z0-9]{2,8}$/.test(part)) return null;
    if (part.length === 2) out.push(part.toUpperCase());
    else if (part.length === 3 && /^\d+$/.test(part)) out.push(part);
    else if (part.length === 4) out.push(part[0].toUpperCase() + part.slice(1).toLowerCase());
    else out.push(part.toLowerCase());
  }
  return out.join('-');
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** The language's name in that language ("uk" → "Українська"), or the tag itself. */
export function nativeName(tag: string): string {
  try {
    const name = new Intl.DisplayNames([tag], { type: 'language' }).of(tag);
    if (name && !same(name, tag)) return name.charAt(0).toLocaleUpperCase(tag) + name.slice(1);
  } catch {
    // not a tag Intl knows
  }
  return tag;
}

/** The language's name in the interface language ("uk" in English → "Ukrainian"). */
export function localName(tag: string, uiLocale: string): string {
  try {
    const name = new Intl.DisplayNames([uiLocale], { type: 'language' }).of(tag);
    if (name && !same(name, tag)) return name;
  } catch {
    // not a tag Intl knows
  }
  return tag;
}

/**
 * What the user typed in the add box → a tag: a code ("de", "pt-br") or a name from the list
 * ("Deutsch", "German"). null when it is neither.
 */
export function resolveLanguage(input: string, uiLocale: string): string | null {
  const text = input.trim();
  if (!text) return null;
  const named = COMMON_LANGUAGES.find((tag) => same(nativeName(tag), text) || same(localName(tag, uiLocale), text));
  return named ?? normalizeTag(text);
}

/** Add `tag` at the end (no duplicates in any case). */
export function addLanguage(list: string[], tag: string): string[] {
  return list.some((l) => same(l, tag)) ? list : [...list, tag];
}

/** Remove the language at `index`; the last one stays (websites need at least one). */
export function removeLanguage(list: string[], index: number): string[] {
  if (list.length <= 1) return list;
  return list.filter((_, i) => i !== index);
}

/** Move the language at `index` up (-1) or down (+1). */
export function moveLanguage(list: string[], index: number, by: -1 | 1): string[] {
  const to = index + by;
  if (index < 0 || index >= list.length || to < 0 || to >= list.length) return list;
  const next = [...list];
  [next[index], next[to]] = [next[to], next[index]];
  return next;
}

/** Switch spellcheck for `tag` on or off (order kept as in `web`, extras at the end). */
export function toggleSpellcheck(spellcheck: string[], tag: string, web: string[]): string[] {
  const on = spellcheck.some((l) => same(l, tag));
  const next = on ? spellcheck.filter((l) => !same(l, tag)) : [...spellcheck, tag];
  const rank = (l: string) => {
    const i = web.findIndex((w) => same(w, l));
    return i < 0 ? web.length : i;
  };
  return [...next].sort((a, b) => rank(a) - rank(b));
}

/**
 * Spellcheck dictionaries that are on but have no row in the website list (exact tags: Chromium
 * keeps "en-US" and "en" apart). Listed separately in the card so they can still be turned off.
 */
export function spellExtras(spellcheck: string[], web: string[]): string[] {
  return spellcheck.filter((l) => !web.some((w) => same(w, l)));
}

/** The automatic list for an interface language (mirror of crates/languages `automatic_web`). */
export function automaticWeb(uiLanguage: string): string[] {
  const tag = normalizeTag(uiLanguage) ?? 'en';
  const list = [tag === 'en' ? 'en-US' : tag];
  for (const extra of [tag.split('-')[0], 'en']) if (!list.some((l) => same(l, extra))) list.push(extra);
  return list;
}

/** Languages that can still be added (not in the list yet), for the add box. */
export function addable(list: string[]): string[] {
  return COMMON_LANGUAGES.filter((tag) => !list.some((l) => same(l, tag)));
}
