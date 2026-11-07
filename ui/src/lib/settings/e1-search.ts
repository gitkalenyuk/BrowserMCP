// The address bar's search engines (Settings → Search). Same rules as crates/settings/src/search.rs
// (`search.url`): "<keyword> words" searches with that engine, anything else with the default
// engine; `%s` in the engine URL is replaced by the encoded words.

import { DEFAULT_SEARCH, resolveOmniboxInput } from '../urls';
import type { SearchConfig, SearchEngine } from './e1-types';

/** What the address bar needs: the default engine and every keyword. */
export interface OmniboxSearch {
  /** Default engine's URL with `%s`. */
  template: string;
  /** Default engine's name ("Google"). */
  name: string;
  engines: Pick<SearchEngine, 'id' | 'name' | 'url' | 'keyword'>[];
}

/** Before the browser answered (and in old cores without search.config): Google. */
export const FALLBACK_SEARCH: OmniboxSearch = {
  template: DEFAULT_SEARCH,
  name: 'Google',
  engines: [],
};

export function omniboxSearch(config: SearchConfig | null | undefined): OmniboxSearch {
  if (!config) return FALLBACK_SEARCH;
  const def = config.engines.find((e) => e.id === config.default) ?? config.engines[0];
  if (!def) return FALLBACK_SEARCH;
  return { template: def.url, name: def.name, engines: config.engines };
}

/** The engine whose keyword starts `input` ("w cats" → the `w` engine and "cats"). */
export function keywordSearch(input: string, search: OmniboxSearch): { engine: OmniboxSearch['engines'][number]; words: string } | null {
  const m = input.trim().match(/^(\S+)\s+(\S.*)$/s);
  if (!m) return null;
  const engine = search.engines.find((e) => e.keyword.toLowerCase() === m[1].toLowerCase());
  return engine ? { engine, words: m[2].trim() } : null;
}

/** Address bar input → URL: a keyword search, a typed address, or a default search. */
export function resolveWithEngines(raw: string, search: OmniboxSearch = FALLBACK_SEARCH): string | null {
  const kw = keywordSearch(raw, search);
  if (kw) return kw.engine.url.replace('%s', encodeURIComponent(kw.words));
  return resolveOmniboxInput(raw, search.template);
}

/** What Enter would do: open an address, or search with which engine (for the hint). */
export function intentWithEngines(raw: string, search: OmniboxSearch = FALLBACK_SEARCH): { kind: 'url' } | { kind: 'search'; engine: string } | null {
  if (!raw.trim()) return null;
  const kw = keywordSearch(raw, search);
  if (kw) return { kind: 'search', engine: kw.engine.name };
  const url = resolveOmniboxInput(raw, search.template);
  if (!url) return null;
  return url === search.template.replace('%s', encodeURIComponent(raw.trim())) ? { kind: 'search', engine: search.name } : { kind: 'url' };
}

// ---- the form in Settings → Search -----------------------------------------------------

export interface EngineDraft {
  name: string;
  url: string;
  keyword: string;
}

export type EngineErrors = Partial<Record<keyof EngineDraft, 'required' | 'tooLong' | 'scheme' | 'placeholder' | 'spaces' | 'taken'>>;

/** Client-side checks (the core checks the same and has the last word). `editing` = id of the engine being changed. */
export function engineErrors(draft: EngineDraft, engines: SearchEngine[], editing: string | null = null): EngineErrors {
  const errors: EngineErrors = {};
  const name = draft.name.trim();
  if (!name) errors.name = 'required';
  else if (name.length > 60) errors.name = 'tooLong';
  const url = draft.url.trim();
  if (!url) errors.url = 'required';
  else if (!/^https?:\/\//i.test(url)) errors.url = 'scheme';
  else if (/\s/.test(url)) errors.url = 'spaces';
  else if (!url.includes('%s')) errors.url = 'placeholder';
  const keyword = draft.keyword.trim().toLowerCase();
  if (keyword) {
    if (/\s|%/.test(keyword)) errors.keyword = 'spaces';
    else if (keyword.length > 20) errors.keyword = 'tooLong';
    else if (engines.some((e) => e.id !== editing && e.keyword.toLowerCase() === keyword)) errors.keyword = 'taken';
  }
  return errors;
}
