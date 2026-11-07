// The demo backend of the new settings sections behaves like crates/settings, and the chrome's
// BrowserPrefs follows `settings.changed`.
import { describe, expect, it } from 'vitest';
import { createDataMocks } from '../api/mock-data';
import { createE1Mocks } from '../api/mock-e1';
import type { BrowserApi } from '../api/client';
import { BrowserPrefs } from './e1-prefs.svelte';

type AnyHandlers = Record<string, (p?: unknown) => unknown>;

function setup() {
  const events: [string, unknown][] = [];
  const data = createDataMocks({ emit: () => {} }) as unknown as AnyHandlers;
  const e1 = createE1Mocks(
    (e, p) => events.push([e, p]),
    async (m, p) => data[m](p) as never,
  ) as unknown as AnyHandlers;
  return { e1, events };
}

describe('mock: search engines', () => {
  it('adds an engine, makes it the default and builds its URLs', async () => {
    const { e1, events } = setup();
    const added = (await e1['search.config']({ add: { name: 'Docs', url: 'https://docs.test/?q=%s', keyword: 'dx' } })) as { added: string };
    expect(added.added).toBe('custom-docs');
    await e1['search.config']({ default: 'custom-docs' });
    expect(await e1['search.url']({ query: 'a b' })).toEqual({ url: 'https://docs.test/?q=a%20b', engine: 'custom-docs' });
    expect(await e1['search.url']({ query: 'b hello' })).toMatchObject({ engine: 'bing' });
    expect(events.filter(([e]) => e === 'settings.changed')).toHaveLength(2);
  });

  it('refuses what the core refuses and keeps the old state', async () => {
    const { e1 } = setup();
    expect(() => e1['search.config']({ add: { name: 'G2', url: 'https://g.test/?q=%s', keyword: 'g' } })).toThrow(/keyword/);
    expect(() => e1['search.config']({ remove: 'google' })).toThrow(/built-in/);
    const out = (await e1['search.config']()) as { engines: unknown[]; default: string };
    expect(out.engines).toHaveLength(5);
    expect(out.default).toBe('google');
  });
});

describe('mock: startup and downloads', () => {
  it('validates pages and announces the change', async () => {
    const { e1, events } = setup();
    expect(() => e1['startup.config']({ on_startup: 'pages', pages: [] })).toThrow(/at least one page/);
    const out = (await e1['startup.config']({ on_startup: 'pages', pages: ['a.test'] })) as { config: { pages: string[] } };
    expect(out.config.pages).toEqual(['https://a.test/']);
    expect(events).toContainEqual(['settings.changed', { area: 'startup' }]);
  });

  it('download folder must be a full path', async () => {
    const { e1 } = setup();
    expect(() => e1['downloads.config']({ dir: 'relative' })).toThrow(/full path/);
    expect(await e1['downloads.config']({ dir: 'D:\\Dl', ask: true })).toMatchObject({ dir: 'D:\\Dl', custom: true, ask: true });
    expect(await e1['downloads.config']({ dir: '' })).toMatchObject({ custom: false });
  });

  it('clearing browsing data clears the mock history and downloads list', async () => {
    const { e1 } = setup();
    const out = (await e1['browsing_data.clear']({ history: true, downloads: true, cache: true })) as Record<string, unknown>;
    expect(out).toMatchObject({ cache: true, errors: [] });
    expect(typeof out.history_visits).toBe('number');
    expect(typeof out.downloads).toBe('number');
    await expect(Promise.resolve().then(() => e1['browsing_data.clear']({}))).rejects.toThrow(/at least one/);
  });
});

describe('BrowserPrefs (chrome)', () => {
  it('loads the engines and new tab page and follows settings.changed', async () => {
    const handlers = new Map<string, (p: unknown) => void>();
    let engine = 'google';
    const api = {
      call: async (method: string) => {
        if (method === 'search.config') {
          return {
            default: engine,
            engines: [
              { id: 'google', name: 'Google', url: 'https://g.test/?q=%s', keyword: 'g', builtin: true },
              { id: 'bing', name: 'Bing', url: 'https://b.test/?q=%s', keyword: 'b', builtin: true },
            ],
          };
        }
        return {
          config: {
            on_startup: 'restore',
            pages: [],
            home_page: 'https://home.test/',
            show_home_button: true,
            new_tab_url: null,
            new_tab_position: 'end',
            keep_window_on_last_tab: false,
          },
          new_tab_page: 'bmcp://ui/newtab.html',
        };
      },
      on: (event: string, h: (p: unknown) => void) => {
        handlers.set(event, h);
        return () => handlers.delete(event);
      },
      onStatus: (h: (s: string) => void) => {
        h('open');
        return () => {};
      },
    } as unknown as BrowserApi;

    const prefs = new BrowserPrefs();
    const stop = prefs.start(api);
    await new Promise((r) => setTimeout(r, 0));
    expect(prefs.search.name).toBe('Google');
    expect(prefs.showHome).toBe(true);
    expect(prefs.homeUrl).toBe('https://home.test/');
    expect(prefs.newTabUrl).toBe('bmcp://ui/newtab.html');

    engine = 'bing';
    handlers.get('settings.changed')!({ area: 'search' });
    await new Promise((r) => setTimeout(r, 0));
    expect(prefs.search.name).toBe('Bing');
    stop();
    expect(handlers.has('settings.changed')).toBe(false);
  });
});
