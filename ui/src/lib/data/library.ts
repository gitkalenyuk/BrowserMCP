// Plumbing between the chrome and the library page (bmcp://ui/library.html).
//
// The library URL carries no query or fragment (the browser lets content tabs load exactly
// `bmcp://ui/library.html`). Which section to show travels through localStorage, which the
// chrome and the library share (same origin): the chrome writes a request, then opens or
// activates the library tab; the page reads it on load, or on the `storage` event when it is
// already open, and clears it.
//
// Data changes come from the core itself: every edit (by the library, the star, an agent over
// REST/MCP, a script) emits `bookmarks.changed` / `history.changed`, and the bar, the star and
// the library re-read on them (`onDataChanged`).

import type { BrowserApi } from '../api/client';
import type { ConnectionStatus } from '../api/types';
import type { DataApiEvents } from '../api/types-data';
import { load, save, watch } from '../storage';

export const LIBRARY_URL = 'bmcp://ui/library.html';
export const LIBRARY_SECTIONS = ['bookmarks', 'history', 'downloads', 'passwords', 'import'] as const;
export type LibrarySection = (typeof LIBRARY_SECTIONS)[number];

export interface LibraryRequest {
  section: LibrarySection;
  /** Bookmarks section: the folder to open. */
  folder?: number;
  at: number;
}

const REQUEST_KEY = 'library.request';
/** A request older than this is stale (the user opened the library some other way). */
const REQUEST_TTL_MS = 15_000;

export function isLibraryUrl(url: string): boolean {
  return url === LIBRARY_URL;
}

export function requestSection(section: LibrarySection, folder?: number, now = Date.now()): void {
  const req: LibraryRequest = { section, at: now };
  if (folder !== undefined) req.folder = folder;
  save(REQUEST_KEY, req);
}

/** Validate a stored request (anything may be in localStorage). */
export function parseRequest(raw: unknown, now = Date.now()): LibraryRequest | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as Partial<LibraryRequest>;
  if (!LIBRARY_SECTIONS.includes(r.section as LibrarySection)) return null;
  if (typeof r.at !== 'number' || now - r.at > REQUEST_TTL_MS || r.at - now > REQUEST_TTL_MS) return null;
  const out: LibraryRequest = { section: r.section as LibrarySection, at: r.at };
  if (typeof r.folder === 'number' && Number.isInteger(r.folder) && r.folder > 0) out.folder = r.folder;
  return out;
}

/** Read and clear the pending request (library page). */
export function takeRequest(now = Date.now()): LibraryRequest | null {
  const req = parseRequest(load<unknown>(REQUEST_KEY, null), now);
  if (req) save(REQUEST_KEY, null);
  return req;
}

export function onRequest(cb: () => void): () => void {
  return watch(REQUEST_KEY, cb);
}

/** How long `onDataChanged` waits for more events before calling back (a folder import or a
 * multi-remove sends a burst). */
export const DATA_CHANGED_DEBOUNCE_MS = 150;

/**
 * Calls `cb` once after a burst of `topic` events from the core (`bookmarks.changed` or
 * `history.changed`). Returns the unsubscribe function, so it fits an $effect.
 */
export function onDataChanged(
  api: Pick<BrowserApi, 'on'>,
  topic: keyof DataApiEvents,
  cb: () => void,
  delay = DATA_CHANGED_DEBOUNCE_MS,
): () => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const off = api.on(topic, () => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      cb();
    }, delay);
  });
  return () => {
    off();
    if (timer) clearTimeout(timer);
  };
}

/**
 * Calls `cb` (asynchronously) whenever the connection comes back after having been down. The
 * immediate status report of `onStatus` is skipped, so it is safe inside an $effect.
 */
export function onReconnect(api: Pick<BrowserApi, 'onStatus'>, cb: () => void): () => void {
  let last: ConnectionStatus | null = null;
  return api.onStatus((s) => {
    const prev = last;
    last = s;
    if (s === 'open' && prev !== null && prev !== 'open') queueMicrotask(cb);
  });
}

/** What `openLibrary` needs from the chrome controller. */
export interface LibraryOpener {
  tabs: { tabs: { id: number; url: string }[] };
  activate(id: number): void;
  newTab(url: string): void;
}

/** Show the library (an existing library tab is reused) at `section`. */
export function openLibrary(chrome: LibraryOpener, section: LibrarySection, folder?: number): void {
  requestSection(section, folder);
  const existing = chrome.tabs.tabs.find((t) => isLibraryUrl(t.url));
  if (existing) chrome.activate(existing.id);
  else chrome.newTab(LIBRARY_URL);
}

// ---- first run ------------------------------------------------------------------------

const FIRST_RUN_KEY = 'import.offered';

/** The wizard was shown (also when opened by hand): never offer it on launch again. */
export function markImportOffered(): void {
  save(FIRST_RUN_KEY, true);
}

/** True when there are no bookmarks, folders or history at all. */
export async function isDataEmpty(api: Pick<BrowserApi, 'call'>): Promise<boolean> {
  const [bar, other, hist] = await Promise.all([
    api.call('data.bookmarks', { folder: 1, limit: 1 }),
    api.call('data.bookmarks', { folder: 2, limit: 1 }),
    api.call('history.search', { limit: 1 }),
  ]);
  return (
    bar.bookmarks.length + bar.folders.length + other.bookmarks.length + other.folders.length + hist.entries.length === 0
  );
}

/**
 * On the very first launch (no flag yet) and with an empty database, open the import wizard.
 * The flag is set once the check succeeded either way, so it is offered at most once; a failed
 * check (core not reachable, data store not open) tries again next launch.
 */
export async function offerImportOnFirstRun(
  api: Pick<BrowserApi, 'call'>,
  open: () => void,
  store: { get: () => boolean; set: () => void } = {
    get: () => load<boolean>(FIRST_RUN_KEY, false),
    set: () => save(FIRST_RUN_KEY, true),
  },
): Promise<'opened' | 'not_empty' | 'already' | 'failed'> {
  if (store.get()) return 'already';
  let empty: boolean;
  try {
    empty = await isDataEmpty(api);
  } catch {
    return 'failed';
  }
  store.set();
  if (!empty) return 'not_empty';
  open();
  return 'opened';
}
