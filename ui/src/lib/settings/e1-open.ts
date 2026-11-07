// Open other browser pages from the settings page, which has the API but not the chrome
// controller: the library (downloads list, Chrome import wizard) and plain URLs.

import type { BrowserApi } from '../api/client';
import { isLibraryUrl, LIBRARY_URL, requestSection, type LibrarySection } from '../data/library';

/** Show the library at `section`: an open library tab is reused, like the chrome's menu does. */
export async function openLibraryFrom(api: Pick<BrowserApi, 'call'>, section: LibrarySection): Promise<void> {
  requestSection(section);
  const { tabs } = await api.call('tabs.list');
  const existing = tabs.find((t) => isLibraryUrl(t.url));
  if (existing) await api.call('tabs.activate', { tab: existing.id });
  else await api.call('tabs.open', { url: LIBRARY_URL });
}

/** Copy text; false when the page may not write the clipboard. */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
