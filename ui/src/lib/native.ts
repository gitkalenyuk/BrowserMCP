// The window this page lives in, when it runs inside the real browser (CEF).
//
// Data (tabs, navigation, events) always goes through BrowserApi (WebSocket). Only
// "this window" plumbing goes here, because the browser identifies the sender of a
// bmcp://cmd/ request and therefore knows which window it means:
//   layout          where the page slot is, so the core places the web page over it
//   palette         show / hide / toggle the Ctrl+K overlay (a separate view on top)
//   focus-content   keyboard focus back to the page (after Enter in the omnibox)
//   focus-omnibox   keyboard focus to the chrome's omnibox (from the palette)
//   split           the split-view divider was dragged (ratio, done)
//   focus-pane      keyboard focus to the left / right half of the split view
//   cover           a dropdown of the chrome is open: hide the page (it would cover the
//                   dropdown, which this page draws under the native page view)
//   open-ai         (from the new tab page in a tab) show a chat in the window's AI panel
// The other direction: the browser calls window.bmcp.run(commandId) for shortcuts pressed
// while a web page has focus, bmcp.openAi(session) for open-ai, and bmcp.onShow() /
// bmcp.onHide() in the palette page.

import type { Rect } from './api/types';

/** Injected by the browser into every bmcp://ui/ page (see crates/cef/src/ui_files.rs). */
export interface BmcpConfig {
  ws?: string | null;
}

declare global {
  interface Window {
    __BMCP__?: BmcpConfig;
    bmcp?: NativeHooks;
  }
}

/** What the browser may call in our page. */
export interface NativeHooks {
  run?: (commandId: string) => void;
  /** Show this AI chat in the panel (asked by a new tab page of the window: `openAiPanel`). */
  openAi?: (session: string) => void;
  onShow?: () => void;
  onHide?: () => void;
}

export interface NativeHost {
  layout(rect: Rect, viewport: { w: number; h: number }): void;
  palette(action: 'show' | 'hide' | 'toggle'): void;
  focusContent(): void;
  focusOmnibox(): void;
  splitRatio(ratio: number, done: boolean): void;
  focusPane(side: 'left' | 'right'): void;
  cover(on: boolean): void;
}

/** This chrome page's window key (the browser adds `?window=<key>` to its URL). */
export function windowKey(search: string = typeof location === 'undefined' ? '' : location.search): number | null {
  const v = new URLSearchParams(search).get('window');
  const n = v === null ? NaN : Number(v);
  return Number.isInteger(n) ? n : null;
}

/** True when served by the browser itself (bmcp://ui/ or a --ui-dev page inside CEF). */
export function isEmbedded(loc: { protocol: string } = location): boolean {
  return loc.protocol === 'bmcp:' || (typeof window !== 'undefined' && !!window.__BMCP__);
}

function send(name: string, args: Record<string, string | number> = {}): void {
  const query = new URLSearchParams(Object.entries(args).map(([k, v]) => [k, String(v)])).toString();
  fetch(`bmcp://cmd/${name}${query ? `?${query}` : ''}`).catch(() => {
    // Not inside the browser, or the window is closing: nothing to do.
  });
}

export const nativeHost: NativeHost = {
  layout: (r, vp) => send('layout', { x: r.x, y: r.y, w: r.w, h: r.h, vw: vp.w, vh: vp.h }),
  palette: (action) => send('palette', { action }),
  focusContent: () => send('focus-content'),
  focusOmnibox: () => send('focus-omnibox'),
  splitRatio: (ratio, done) => send('split', done ? { ratio, done: 1 } : { ratio }),
  focusPane: (side) => send('focus-pane', { side }),
  cover: (on) => send('cover', { on: on ? 1 : 0 }),
};

/**
 * From one of our pages shown in a tab (the new tab page): open the AI panel of the window
 * around the tab on chat `session`. The browser accepts `open-ai` from a tab only while it
 * shows a bmcp://ui page, and calls `bmcp.openAi(session)` in that window's chrome. In the
 * dev mock the page sits in the chrome's iframe and asks the parent instead.
 */
export function openAiPanel(session: string): void {
  if (typeof window !== 'undefined' && window.parent !== window) {
    window.parent.postMessage({ type: 'bmcp:open-ai', session }, location.origin);
    return;
  }
  send('open-ai', { session });
}

/** Install the hooks the browser calls (merged with ones installed earlier). */
export function installHooks(hooks: NativeHooks): void {
  window.bmcp = { ...(window.bmcp ?? {}), ...hooks };
}
