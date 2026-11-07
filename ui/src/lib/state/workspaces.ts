// Workspaces = profiles (own cookies, cache, proxy, identity). Pure helpers, unit-tested.
//
// The core keeps every tab of a window in one tab strip; each tab carries its `profile` and
// `window`. A chrome page shows only the tabs of its own window and of the workspace picked in
// that window, never off-screen (automation) tabs.
import type { GroupColor, ProfileEntry, TabId, TabInfo, Workspace } from '../api/types';
import { t } from '../i18n/index.svelte';
import { formatList } from '../i18n/intl';
import type { TabsState } from './tabs';

const COLORS: GroupColor[] = ['blue', 'purple', 'green', 'orange', 'pink', 'cyan', 'red', 'yellow', 'grey'];

/** Readable proxy label: "Direct", or the masked URL's scheme and host ("socks5 · 1.2.3.4:1080"). */
export function proxyLabel(proxy: string | null): string {
  if (!proxy) return t('chrome.proxy.direct');
  const m = proxy.match(/^([a-z0-9]+):\/\/(?:[^@/]*@)?([^/]+)/i);
  return m ? `${m[1]} · ${m[2]}` : proxy;
}

/** profile.list → workspaces (launch profile first, then by name; no in-memory profiles). */
export function workspacesFromProfiles(profiles: ProfileEntry[]): Workspace[] {
  const stable = profiles.filter((p) => !p.incognito);
  stable.sort((a, b) => Number(b.launch) - Number(a.launch) || a.label.localeCompare(b.label));
  return stable.map((p, i) => ({
    id: p.id,
    name: p.label || p.id,
    profile: p.id,
    proxy: proxyLabel(p.proxy),
    color: p.color ?? COLORS[i % COLORS.length],
    identity: p.identity,
    launch: p.launch,
  }));
}

export interface TabFilter {
  /** This chrome page's window key (null: show every window, e.g. the dev mock). */
  window: number | null;
  /** Workspace (profile id) picked in this window; null = before profiles are known. */
  workspace: string | null;
  /** Profile the browser started with: tabs without a `profile` belong to it. */
  launch: string;
}

/** Does `tab` belong in this window's strip? */
export function tabVisible(tab: TabInfo, f: TabFilter): boolean {
  if (tab.offscreen) return false;
  if (f.window !== null && tab.window != null && tab.window !== f.window) return false;
  if (f.workspace === null) return true;
  return (tab.profile ?? f.launch) === f.workspace;
}

/** The tab strip this chrome page shows. The active tab stays active only if it is visible. */
export function visibleTabs(state: TabsState, f: TabFilter): TabsState {
  const tabs = state.tabs.filter((t) => tabVisible(t, f));
  const activeId = tabs.some((t) => t.id === state.activeId) ? state.activeId : null;
  return { ...state, tabs, activeId };
}

/** Workspace of a tab. */
export function workspaceOf(tab: TabInfo, launch: string): string {
  return tab.profile ?? launch;
}

/**
 * Which tab to show when switching to `workspace`: the one shown there last, else its first
 * tab in this window, else null (the chrome then opens a new tab page in that profile).
 */
export function tabForWorkspace(
  all: TabInfo[],
  f: TabFilter,
  lastActive: Record<string, TabId | undefined>,
): TabId | null {
  const tabs = all.filter((t) => tabVisible(t, f));
  const last = lastActive[f.workspace ?? ''];
  if (last !== undefined && tabs.some((t) => t.id === last)) return last;
  return tabs[0]?.id ?? null;
}

/** A profile id from a workspace name: "My Work 2" → "my-work-2" (unique among `taken`). */
export function profileIdFor(name: string, taken: string[]): string {
  const base =
    name
      .normalize('NFKD')
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, '-')
      .replace(/^-+|-+$/g, '')
      .slice(0, 48) || 'workspace';
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
  return id;
}

/** Text the AI panel gets for "Ask AI about this" (page context menu). */
export function askAiPrompt(p: { kind: string; text: string; url: string; title: string }): string {
  const page = p.title ? `"${p.title}" (${p.url})` : p.url;
  // The prompt lands in the user's own draft, so it is written in the UI language.
  switch (p.kind) {
    case 'selection':
      return `${t('chrome.ask.selection', { page })}\n\n> ${p.text.trim().replace(/\n/g, '\n> ')}\n\n`;
    case 'link':
      return `${t('chrome.ask.link', { page, text: p.text })}\n\n`;
    case 'image':
      return `${t('chrome.ask.image', { page, text: p.text })}\n\n`;
    default:
      return `${t('chrome.ask.page', { page })}\n\n`;
  }
}

/** Divider position for a split view inside a page slot of width `w` (mirrors protocol.rs). */
export const SPLIT_GAP = 6;
export function splitLeftWidth(w: number, ratio: number): number {
  const room = Math.max(0, w - SPLIT_GAP);
  return Math.min(room, Math.max(0, Math.round(room * ratio)));
}

/** Ratio for a divider dragged to `x` (px from the slot's left edge), clamped like the core. */
export function splitRatioAt(x: number, w: number): number {
  const room = Math.max(1, w - SPLIT_GAP);
  const r = (x - SPLIT_GAP / 2) / room;
  return Math.min(0.85, Math.max(0.15, r));
}

/** Permission kinds with a human label (`chrome.perm.<kind>`: "geolocation" → "know your location"). */
const KNOWN_KINDS = new Set([
  'geolocation',
  'notifications',
  'camera',
  'microphone',
  'screen_capture',
  'clipboard',
  'midi',
  'local_network',
  'sensors',
  'storage_access',
]);

/** What the page wants, after "<origin> wants to": "use your camera and use your microphone". */
export function permissionText(kinds: string[]): string {
  const parts = kinds.map((k) => (KNOWN_KINDS.has(k) ? t(`chrome.perm.${k}`) : k.replace(/_/g, ' ')));
  if (!parts.length) return t('chrome.perm.unknown');
  return formatList(parts);
}
