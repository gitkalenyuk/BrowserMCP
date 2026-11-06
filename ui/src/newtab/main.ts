import { mount } from 'svelte';
import '../lib/styles/app.css';
import { createApi } from '../lib/api';
import NewTab from './NewTab.svelte';

const api = createApi();
void api.connect();

const embedded = window.parent !== window;
if (embedded) {
  // Dev only: inside the mock chrome's iframe, forward shortcuts to the chrome (the real
  // core intercepts them before they reach the page).
  window.addEventListener(
    'keydown',
    (e) => {
      if (!(e.ctrlKey || e.altKey || e.metaKey || /^F\d+$/.test(e.key))) return;
      const { key, code, ctrlKey, shiftKey, altKey, metaKey } = e;
      window.parent.postMessage({ type: 'bmcp:key', key: { key, code, ctrlKey, shiftKey, altKey, metaKey } }, location.origin);
      if (ctrlKey && ['KeyT', 'KeyW', 'KeyK', 'KeyL', 'KeyR', 'Tab'].includes(code)) e.preventDefault();
    },
    true,
  );
}

mount(NewTab, { target: document.getElementById('app')!, props: { api, embedded } });
