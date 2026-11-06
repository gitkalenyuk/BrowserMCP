// Fakes for the data commands (crates/data): a bookmark tree, history, downloads, passwords and
// the Chrome import, used by MockApi so `pnpm dev` demos the library page, the bookmarks bar
// and the import wizard without a backend. Semantics follow the Rust store (positions shared by
// folders and bookmarks, move index without the moved node, cascade remove, http/https/file URLs
// only, history kept as visits and deleted by visit, password reveal, `bookmarks.changed` /
// `history.changed` after edits); it is plausible, not a spec.

import { RpcError } from './client';
import type { ApiEvents, EventName, Params, Result } from './types';
import {
  BOOKMARKS_BAR,
  OTHER_BOOKMARKS,
  type DataBookmark,
  type DataFolder,
  type DataRpcMethods,
  type Download,
  type HistoryEntry,
  type ImportPartResult,
  type PasswordEntry,
} from './types-data';

type DataMethod = keyof DataRpcMethods;
type DataEmit = <E extends EventName>(event: E, payload: ApiEvents[E]) => void;
export type DataHandlers = { [M in DataMethod]: (params: Params<M>) => Result<M> | Promise<Result<M>> };

interface Node {
  id: number;
  kind: 'folder' | 'url';
  title: string;
  url: string | null;
  parent: number | null;
  created: number;
}

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

/** Like the core: an absolute http, https or file URL (trimmed), else INVALID_PARAMS. */
function checkUrl(url: string): string {
  const u = url.trim();
  let protocol: string;
  try {
    protocol = new URL(u).protocol;
  } catch {
    throw new RpcError('INVALID_PARAMS', `invalid url ${JSON.stringify(u)}`);
  }
  if (!['http:', 'https:', 'file:'].includes(protocol)) {
    throw new RpcError('INVALID_PARAMS', `only http, https and file URLs can be bookmarked, not ${protocol}`);
  }
  return u;
}

/** `[start, end)` of the local day `YYYY-MM-DD`. */
function localDay(day: string): [number, number] {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(day.trim());
  if (!m) throw new RpcError('INVALID_PARAMS', `\`day\` must be YYYY-MM-DD, got ${JSON.stringify(day)}`);
  const [y, mo, d] = [Number(m[1]), Number(m[2]) - 1, Number(m[3])];
  return [new Date(y, mo, d).getTime(), new Date(y, mo, d + 1).getTime()];
}

function originOf(input: string): string {
  try {
    const u = new URL(input.includes('://') ? input : `https://${input}`);
    return u.origin;
  } catch {
    throw new RpcError('INVALID_PARAMS', `invalid origin ${JSON.stringify(input)}`);
  }
}

export interface DataMockOptions {
  /** Start with an empty database (first run: the import wizard is offered). */
  empty?: boolean;
  now?: () => number;
  /** Delivers `bookmarks.changed` / `history.changed` (MockApi passes its emitter). */
  emit?: DataEmit;
}

