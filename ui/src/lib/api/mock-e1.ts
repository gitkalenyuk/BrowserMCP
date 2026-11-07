// Fakes for Settings → Startup & tabs, Search, Downloads, Site settings, Data & storage and
// About (startup.config, search.*, downloads.config, permissions.list/set, browsing_data.*,
// system.about), used by MockApi so `pnpm dev` and the tests show every section without a
// browser. The rules mirror crates/settings (built-in engines, `%s`, unique keywords, URL
// checks); the core is the truth. tabs.closed* and zoom.* live in mock.ts (they need its
// tabs), downloads.clear in mock-data.ts (its downloads list).

import { RpcError } from './client';
import type { ApiEvents, EventName, Method, Params, Result } from './types';
import { engineErrors } from '../settings/e1-search';
import { normalizePage } from '../settings/e1-startup';
import type { E1Methods, PermissionRule, SearchEngine, StartupConfig } from '../settings/e1-types';

type Emit = <E extends EventName>(event: E, payload: ApiEvents[E]) => void;
type Call = <M extends Method>(method: M, params: Params<M>) => Promise<Result<M>>;

type E1Mocked = Exclude<keyof E1Methods, 'tabs.closed' | 'tabs.closed_reopen' | 'tabs.closed_clear' | 'zoom.list' | 'zoom.reset' | 'downloads.clear'>;
export type E1Handlers = { [M in E1Mocked]: (params: Params<M>) => Result<M> | Promise<Result<M>> };

const ROOT = 'C:\\BrowserMCP';
const DATA = `${ROOT}\\data`;

const BUILTIN: SearchEngine[] = [
  { id: 'google', name: 'Google', url: 'https://www.google.com/search?q=%s', keyword: '@google', builtin: true },
  { id: 'bing', name: 'Bing', url: 'https://www.bing.com/search?q=%s', keyword: '@bing', builtin: true },
  { id: 'duckduckgo', name: 'DuckDuckGo', url: 'https://duckduckgo.com/?q=%s', keyword: '@ddg', builtin: true },
  { id: 'brave', name: 'Brave Search', url: 'https://search.brave.com/search?q=%s', keyword: '@brave', builtin: true },
  { id: 'startpage', name: 'Startpage', url: 'https://www.startpage.com/do/search?q=%s', keyword: '@startpage', builtin: true },
];

const DEFAULT_STARTUP: StartupConfig = {
  on_startup: 'restore',
  pages: [],
  home_page: null,
  show_home_button: false,
  new_tab_url: null,
  new_tab_position: 'end',
  keep_window_on_last_tab: false,
};

const slug = (name: string) =>
  name
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, '-')
    .replace(/^-+|-+$/g, '') || 'engine';

function page(text: string): string {
  const url = normalizePage(text);
  if (!url) throw new RpcError('INVALID_PARAMS', `${JSON.stringify(text)} cannot be used as a page URL`);
  return url;
}

