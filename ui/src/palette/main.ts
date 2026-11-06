// The Ctrl+K palette as its own page. In the browser it lives in an overlay view on top
// of the web page (a web page cannot be drawn over by the chrome's own view), with its
// own API connection. The browser calls bmcp.onShow() / bmcp.onHide().
import { mount } from 'svelte';
import '../lib/styles/app.css';
import { createApi } from '../lib/api';
import { installHooks, isEmbedded, nativeHost } from '../lib/native';
import { ChromeController } from '../lib/state/chrome.svelte';
import PaletteApp from './PaletteApp.svelte';

const embedded = isEmbedded();
const api = createApi();
const chrome = new ChromeController(api, { host: embedded ? nativeHost : null, role: 'palette' });
void chrome.start();

installHooks({
  onShow: () => (chrome.paletteOpen = true),
  onHide: () => (chrome.paletteOpen = false),
});
// Opened directly in a normal browser (dev): show it right away.
if (!embedded) chrome.paletteOpen = true;

mount(PaletteApp, { target: document.getElementById('app')!, props: { chrome } });
