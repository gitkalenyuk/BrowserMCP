import { mount } from 'svelte';
import '../lib/styles/app.css';
import { createApi } from '../lib/api';
import { offerImportOnFirstRun, openLibrary } from '../lib/data/library';
import { COMMAND_IDS, type CommandId } from '../lib/keymap';
import { installHooks, isEmbedded, nativeHost } from '../lib/native';
import { ChromeController } from '../lib/state/chrome.svelte';
import App from './App.svelte';

const api = createApi();
const chrome = new ChromeController(api, { host: isEmbedded() ? nativeHost : null });
// First launch with an empty database: open the Chrome import wizard (once).
void chrome.start().then(() => offerImportOnFirstRun(api, () => openLibrary(chrome, 'import')));

// Shortcuts pressed while a web page has focus arrive from the browser as command ids.
installHooks({
  run: (id) => {
    if ((COMMAND_IDS as readonly string[]).includes(id)) chrome.run(id as CommandId);
  },
});

mount(App, { target: document.getElementById('app')!, props: { chrome } });

if (import.meta.env.DEV) {
  // Handy in devtools: __bmcp.api.call('tabs.list')
  (window as unknown as { __bmcp: unknown }).__bmcp = { api, chrome };
}
