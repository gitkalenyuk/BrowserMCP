// Wire types of the daily-driver data commands (crates/data): bookmarks, history, downloads,
// passwords and the Chrome import, their change events, and system.reveal_path / open_path
// (crates/system). Copied from sdk/protocol/src/generated/commands.ts (the
// generated contract); keep them in sync with it. Times are Unix milliseconds.
//
// They are merged into RpcMethods (types.ts) by the module augmentation at the bottom, so
// `api.call('history.search', { q })` is typed like every other method.

/** Fixed id of the "Bookmarks bar" root folder. */
export const BOOKMARKS_BAR = 1;
/** Fixed id of the "Other bookmarks" root folder. */
export const OTHER_BOOKMARKS = 2;

// ---- bookmarks --------------------------------------------------------------------------

/** A URL bookmark. */
export interface DataBookmark {
  id: number;
  title: string;
  url: string;
  /** Id of the folder it is in. */
  folder: number;
  /** Index among its siblings (folders and bookmarks share one order). */
  position: number;
  created: number;
}

/** A bookmark folder. */
export interface DataFolder {
  id: number;
  title: string;
  /** Parent folder; `null` for the two root folders. */
  parent: number | null;
  position: number;
  created: number;
  /** Number of direct children (folders + bookmarks). */
  children: number;
}

export interface DataBookmarksArgs {
  /** Folder id (default 1 = bookmarks bar; 2 = other bookmarks). */
  folder?: number | null;
  /** Max bookmarks to return (default 500). */
  limit?: number | null;
}

export interface DataBookmarksOutput {
  /** The listed folder's id. */
  folder: number;
  /** URL bookmarks directly in the folder, in order. */
  bookmarks: DataBookmark[];
  /** Sub-folders directly in the folder, in order. */
  folders: DataFolder[];
}

export interface BookmarksAddArgs {
  /** Folder id (default 1 = bookmarks bar). */
  folder?: number | null;
  /** Title (default: the page title from history, else the URL). */
  title?: string | null;
  url: string;
}

export interface BookmarksAddFolderArgs {
  /** Parent folder id (default 1 = bookmarks bar). */
  parent?: number | null;
  title: string;
}

export interface BookmarksMoveArgs {
  /** Bookmark or folder id. */
  id: number;
  /** Target folder id (default: keep the current folder). */
  folder?: number | null;
  /** Index among the target folder's children, not counting the moved node (default: the end). */
  position?: number | null;
}

export interface BookmarksMoveOutput {
  id: number;
  folder: number;
  position: number;
}

export interface BookmarksUpdateArgs {
  /** Bookmark or folder id. */
  id: number;
  /** New title (bookmark: empty = the URL; folder: must not be empty). */
  title?: string | null;
  /** New URL (bookmarks only; http, https or file). */
  url?: string | null;
}

/** What `bookmarks.update` changed: exactly one of the two is set. */
export interface BookmarksUpdateOutput {
  bookmark: DataBookmark | null;
  folder: DataFolder | null;
}

// ---- history ----------------------------------------------------------------------------

export interface HistoryEntry {
  url: string;
  title: string;
  visit_count: number;
  /** Unix ms of the newest visit. */
  last_visit: number;
}

export interface HistorySearchArgs {
  /** Text to find in URLs and titles (default: everything). */
  q?: string | null;
  /** Only URLs visited at or after this time (Unix ms). */
  since?: number | null;
  /** Max results (default 100, max 5000). */
  limit?: number | null;
}

/**
 * Which visits to delete (at least one field): those of `url` (alone: the URL with all its
 * visits) and/or in `[since, before)` or on the local `day` (`YYYY-MM-DD`). URLs left without
 * visits are dropped.
 */
export interface HistoryDeleteArgs {
  url?: string | null;
  /** Unix ms, inclusive. */
  since?: number | null;
  /** Unix ms, exclusive. */
  before?: number | null;
  /** Local calendar day `YYYY-MM-DD` (not together with since/before). */
  day?: string | null;
}

export interface HistoryDeleteOutput {
  /** URLs that had no visit left and were dropped. */
  deleted_urls: number;
  deleted_visits: number;
}

export interface TopSite {
  url: string;
  /** Page title, or the host name when the page had none. */
  title: string;
  visit_count: number;
  last_visit: number;
  /** Frecency score (higher = shown first). */
  score: number;
}

// ---- downloads --------------------------------------------------------------------------

export interface Download {
  id: number;
  url: string;
  /** Target file on disk. */
  path: string;
  mime: string;
  received_bytes: number;
  /** Size in bytes when the server said it. */
  total_bytes: number | null;
  /** `in_progress`, `complete`, `cancelled` or `interrupted`. */
  state: string;
  started: number;
  /** `null` while in progress. */
  ended: number | null;
}

