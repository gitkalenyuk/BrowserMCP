// Bookmark state the chrome shows: the bookmarks bar (folder 1) and whether the active page is
// bookmarked (the omnibox star). One instance per API connection, shared by the components.

import type { BrowserApi } from '../api/client';
import { BOOKMARKS_BAR, type DataBookmark, type DataFolder } from '../api/types-data';
import { isSafeLinkUrl } from '../safeurl';
import { bumpDataRev, onDataRev, onReconnect } from './library';
import { mergeChildren } from './tree';

export type BarItem = ({ kind: 'url' } & DataBookmark) | ({ kind: 'folder' } & DataFolder);

/** Can the star bookmark this URL? (web pages only, not our own pages) */
export const isStarrable = (url: string | null | undefined): url is string => isSafeLinkUrl(url);

export class ChromeBookmarks {
  readonly api: BrowserApi;
  bar = $state<BarItem[]>([]);
  /** Bookmarks of the URL last asked about with `check`. */
  marked = $state<DataBookmark[]>([]);
  checkedUrl = $state<string | null>(null);
  busy = $state(false);
  #users = 0;
  #off: (() => void)[] = [];
  #checkSeq = 0;

  constructor(api: BrowserApi) {
    this.api = api;
  }

  /** Components call this in an $effect; the store listens while anyone uses it. */
  use(): () => void {
    if (this.#users++ === 0) {
      const refresh = () => {
        void this.refreshBar();
        if (this.checkedUrl) void this.check(this.checkedUrl);
      };
      this.#off.push(
        onDataRev(refresh),
        onReconnect(this.api, refresh),
      );
      queueMicrotask(refresh);
      if (typeof window !== 'undefined') {
        window.addEventListener('focus', refresh);
        this.#off.push(() => window.removeEventListener('focus', refresh));
      }
    }
    return () => {
      if (--this.#users === 0) {
        this.#off.forEach((f) => f());
        this.#off = [];
      }
    };
  }

  async refreshBar(): Promise<void> {
    try {
      const listing = await this.api.call('data.bookmarks', { folder: BOOKMARKS_BAR, limit: 200 });
      this.bar = mergeChildren(listing).map((item) =>
        'url' in item ? { kind: 'url' as const, ...item } : { kind: 'folder' as const, ...item },
      );
    } catch {
      // No data store (older core) or disconnected: keep what we have.
    }
  }

  /** Look up the bookmarks of `url` (the star state). */
  async check(url: string | null): Promise<void> {
    const seq = ++this.#checkSeq;
    this.checkedUrl = url;
    if (!isStarrable(url)) {
      this.marked = [];
      return;
    }
    try {
      const { bookmarks } = await this.api.call('bookmarks.search', { q: url, limit: 50 });
      if (seq === this.#checkSeq) this.marked = bookmarks.filter((b) => b.url === url);
    } catch {
      if (seq === this.#checkSeq) this.marked = [];
    }
  }

  /** Star click: bookmark the page in the bookmarks bar, or remove all its bookmarks. */
  async toggle(url: string | null | undefined, title: string | null | undefined): Promise<void> {
    if (!isStarrable(url) || this.busy) return;
    this.busy = true;
    try {
      if (this.checkedUrl !== url) await this.check(url);
      if (this.marked.length) {
        for (const b of this.marked) await this.api.call('bookmarks.remove', { id: b.id });
      } else {
        await this.api.call('bookmarks.add', { url, title: title?.trim() || null, folder: BOOKMARKS_BAR });
      }
    } finally {
      this.busy = false;
      bumpDataRev();
      await Promise.all([this.check(url), this.refreshBar()]);
    }
  }
}

const instances = new WeakMap<BrowserApi, ChromeBookmarks>();

export function chromeBookmarks(api: BrowserApi): ChromeBookmarks {
  let b = instances.get(api);
  if (!b) instances.set(api, (b = new ChromeBookmarks(api)));
  return b;
}
