import { t } from './i18n/index.svelte';

/** The new tab page (served by the browser, see crates/core NEW_TAB_URL). */
export const NEWTAB_URL = 'bmcp://ui/newtab.html';
export const DEFAULT_SEARCH = 'https://www.google.com/search?q=%s';

export function isNewTab(url: string): boolean {
  return (
    url === '' ||
    url === 'about:blank' ||
    url.startsWith('bmcp://newtab') ||
    url.startsWith(NEWTAB_URL)
  );
}

const WITH_AUTHORITY = /^[a-z][a-z0-9+.-]*:\/\//i;
// javascript: is deliberately absent: typing it in the omnibox searches instead of executing.
const OPAQUE_SCHEME = /^(about|mailto|data|view-source):/i;
const HOST_LIKE = /^(localhost|\[[0-9a-f:]+\]|(\d{1,3}\.){3}\d{1,3}|([a-z0-9-]+\.)+[a-z]{2,63})(:\d{1,5})?([/?#].*)?$/i;

/**
 * Turns what the user typed in the omnibox into a URL:
 * full URLs pass through, host-like input gets https:// (http:// for localhost / IPs),
 * everything else becomes a search.
 */
export function resolveOmniboxInput(raw: string, searchTemplate = DEFAULT_SEARCH): string | null {
  const input = raw.trim();
  if (!input) return null;
  if (!/\s/.test(input)) {
    // "https://x", "bmcp://newtab", "about:blank" pass through; "example.com:8080" does not match here.
    if (WITH_AUTHORITY.test(input) || OPAQUE_SCHEME.test(input)) return input;
    if (HOST_LIKE.test(input)) {
      const local = /^(localhost|\[|(\d{1,3}\.){3}\d{1,3})/i.test(input);
      return `${local ? 'http' : 'https'}://${input}`;
    }
  }
  return searchTemplate.replace('%s', encodeURIComponent(input));
}

/** Whether Enter would open the input as an address or search for it. */
export function omniboxIntent(raw: string, searchTemplate = DEFAULT_SEARCH): 'url' | 'search' | null {
  const url = resolveOmniboxInput(raw, searchTemplate);
  if (!url) return null;
  return url === searchTemplate.replace('%s', encodeURIComponent(raw.trim())) ? 'search' : 'url';
}

export function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return '';
  }
}

/** Text shown in the omnibox when it is not focused. */
export function displayUrl(url: string): string {
  return isNewTab(url) ? '' : url;
}

/** A readable fallback title for a tab that has not reported one yet. */
export function fallbackTitle(url: string): string {
  if (isNewTab(url)) return t('components.newTab');
  return hostOf(url) || url;
}