// ---- passwords --------------------------------------------------------------------------

export interface PasswordEntry {
  id: number;
  /** `scheme://host[:port]`. */
  origin: string;
  username: string;
  /** Plain text only when revealed, otherwise `null`. */
  password: string | null;
  note: string;
  created: number;
  updated: number;
}

export interface PasswordsListArgs {
  /** Only this site (URL or origin). Default: all sites. */
  origin?: string | null;
  /** Include the plain-text passwords (default false). */
  reveal?: boolean;
}

export interface PasswordsSaveArgs {
  origin: string;
  username: string;
  password: string;
  note?: string | null;
}

export interface PasswordsSaveOutput {
  id: number;
  /** The normalized origin it was saved under. */
  origin: string;
  /** False when an existing login (same origin + username) was updated. */
  created: boolean;
}

// ---- Chrome import ----------------------------------------------------------------------

export type ImportPart = 'bookmarks' | 'history' | 'passwords_csv' | 'cookies_json';

export interface ChromeProfile {
  /** Folder name: `Default`, `Profile 1`, … */
  name: string;
  /** Full path; pass it as `profile_dir` to import.chrome. */
  path: string;
  has_bookmarks: boolean;
  has_history: boolean;
}

export interface ImportChromeDetectOutput {
  profiles: ChromeProfile[];
  /** The folders that were scanned. */
  user_data_dirs: string[];
}

export interface ImportChromeArgs {
  parts: ImportPart[];
  /** Chrome profile folder for bookmarks/history (default: `<Chrome User Data>/Default`). */
  profile_dir?: string | null;
  /** The CSV exported from Chrome's password manager (needed for `passwords_csv`). */
  passwords_csv_path?: string | null;
  /** A cookie export JSON file (needed for `cookies_json`). */
  cookies_json_path?: string | null;
}

export interface ImportPartResult {
  part: ImportPart;
  ok: boolean;
  imported: number;
  skipped: number;
  /** File it read. */
  source: string;
  error: string | null;
}

// ---- system (crates/system) ------------------------------------------------------------

/** An absolute path inside the BrowserMCP data folder or the Downloads folder. */
export interface SystemPathArgs {
  path: string;
}

export interface SystemPathOutput {
  /** The resolved path that was shown. */
  path: string;
}

// ---- change events ----------------------------------------------------------------------

/** Pushed by the core after every edit, by any caller (UI, REST, MCP, scripts). */
export interface DataApiEvents {
  /** `id`: the bookmark/folder that changed; `null` after an import. */
  'bookmarks.changed': { op: 'add' | 'add_folder' | 'update' | 'remove' | 'move' | 'import'; id: number | null };
  'history.changed': { op: 'delete' | 'import' };
}

/** name -> params / result of every data command. */
export interface DataRpcMethods {
  'data.bookmarks': { params: DataBookmarksArgs; result: DataBookmarksOutput };
  'data.top_sites': { params: { limit?: number | null }; result: { sites: TopSite[] } };
  'bookmarks.add': { params: BookmarksAddArgs; result: DataBookmark };
  'bookmarks.add_folder': { params: BookmarksAddFolderArgs; result: DataFolder };
  'bookmarks.remove': { params: { id: number }; result: { removed: number } };
  'bookmarks.move': { params: BookmarksMoveArgs; result: BookmarksMoveOutput };
  'bookmarks.update': { params: BookmarksUpdateArgs; result: BookmarksUpdateOutput };
  'bookmarks.search': { params: { q: string; limit?: number | null }; result: { bookmarks: DataBookmark[] } };
  'history.search': { params: HistorySearchArgs; result: { entries: HistoryEntry[] } };
  'history.delete': { params: HistoryDeleteArgs; result: HistoryDeleteOutput };
  'downloads.list': { params: { limit?: number | null }; result: { downloads: Download[] } };
  'passwords.list': { params: PasswordsListArgs; result: { entries: PasswordEntry[] } };
  'passwords.get': { params: { origin: string }; result: { entries: PasswordEntry[] } };
  'passwords.save': { params: PasswordsSaveArgs; result: PasswordsSaveOutput };
  'passwords.delete': { params: { id: number }; result: { deleted: boolean } };
  'import.chrome_detect': { params: { user_data_dir?: string | null }; result: ImportChromeDetectOutput };
  'import.chrome': { params: ImportChromeArgs; result: { results: ImportPartResult[] } };
  'system.reveal_path': { params: SystemPathArgs; result: SystemPathOutput };
  'system.open_path': { params: SystemPathArgs; result: SystemPathOutput };
}

declare module './types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends DataRpcMethods {}
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface ApiEvents extends DataApiEvents {}
}
