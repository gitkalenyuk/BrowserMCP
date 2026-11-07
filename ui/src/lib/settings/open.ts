// Plumbing between the chrome and the settings page (bmcp://ui/settings.html), the same way as
// the library: the URL carries no query (the browser lets content tabs load exactly
// `bmcp://ui/settings.html`), the section to show travels through localStorage (same origin).

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
  'connections',
  'profiles',
  'proxies',
  'recording',
] as const;
export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

const REQUEST_KEY = 'settings.request';
const REQUEST_TTL_MS = 15_000;

export function isSettingsUrl(url: string): boolean {
  return url === SETTINGS_URL;
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