export function createDataMocks(opts: DataMockOptions = {}): DataHandlers {
  const now = opts.now ?? Date.now;
  const emit: DataEmit = opts.emit ?? (() => {});
  const t0 = now();
  const nodes = new Map<number, Node>();
  const order = new Map<number, number[]>();
  let nextId = 3;
  /** url -> title and visit times (like the core's `urls` + `visits`). */
  const history = new Map<string, { title: string; visits: number[] }>();
  const toEntry = (url: string, h: { title: string; visits: number[] }): HistoryEntry => ({
    url,
    title: h.title,
    visit_count: h.visits.length,
    last_visit: Math.max(...h.visits),
  });
  let passwords: PasswordEntry[] = [];
  let nextPassword = 1;
  let downloads: Download[] = [];

  for (const [id, title] of [
    [BOOKMARKS_BAR, 'Bookmarks bar'],
    [OTHER_BOOKMARKS, 'Other bookmarks'],
  ] as const) {
    nodes.set(id, { id, kind: 'folder', title, url: null, parent: null, created: t0 - 30 * DAY });
    order.set(id, []);
  }

  const insert = (parent: number, kind: Node['kind'], title: string, url: string | null, created = now()): Node => {
    const node: Node = { id: nextId++, kind, title, url, parent, created };
    nodes.set(node.id, node);
    if (kind === 'folder') order.set(node.id, []);
    order.get(parent)!.push(node.id);
    return node;
  };

  const folderOf = (id: number): Node => {
    const n = nodes.get(id);
    if (!n) throw new RpcError('NOT_FOUND', `bookmark folder ${id} not found`);
    if (n.kind !== 'folder') throw new RpcError('INVALID_PARAMS', `bookmark ${id} is not a folder`);
    return n;
  };
  const position = (n: Node): number => (n.parent === null ? 0 : order.get(n.parent)!.indexOf(n.id));
  const toBookmark = (n: Node): DataBookmark => ({
    id: n.id,
    title: n.title,
    url: n.url ?? '',
    folder: n.parent ?? 0,
    position: position(n),
    created: n.created,
  });
  const toFolder = (n: Node): DataFolder => ({
    id: n.id,
    title: n.title,
    parent: n.parent,
    position: position(n),
    created: n.created,
    children: order.get(n.id)?.length ?? 0,
  });

  if (!opts.empty) {
    const bar = BOOKMARKS_BAR;
    insert(bar, 'url', 'GitHub', 'https://github.com/', t0 - 20 * DAY);
    insert(bar, 'url', 'MDN Web Docs', 'https://developer.mozilla.org/', t0 - 19 * DAY);
    const rust = insert(bar, 'folder', 'Rust', null, t0 - 18 * DAY);
    insert(rust.id, 'url', 'docs.rs', 'https://docs.rs/', t0 - 18 * DAY);
    insert(rust.id, 'url', 'crates.io: Rust Package Registry', 'https://crates.io/', t0 - 17 * DAY);
    insert(rust.id, 'url', 'The Rust Programming Language', 'https://doc.rust-lang.org/book/', t0 - 16 * DAY);
    const cef = insert(rust.id, 'folder', 'CEF', null, t0 - 15 * DAY);
    insert(cef.id, 'url', 'CEF forum', 'https://magpcss.org/ceforum/', t0 - 15 * DAY);
    insert(bar, 'url', 'Svelte docs', 'https://svelte.dev/docs', t0 - 14 * DAY);
    const work = insert(bar, 'folder', 'Work', null, t0 - 13 * DAY);
    insert(work.id, 'url', 'Linear', 'https://linear.app/', t0 - 13 * DAY);
    insert(work.id, 'url', 'BrowserMCP pull requests', 'https://github.com/browsermcp/browsermcp/pulls', t0 - 12 * DAY);
    insert(bar, 'url', 'Hacker News', 'https://news.ycombinator.com/', t0 - 10 * DAY);
    const other = OTHER_BOOKMARKS;
    const reading = insert(other, 'folder', 'Reading list', null, t0 - 9 * DAY);
    insert(reading.id, 'url', 'Local-first software', 'https://www.inkandswitch.com/local-first/', t0 - 9 * DAY);
    insert(reading.id, 'url', 'Designing Data-Intensive Applications', 'https://dataintensive.net/', t0 - 8 * DAY);
    insert(other, 'url', 'Wikipedia', 'https://en.wikipedia.org/', t0 - 7 * DAY);

    const visits: [string, string, number, number][] = [
      ['https://github.com/browsermcp/browsermcp', 'browsermcp/browsermcp: the browser for agents', 14, 12 * MIN],
      ['https://developer.mozilla.org/en-US/docs/Web/API/WebSocket', 'WebSocket - Web APIs | MDN', 6, 40 * MIN],
      ['https://svelte.dev/docs/svelte/$state', '$state • Svelte Docs', 9, 2 * HOUR],
      ['https://news.ycombinator.com/', 'Hacker News', 31, 3 * HOUR],
      ['https://docs.rs/rusqlite/latest/rusqlite/', 'rusqlite - Rust', 4, 26 * HOUR],
      ['https://crates.io/crates/serde', 'serde - crates.io: Rust Package Registry', 3, 27 * HOUR],
      ['https://www.sqlite.org/wal.html', 'Write-Ahead Logging', 1, 30 * HOUR],
      ['https://en.wikipedia.org/wiki/Frecency', 'Frecency - Wikipedia', 2, 2 * DAY + 3 * HOUR],
      ['https://magpcss.org/ceforum/viewtopic.php?t=19000', 'CEF Forum • Alloy runtime and Views', 1, 2 * DAY + 5 * HOUR],
      ['https://linear.app/browsermcp/issue/BMCP-42', 'BMCP-42 Bookmarks bar · Linear', 5, 3 * DAY + 1 * HOUR],
      ['https://www.youtube.com/watch?v=dQw4w9WgXcQ', 'A video - YouTube', 1, 4 * DAY + 2 * HOUR],
      ['https://github.com/', 'GitHub', 40, 5 * DAY],
    ];
    // The newest visit `ago` ago, the older ones every 7 hours before it.
    for (const [url, title, count, ago] of visits) {
      history.set(url, { title, visits: Array.from({ length: count }, (_, i) => t0 - ago - i * 7 * HOUR) });
    }

    passwords = [
      { id: nextPassword++, origin: 'https://github.com', username: 'octocat', password: 'hunter2-gh', note: '', created: t0 - 40 * DAY, updated: t0 - 3 * DAY },
      { id: nextPassword++, origin: 'https://accounts.example.com', username: 'me@example.com', password: 'correct horse battery staple', note: 'work SSO', created: t0 - 20 * DAY, updated: t0 - 20 * DAY },
      { id: nextPassword++, origin: 'http://localhost:3000', username: 'admin', password: 'admin', note: 'local dev', created: t0 - 2 * DAY, updated: t0 - 2 * DAY },
    ];

    downloads = [
      { id: 3, url: 'https://github.com/browsermcp/browsermcp/releases/download/v0.1.0/browsermcp-0.1.0-x64.msi', path: 'C:\\Users\\you\\Downloads\\browsermcp-0.1.0-x64.msi', mime: 'application/x-msi', received_bytes: 18_400_000, total_bytes: 52_000_000, state: 'in_progress', started: t0 - 2 * MIN, ended: null },
      { id: 2, url: 'https://example.com/report-q3.pdf', path: 'C:\\Users\\you\\Downloads\\report-q3.pdf', mime: 'application/pdf', received_bytes: 1_204_224, total_bytes: 1_204_224, state: 'complete', started: t0 - 26 * HOUR, ended: t0 - 26 * HOUR + 4000 },
      { id: 1, url: 'https://example.com/big.iso', path: 'C:\\Users\\you\\Downloads\\big.iso', mime: 'application/octet-stream', received_bytes: 3_000_000, total_bytes: 4_700_000_000, state: 'cancelled', started: t0 - 3 * DAY, ended: t0 - 3 * DAY + 60_000 },
    ];
  }

  const removeNode = (id: number): number => {
    const n = nodes.get(id)!;
    let removed = 1;
    for (const child of order.get(id) ?? []) removed += removeNode(child);
    order.delete(id);
    nodes.delete(id);
    if (n.parent !== null) {
      const sibs = order.get(n.parent);
      if (sibs) order.set(n.parent, sibs.filter((x) => x !== id));
    }
    return removed;
  };

  const matches = (q: string | null | undefined, ...fields: string[]) => {
    const needle = (q ?? '').trim().toLowerCase();
    return !needle || fields.some((f) => f.toLowerCase().includes(needle));
  };

  return {
    'data.bookmarks': ({ folder, limit }) => {
      const id = folder ?? BOOKMARKS_BAR;
      folderOf(id);
      const kids = order.get(id)!.map((k) => nodes.get(k)!);
      return {
        folder: id,
        bookmarks: kids.filter((n) => n.kind === 'url').slice(0, limit ?? 500).map(toBookmark),
        folders: kids.filter((n) => n.kind === 'folder').map(toFolder),
      };
    },
    'data.top_sites': ({ limit }) => ({
      sites: [...history]
        .map(([url, h]) => toEntry(url, h))
        .map((e) => ({ ...e, score: e.visit_count * 100 - (t0 - e.last_visit) / HOUR }))
        .sort((a, b) => b.score - a.score)
        .slice(0, limit ?? 8),
    }),
    'bookmarks.add': ({ folder, title, url }) => {
      const parent = folder ?? BOOKMARKS_BAR;
      folderOf(parent);
      const u = checkUrl(url);
      const t = title?.trim() || history.get(u)?.title || u;
      const node = insert(parent, 'url', t, u);
      emit('bookmarks.changed', { op: 'add', id: node.id });
      return toBookmark(node);
    },
    'bookmarks.add_folder': ({ parent, title }) => {
      const p = parent ?? BOOKMARKS_BAR;
      folderOf(p);
      if (!title.trim()) throw new RpcError('INVALID_PARAMS', 'folder title must not be empty');
      const node = insert(p, 'folder', title.trim(), null);
      emit('bookmarks.changed', { op: 'add_folder', id: node.id });
      return toFolder(node);
    },
    'bookmarks.update': ({ id, title, url }) => {
      if (title == null && url == null) throw new RpcError('INVALID_PARAMS', 'give `title` and/or `url`');
      const n = nodes.get(id);
      if (!n) throw new RpcError('NOT_FOUND', `bookmark ${id} not found`);
      if (n.kind === 'folder') {
        if (n.parent === null) throw new RpcError('INVALID_PARAMS', `folder ${id} is a root folder and cannot be renamed`);
        if (url != null) throw new RpcError('INVALID_PARAMS', `bookmark ${id} is a folder and has no url`);
        if (!title?.trim()) throw new RpcError('INVALID_PARAMS', 'folder title must not be empty');
        n.title = title.trim();
      } else {
        if (url != null) n.url = checkUrl(url);
        if (title != null) n.title = title.trim() || n.url!;
      }
      emit('bookmarks.changed', { op: 'update', id });
      return n.kind === 'folder' ? { bookmark: null, folder: toFolder(n) } : { bookmark: toBookmark(n), folder: null };
    },
    'bookmarks.remove': ({ id }) => {
      if (id === BOOKMARKS_BAR || id === OTHER_BOOKMARKS) {
        throw new RpcError('INVALID_PARAMS', `folder ${id} is a root folder and cannot be removed`);
      }
      if (!nodes.has(id)) throw new RpcError('NOT_FOUND', `bookmark ${id} not found`);
      const removed = removeNode(id);
      emit('bookmarks.changed', { op: 'remove', id });
      return { removed };
    },
    'bookmarks.move': ({ id, folder, position: pos }) => {
      if (id === BOOKMARKS_BAR || id === OTHER_BOOKMARKS) {
        throw new RpcError('INVALID_PARAMS', `folder ${id} is a root folder and cannot be moved`);
      }
      const n = nodes.get(id);
      if (!n) throw new RpcError('NOT_FOUND', `bookmark ${id} not found`);
      const target = folder ?? n.parent!;
      folderOf(target);
      for (let c: number | null = target; c !== null; c = nodes.get(c)?.parent ?? null) {
        if (c === id) throw new RpcError('INVALID_PARAMS', `cannot move folder ${id} into itself or one of its sub-folders`);
      }
      order.set(n.parent!, order.get(n.parent!)!.filter((x) => x !== id));
      const sibs = order.get(target)!;
      const index = Math.min(pos ?? sibs.length, sibs.length);
      sibs.splice(index, 0, id);
      n.parent = target;
      emit('bookmarks.changed', { op: 'move', id });
      return { id, folder: target, position: index };
    },
    'bookmarks.search': ({ q, limit }) => ({
      bookmarks: [...nodes.values()]
        .filter((n) => n.kind === 'url' && matches(q, n.title, n.url ?? ''))
        .sort((a, b) => b.created - a.created || b.id - a.id)
        .slice(0, limit ?? 100)
        .map(toBookmark),
    }),
    'history.search': ({ q, since, limit }) => ({
      entries: [...history]
        .map(([url, h]) => toEntry(url, h))
        .filter((e) => e.last_visit >= (since ?? 0) && matches(q, e.url, e.title))
        .sort((a, b) => b.last_visit - a.last_visit)
        .slice(0, Math.min(limit ?? 100, 5000)),
    }),
    'history.delete': ({ url, since, before, day }) => {
      if (url == null && since == null && before == null && day == null) {
        throw new RpcError('INVALID_PARAMS', 'give `url`, `since`, `before` and/or `day`');
      }
      if (day != null && (since != null || before != null)) {
        throw new RpcError('INVALID_PARAMS', '`day` cannot be combined with `since`/`before`');
      }
      const [from, to] = day != null ? localDay(day) : [since ?? -Infinity, before ?? Infinity];
      if (from >= to) throw new RpcError('INVALID_PARAMS', '`since` must be before `before`');
      let urls = 0;
      let visits = 0;
      for (const [u, h] of history) {
        if (url != null && u !== url) continue;
        const kept = h.visits.filter((t) => t < from || t >= to);
        visits += h.visits.length - kept.length;
        h.visits = kept;
        if (!kept.length) {
          history.delete(u);
          urls++;
        }
      }
      if (visits + urls) emit('history.changed', { op: 'delete' });
      return { deleted_urls: urls, deleted_visits: visits };
    },
    'downloads.list': ({ limit }) => ({ downloads: downloads.slice(0, limit ?? 100) }),
    'passwords.list': ({ origin, reveal }) => {
      const only = origin ? originOf(origin) : null;
      return {
        entries: passwords
          .filter((p) => !only || p.origin === only)
          .map((p) => ({ ...p, password: reveal ? p.password : null })),
      };
    },
    'passwords.get': ({ origin }) => {
      const only = originOf(origin);
      return { entries: passwords.filter((p) => p.origin === only) };
    },
    'passwords.save': ({ origin, username, password, note }) => {
      const o = originOf(origin);
      const existing = passwords.find((p) => p.origin === o && p.username === username);
      if (existing) {
        Object.assign(existing, { password, note: note ?? existing.note, updated: now() });
        return { id: existing.id, origin: o, created: false };
      }
      const entry: PasswordEntry = { id: nextPassword++, origin: o, username, password, note: note ?? '', created: now(), updated: now() };
      passwords = [...passwords, entry];
      return { id: entry.id, origin: o, created: true };
    },
    'passwords.delete': ({ id }) => {
      const before = passwords.length;
      passwords = passwords.filter((p) => p.id !== id);
      if (passwords.length === before) throw new RpcError('NOT_FOUND', `password ${id} not found`);
      return { deleted: true };
    },
    'import.chrome_detect': () => ({
      user_data_dirs: ['C:\\Users\\you\\AppData\\Local\\Google\\Chrome\\User Data'],
      profiles: [
        { name: 'Default', path: 'C:\\Users\\you\\AppData\\Local\\Google\\Chrome\\User Data\\Default', has_bookmarks: true, has_history: true },
        { name: 'Profile 1', path: 'C:\\Users\\you\\AppData\\Local\\Google\\Chrome\\User Data\\Profile 1', has_bookmarks: true, has_history: false },
      ],
    }),
    'import.chrome': async (args) => {
      if (!args.parts.length) throw new RpcError('INVALID_PARAMS', 'give at least one part');
      if (args.parts.includes('passwords_csv') && !args.passwords_csv_path) {
        throw new RpcError('INVALID_PARAMS', '`passwords_csv` needs `passwords_csv_path`');
      }
      if (args.parts.includes('cookies_json') && !args.cookies_json_path) {
        throw new RpcError('INVALID_PARAMS', '`cookies_json` needs `cookies_json_path`');
      }
      await new Promise((r) => setTimeout(r, 900));
      const profile = args.profile_dir ?? 'C:\\Users\\you\\AppData\\Local\\Google\\Chrome\\User Data\\Default';
      const results: ImportPartResult[] = args.parts.map((part) => {
        switch (part) {
          case 'bookmarks': {
            const folder = insert(BOOKMARKS_BAR, 'folder', 'Imported from Chrome', null);
            insert(folder.id, 'url', 'Gmail', 'https://mail.google.com/');
            insert(folder.id, 'url', 'Google Drive', 'https://drive.google.com/');
            emit('bookmarks.changed', { op: 'import', id: null });
            return { part, ok: true, imported: 2, skipped: 0, source: `${profile}\\Bookmarks`, error: null };
          }
          case 'history':
            history.set('https://www.google.com/', { title: 'Google', visits: [now() - 6 * DAY, now() - 9 * DAY] });
            emit('history.changed', { op: 'import' });
            return { part, ok: true, imported: 1, skipped: 0, source: `${profile}\\History`, error: null };
          case 'passwords_csv': {
            const path = args.passwords_csv_path!;
            if (!/\.csv$/i.test(path)) {
              return { part, ok: false, imported: 0, skipped: 0, source: path, error: `not found: ${path}` };
            }
            passwords = [...passwords, { id: nextPassword++, origin: 'https://mail.google.com', username: 'you@gmail.com', password: 'imported', note: '', created: now(), updated: now() }];
            return { part, ok: true, imported: 1, skipped: 1, source: path, error: null };
          }
          case 'cookies_json':
            return { part, ok: true, imported: 37, skipped: 2, source: args.cookies_json_path!, error: null };
        }
      });
      return { results };
    },
    // Nothing to open in the browser-less demo: answer like the core would.
    'system.reveal_path': ({ path }) => {
      console.info('[mock] system.reveal_path', path);
      return { path };
    },
    'system.open_path': ({ path }) => {
      console.info('[mock] system.open_path', path);
      return { path };
    },
  };
}