export function createE1Mocks(emit: Emit, call: Call): E1Handlers {
  const started = Date.now();
  let startup: StartupConfig = structuredClone(DEFAULT_STARTUP);
  let search = { default: 'google', custom: [] as SearchEngine[] };
  let downloads = { dir: null as string | null, ask: false };
  let permissions: PermissionRule[] = [
    { origin: 'https://meet.google.com', kind: 'camera', allow: true },
    { origin: 'https://meet.google.com', kind: 'microphone', allow: true },
    { origin: 'https://maps.example.com', kind: 'geolocation', allow: false },
    { origin: 'https://news.example.com', kind: 'notifications', allow: false },
  ];
  // Sizes shrink when data is cleared, so the page shows a change.
  const usage = new Map<string, number>([
    ['profiles/default', 412_000_000],
    ['profiles/work', 96_500_000],
    ['db', 18_300_000],
    ['downloads', 230_000_000],
    ['recordings', 61_000_000],
    ['logs', 4_200_000],
    ['config', 48_000],
  ]);

  const engines = () => [...BUILTIN, ...search.custom];
  const searchOut = (added?: string) => {
    const def = engines().some((e) => e.id === search.default) ? search.default : 'google';
    return { default: def, engines: engines(), ...(added ? { added } : {}) };
  };
  const checked = (input: { name: string; url: string; keyword?: string | null }, editing: string | null) => {
    const keyword = (input.keyword ?? '').trim().toLowerCase() || slug(input.name);
    const errors = engineErrors({ name: input.name, url: input.url, keyword }, engines(), editing);
    const [field, problem] = Object.entries(errors)[0] ?? [];
    if (field) throw new RpcError('INVALID_PARAMS', `${field}: ${problem}`);
    return { name: input.name.trim(), url: input.url.trim(), keyword };
  };
  const custom = (id: string) => {
    if (BUILTIN.some((e) => e.id === id)) throw new RpcError('INVALID_PARAMS', `${id} is a built-in engine and cannot be changed or removed`);
    const e = search.custom.find((x) => x.id === id);
    if (!e) throw new RpcError('NOT_FOUND', `no search engine "${id}"`);
    return e;
  };
  const downloadsOut = () => ({
    dir: downloads.dir ?? `${DATA}\\downloads`,
    default_dir: `${DATA}\\downloads`,
    custom: downloads.dir !== null,
    ask: downloads.ask,
  });

  return {
    'startup.config': (args) => {
      if (!args || Object.keys(args).length === 0) return { config: startup, new_tab_page: startup.new_tab_url ?? 'bmcp://ui/newtab.html' };
      const next: StartupConfig = args.reset ? structuredClone(DEFAULT_STARTUP) : { ...startup };
      if (args.on_startup) next.on_startup = args.on_startup;
      if (args.pages) next.pages = args.pages.filter((p) => p.trim()).map(page);
      if (args.home_page != null) next.home_page = args.home_page.trim() ? page(args.home_page) : null;
      if (args.show_home_button != null) next.show_home_button = args.show_home_button;
      if (args.new_tab_url != null) {
        const url = args.new_tab_url.trim() ? page(args.new_tab_url) : null;
        next.new_tab_url = url === 'bmcp://ui/newtab.html' ? null : url;
      }
      if (args.new_tab_position) next.new_tab_position = args.new_tab_position;
      if (args.keep_window_on_last_tab != null) next.keep_window_on_last_tab = args.keep_window_on_last_tab;
      if (next.on_startup === 'pages' && next.pages.length === 0) {
        throw new RpcError('INVALID_PARAMS', 'on_startup "pages" needs at least one page in `pages`');
      }
      startup = next;
      emit('settings.changed', { area: 'startup' });
      return { config: startup, new_tab_page: startup.new_tab_url ?? 'bmcp://ui/newtab.html' };
    },
    'search.config': (args) => {
      if (!args || Object.keys(args).length === 0) return searchOut();
      const before = structuredClone(search);
      try {
        if (args.remove) {
          custom(args.remove);
          search.custom = search.custom.filter((e) => e.id !== args.remove);
          if (search.default === args.remove) search.default = 'google';
        }
        if (args.update) {
          const e = custom(args.update.id);
          Object.assign(e, checked(args.update, e.id));
        }
        let added: string | undefined;
        if (args.add) {
          const input = checked(args.add, null);
          const base = `custom-${slug(input.name)}`;
          let id = base;
          for (let n = 2; engines().some((e) => e.id === id); n++) id = `${base}-${n}`;
          search.custom.push({ id, ...input, builtin: false });
          added = id;
        }
        if (args.default) {
          if (!engines().some((e) => e.id === args.default)) throw new RpcError('NOT_FOUND', `no search engine "${args.default}"`);
          search.default = args.default;
        }
        emit('settings.changed', { area: 'search', default: search.default });
        return searchOut(added);
      } catch (err) {
        search = before;
        throw err;
      }
    },
    'search.url': ({ query, engine }) => {
      const q = query.trim();
      if (!q) throw new RpcError('INVALID_PARAMS', '`query` must not be empty');
      const m = q.match(/^(\S+)\s+(\S.*)$/s);
      const byKeyword = m ? engines().find((e) => e.keyword === m[1].toLowerCase()) : undefined;
      const e = engine ? engines().find((x) => x.id === engine) : (byKeyword ?? engines().find((x) => x.id === search.default) ?? BUILTIN[0]);
      if (!e) throw new RpcError('NOT_FOUND', `no search engine "${engine}"`);
      const words = !engine && byKeyword && m ? m[2] : q;
      return { url: e.url.replaceAll('%s', encodeURIComponent(words)), engine: e.id };
    },
    'downloads.config': (args) => {
      if (args?.dir != null) {
        const dir = args.dir.trim();
        if (dir && !/^[a-z]:\\/i.test(dir)) {
          throw new RpcError('INVALID_PARAMS', `the download folder must be a full path like C:\\Users\\you\\Downloads, got "${dir}"`);
        }
        // Like check_dir in crates/settings (simplified): no drive roots or system folders.
        if (dir && /^[a-z]:\\*$|^[a-z]:\\(windows|program files|programdata)(\\|$)|\\appdata\\roaming(\\|$)/i.test(dir)) {
          throw new RpcError('INVALID_PARAMS', `${dir} is a system folder: pick a folder of your own`);
        }
        downloads.dir = dir || null;
      }
      if (args?.ask != null) downloads.ask = args.ask;
      if (args && (args.dir != null || args.ask != null)) emit('settings.changed', { area: 'downloads' });
      return downloadsOut();
    },
    'permissions.list': () => ({ rules: permissions }),
    'permissions.set': ({ origin, kind, allow }) => {
      const o = origin.trim() === '*' ? '*' : origin.trim().toLowerCase().replace(/\/+$/, '');
      if (!o) throw new RpcError('INVALID_PARAMS', 'origin must not be empty');
      permissions = permissions.filter((r) => !(r.origin === o && r.kind === kind));
      if (allow !== null && allow !== undefined) permissions = [...permissions, { origin: o, kind, allow }];
      return { rules: permissions };
    },
    'browsing_data.clear': async (args) => {
      const parts = (['history', 'downloads', 'cookies', 'cache', 'site_data'] as const).filter((p) => args[p]);
      if (!parts.length) throw new RpcError('INVALID_PARAMS', 'choose at least one of history, downloads, cookies, cache, site_data');
      await new Promise((r) => setTimeout(r, 400));
      const profile = args.profile ?? 'default';
      const out: Result<'browsing_data.clear'> = { profile, cache: false, errors: [] };
      if (args.history) {
        const r = await call('history.delete', { since: args.since ?? 0 });
        out.history_visits = r.deleted_visits;
      }
      if (args.downloads) out.downloads = (await call('downloads.clear', { since: args.since })).removed;
      if (args.cookies) out.cookies = args.since ? 3 : 41;
      if (args.cache) out.cache = true;
      if (args.site_data) out.site_data_origins = args.since ? 2 : 17;
      const key = `profiles/${profile}`;
      if ((args.cache || args.site_data || args.cookies) && usage.has(key)) usage.set(key, Math.round((usage.get(key) ?? 0) * 0.35));
      return out;
    },
    'browsing_data.usage': () => {
      const areas = [...usage]
        .map(([id, bytes]) => ({ id, path: `${DATA}\\${id.replace('/', '\\')}`, bytes, files: Math.max(1, Math.round(bytes / 40_000)) }))
        .sort((a, b) => b.bytes - a.bytes);
      return { root: DATA, total_bytes: areas.reduce((n, a) => n + a.bytes, 0), areas };
    },
    'system.about': () => ({
      name: 'BrowserMCP',
      version: '0.1.0',
      cef_version: '156.0.15+gcf671c0+chromium-156.0.8078.80',
      chromium_version: '156.0.8078.80',
      os: 'windows x86_64',
      pid: 4242,
      uptime_secs: Math.round((Date.now() - started) / 1000) + 3 * 3600 + 12 * 60,
      exe_dir: ROOT,
      data_dir: DATA,
      config_dir: `${DATA}\\config`,
      logs_dir: `${DATA}\\logs`,
      api: {
        port: 47800,
        mcp: 'http://127.0.0.1:47800/mcp',
        rest: 'http://127.0.0.1:47800/v1',
        ws: 'ws://127.0.0.1:47800/v1/ws',
      },
      licenses: [{ name: 'CREDITS.html', path: `${ROOT}\\CREDITS.html` }],
    }),
  };
}
