import { describe, expect, it } from 'vitest';
import { engineErrors, FALLBACK_SEARCH, intentWithEngines, keywordSearch, omniboxSearch, resolveWithEngines } from './e1-search';
import type { SearchConfig, SearchEngine } from './e1-types';

const engines: SearchEngine[] = [
  { id: 'google', name: 'Google', url: 'https://www.google.com/search?q=%s', keyword: 'g', builtin: true },
  { id: 'duckduckgo', name: 'DuckDuckGo', url: 'https://duckduckgo.com/?q=%s', keyword: 'd', builtin: true },
  { id: 'custom-wiki', name: 'Wiki', url: 'https://uk.wikipedia.org/w/index.php?search=%s', keyword: 'w', builtin: false },
];
const config: SearchConfig = { default: 'duckduckgo', engines };

describe('address bar search engines', () => {
  it('uses the default engine for words and passes addresses through', () => {
    const s = omniboxSearch(config);
    expect(s.name).toBe('DuckDuckGo');
    expect(resolveWithEngines('rust lang', s)).toBe('https://duckduckgo.com/?q=rust%20lang');
    expect(resolveWithEngines('example.com', s)).toBe('https://example.com');
    expect(resolveWithEngines('   ', s)).toBeNull();
  });

  it('a keyword, a space and words search with that engine (same rule as search.url)', () => {
    const s = omniboxSearch(config);
    expect(keywordSearch('W Київ', s)?.engine.id).toBe('custom-wiki');
    expect(resolveWithEngines('w Київ', s)).toBe('https://uk.wikipedia.org/w/index.php?search=%D0%9A%D0%B8%D1%97%D0%B2');
    // The keyword alone is an ordinary search for that word.
    expect(keywordSearch('w', s)).toBeNull();
    expect(resolveWithEngines('w', s)).toBe('https://duckduckgo.com/?q=w');
    // An unknown first word is part of the search.
    expect(resolveWithEngines('x cats', s)).toBe('https://duckduckgo.com/?q=x%20cats');
  });

  it('says what Enter would do, with the engine name', () => {
    const s = omniboxSearch(config);
    expect(intentWithEngines('cats', s)).toEqual({ kind: 'search', engine: 'DuckDuckGo' });
    expect(intentWithEngines('g cats', s)).toEqual({ kind: 'search', engine: 'Google' });
    expect(intentWithEngines('https://a.test', s)).toEqual({ kind: 'url' });
    expect(intentWithEngines('', s)).toBeNull();
  });

  it('falls back to Google before the browser answered', () => {
    expect(omniboxSearch(null)).toBe(FALLBACK_SEARCH);
    expect(resolveWithEngines('cats')).toBe('https://www.google.com/search?q=cats');
    // A default that is not in the list: the first engine.
    expect(omniboxSearch({ default: 'gone', engines }).name).toBe('Google');
  });
});

describe('engine form', () => {
  const draft = { name: 'Docs', url: 'https://docs.test/?q=%s', keyword: 'dx' };

  it('accepts a good engine', () => {
    expect(engineErrors(draft, engines)).toEqual({});
    expect(engineErrors({ ...draft, keyword: '' }, engines)).toEqual({});
  });

  it('flags each problem like the core', () => {
    expect(engineErrors({ ...draft, name: ' ' }, engines).name).toBe('required');
    expect(engineErrors({ ...draft, name: 'x'.repeat(61) }, engines).name).toBe('tooLong');
    expect(engineErrors({ ...draft, url: '' }, engines).url).toBe('required');
    expect(engineErrors({ ...draft, url: 'javascript:%s' }, engines).url).toBe('scheme');
    expect(engineErrors({ ...draft, url: 'https://docs.test/?q=' }, engines).url).toBe('placeholder');
    expect(engineErrors({ ...draft, url: 'https://docs.test/?q=%s x' }, engines).url).toBe('spaces');
    expect(engineErrors({ ...draft, keyword: 'a b' }, engines).keyword).toBe('spaces');
    expect(engineErrors({ ...draft, keyword: 'D' }, engines).keyword).toBe('taken');
    // Editing an engine may keep its own keyword.
    expect(engineErrors({ ...draft, keyword: 'w' }, engines, 'custom-wiki')).toEqual({});
  });
});
