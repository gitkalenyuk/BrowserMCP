// Plumbing between the chrome and the settings page (bmcp://ui/settings.html), the same way as
// the library: the URL carries no query (the browser lets content tabs load exactly
// `bmcp://ui/settings.html`, or with a `#guide/…` anchor), the section to show travels through
// localStorage (same origin).

import { load, save, watch } from '../storage';

export const SETTINGS_URL = 'bmcp://ui/settings.html';
export const SETTINGS_SECTIONS = [
  'general',
  'ai',
  'notifications',
  'performance',
  'privacy',
  'hints',
  'plugins',
  'keyboard',
  // The user guide (ui/src/settings/guide); deep links: `#guide/<chapter>[/<section>]`.
  'guide',
] as const;
export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

const REQUEST_KEY = 'settings.request';
const REQUEST_TTL_MS = 15_000;

export function isSettingsUrl(url: string): boolean {
  // An in-page `#guide/…` anchor may show up in the URL until the page clears it.
  return url === SETTINGS_URL || url.startsWith(`${SETTINGS_URL}#`);
}

export function isSettingsSection(s: unknown): s is SettingsSection {
  return typeof s === 'string' && (SETTINGS_SECTIONS as readonly string[]).includes(s);
}

/** Validate a stored request (anything may be in localStorage). */
export function parseSettingsRequest(raw: unknown, now = Date.now()): SettingsSection | null {
  if (!raw || typeof raw !== 'object') return null;
  const r = raw as { section?: unknown; at?: unknown };
  if (!isSettingsSection(r.section)) return null;
  if (typeof r.at !== 'number' || Math.abs(now - r.at) > REQUEST_TTL_MS) return null;
  return r.section;
}

/** Read and clear the pending request (settings page). */
export function takeSettingsRequest(now = Date.now()): SettingsSection | null {
  const section = parseSettingsRequest(load<unknown>(REQUEST_KEY, null), now);
  if (section) save(REQUEST_KEY, null);
  return section;
}

/**
 * `chapter` or `chapter/section` from `#guide/<chapter>[/<section>]` (ids: lower-case letters,
 * digits and dashes); null for any other hash.
 */
export function parseGuideHash(hash: string): string | null {
  const m = /^#?guide\/([a-z0-9-]{1,40}(?:\/[a-z0-9-]{1,40})?)$/.exec(hash);
  return m ? m[1] : null;
}

/** The guide anchor of a pending request (read it before `takeSettingsRequest` clears it). */
export function peekGuideAnchor(now = Date.now()): string | null {
  const raw = load<unknown>(REQUEST_KEY, null);
  if (parseSettingsRequest(raw, now) !== 'guide') return null;
  const anchor = (raw as { guide?: unknown }).guide;
  return typeof anchor === 'string' ? parseGuideHash(`guide/${anchor}`) : null;
}

export function onSettingsRequest(cb: () => void): () => void {
  return watch(REQUEST_KEY, cb);
}

/** What `openSettings` needs from the chrome controller. */
export interface SettingsOpener {
  tabs: { tabs: { id: number; url: string }[] };
  activate(id: number): void;
  newTab(url: string): void;
}

/** Show the settings page (an existing settings tab is reused), optionally at `section`. */
export function openSettings(chrome: SettingsOpener, section?: SettingsSection, now = Date.now()): void {
  if (section) save(REQUEST_KEY, { section, at: now });
  const existing = chrome.tabs.tabs.find((t) => isSettingsUrl(t.url));
  if (existing) chrome.activate(existing.id);
  else chrome.newTab(SETTINGS_URL);
}

/** Show the guide at `anchor` (`chapter` or `chapter/section`) on the settings page. */
export function openGuide(chrome: SettingsOpener, anchor: string, now = Date.now()): void {
  save(REQUEST_KEY, { section: 'guide', guide: anchor, at: now });
  openSettings(chrome, undefined, now);
}
