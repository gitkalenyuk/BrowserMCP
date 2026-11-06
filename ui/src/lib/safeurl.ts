/**
 * URLs coming from data (bookmarks, history, API results) may only become clickable links when
 * they are plain web URLs. `javascript:` / `data:` links would run inside the privileged
 * bmcp://ui origin, which can call the whole browser API.
 */
export function isSafeLinkUrl(url: string | null | undefined): url is string {
  if (typeof url !== 'string' || url.length === 0) return false;
  try {
    const { protocol } = new URL(url);
    return protocol === 'http:' || protocol === 'https:';
  } catch {
    return false;
  }
}
