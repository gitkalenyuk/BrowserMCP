// Logic of Settings → Startup & tabs (pure, unit-tested). The core checks every URL again
// (crates/settings/src/urls.rs) and has the last word; this only gives instant feedback.

import { NEWTAB_URL } from '../urls';
import type { StartupArgs, StartupConfig } from './e1-types';

/** Most pages "Open specific pages" takes (crates/settings MAX_PAGES). */
export const MAX_PAGES = 20;

/** `example.com` → `https://example.com/`; null when it cannot be a page URL. */
export function normalizePage(text: string): string | null {
  const t = text.trim();
  if (!t || /\s/.test(t)) return null;
  if (/^(javascript|data|vbscript):/i.test(t)) return null;
  if (/^(about|bmcp|file):/i.test(t)) return t;
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(t) ? t : `${/^(localhost|127\.)/i.test(t) ? 'http' : 'https'}://${t}`;
  try {
    const u = new URL(withScheme);
    if (u.protocol !== 'https:' && u.protocol !== 'http:') return null;
    if (!u.hostname) return null;
    return u.href;
  } catch {
    return null;
  }
}

/** One page per line → the list to save; `bad` = lines that are not URLs. */
export function parsePages(text: string): { pages: string[]; bad: string[] } {
  const pages: string[] = [];
  const bad: string[] = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.trim()) continue;
    const url = normalizePage(line);
    if (url) {
      if (!pages.includes(url)) pages.push(url);
    } else bad.push(line.trim());
  }
  return { pages, bad };
}

/** What the form edits (strings for the inputs). */
export interface StartupForm {
  on_startup: StartupConfig['on_startup'];
  pagesText: string;
  show_home_button: boolean;
  home_page: string;
  new_tab_mode: 'builtin' | 'blank' | 'url';
  new_tab_url: string;
  new_tab_position: StartupConfig['new_tab_position'];
  keep_window_on_last_tab: boolean;
}

export function startupForm(c: StartupConfig): StartupForm {
  const url = c.new_tab_url ?? '';
  return {
    on_startup: c.on_startup,
    pagesText: c.pages.join('\n'),
    show_home_button: c.show_home_button,
    home_page: c.home_page ?? '',
    new_tab_mode: !url || url === NEWTAB_URL ? 'builtin' : url === 'about:blank' ? 'blank' : 'url',
    new_tab_url: url === 'about:blank' ? '' : url,
    new_tab_position: c.new_tab_position,
    keep_window_on_last_tab: c.keep_window_on_last_tab,
  };
}

export type StartupErrors = Partial<Record<'pages' | 'home_page' | 'new_tab_url', 'bad' | 'empty' | 'tooMany'>>;

export function startupErrors(f: StartupForm): StartupErrors {
  const e: StartupErrors = {};
  const { pages, bad } = parsePages(f.pagesText);
  if (bad.length) e.pages = 'bad';
  else if (f.on_startup === 'pages' && pages.length === 0) e.pages = 'empty';
  else if (pages.length > MAX_PAGES) e.pages = 'tooMany';
  if (f.home_page.trim() && !normalizePage(f.home_page)) e.home_page = 'bad';
  if (f.new_tab_mode === 'url' && !normalizePage(f.new_tab_url)) e.new_tab_url = f.new_tab_url.trim() ? 'bad' : 'empty';
  return e;
}

/** The startup.config arguments that change `saved` into the form (only changed fields). */
export function startupPatch(saved: StartupConfig, f: StartupForm): StartupArgs {
  const patch: StartupArgs = {};
  if (f.on_startup !== saved.on_startup) patch.on_startup = f.on_startup;
  const { pages } = parsePages(f.pagesText);
  if (pages.join('\n') !== saved.pages.join('\n')) patch.pages = pages;
  if (f.show_home_button !== saved.show_home_button) patch.show_home_button = f.show_home_button;
  const home = f.home_page.trim() ? (normalizePage(f.home_page) ?? f.home_page.trim()) : '';
  if (home !== (saved.home_page ?? '')) patch.home_page = home;
  const newTab = f.new_tab_mode === 'builtin' ? '' : f.new_tab_mode === 'blank' ? 'about:blank' : (normalizePage(f.new_tab_url) ?? f.new_tab_url.trim());
  if (newTab !== (saved.new_tab_url ?? '')) patch.new_tab_url = newTab;
  if (f.new_tab_position !== saved.new_tab_position) patch.new_tab_position = f.new_tab_position;
  if (f.keep_window_on_last_tab !== saved.keep_window_on_last_tab) patch.keep_window_on_last_tab = f.keep_window_on_last_tab;
  return patch;
}
