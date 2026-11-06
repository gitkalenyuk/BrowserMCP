import { mount } from 'svelte';
import '../lib/styles/app.css';
import { createApi } from '../lib/api';
import { ChromeController } from '../lib/state/chrome.svelte';
import App from './App.svelte';

const api = createApi();
const chrome = new ChromeController(api);
void chrome.start();

mount(App, { target: document.getElementById('app')!, props: { chrome } });

if (import.meta.env.DEV) {
  // Handy in devtools: __bmcp.api.call('tabs.list')
  (window as unknown as { __bmcp: unknown }).__bmcp = { api, chrome };
}
