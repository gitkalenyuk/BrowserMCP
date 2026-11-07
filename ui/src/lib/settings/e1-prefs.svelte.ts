// What the chrome needs from Settings → Startup & tabs and Search: the address bar's engines,
// the page a new tab opens and the Home button. Read from the core (startup.config,
// search.config) on connect and again on every `settings.changed`, so a change in the
// settings tab, by an agent or over REST shows up in every window at once.

import type { BrowserApi } from '../api/client';
import { NEWTAB_URL } from '../urls';
import { FALLBACK_SEARCH, omniboxSearch, type OmniboxSearch } from './e1-search';
import type { StartupConfig } from './e1-types';

export class BrowserPrefs {
  search = $state<OmniboxSearch>(FALLBACK_SEARCH);
  startup = $state<StartupConfig | null>(null);

  /** The URL of a new tab (Ctrl+T, +). */
  get newTabUrl(): string {
    return this.startup?.new_tab_url || NEWTAB_URL;
  }

  /** Where the Home button goes. */
  get homeUrl(): string {
    return this.startup?.home_page || this.newTabUrl;
  }

  get showHome(): boolean {
    return this.startup?.show_home_button ?? false;
  }

  /** Load now, on reconnect and on every change. Returns the unsubscribe function. */
  start(api: BrowserApi): () => void {
    const load = (area?: string) => {
      if (!area || area === 'search') {
        api.call('search.config').then(
          (c) => (this.search = omniboxSearch(c)),
          // An older core without search.config: keep Google.
          () => {},
        );
      }
      if (!area || area === 'startup') {
        api.call('startup.config').then(
          (r) => (this.startup = r.config),
          () => {},
        );
      }
    };
    const offChanged = api.on('settings.changed', (e) => load(e.area));
    let last: string | null = null;
    const offStatus = api.onStatus((s) => {
      if (s === 'open' && last !== 'open') load();
      last = s;
    });
    return () => {
      offChanged();
      offStatus();
    };
  }
}
