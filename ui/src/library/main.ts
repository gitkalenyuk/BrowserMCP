import { mount } from 'svelte';
import '../lib/styles/app.css';
import './library.css';
import { createApi } from '../lib/api';
import Library from './Library.svelte';

// SECURITY: createApi takes the endpoint only from the browser's injected config (or, in dev
// builds, from the query). This page can reveal passwords: a URL must never pick its backend.
const api = createApi();
void api.connect();

mount(Library, { target: document.getElementById('app')!, props: { api } });

if (import.meta.env.DEV) (window as unknown as { __bmcp: unknown }).__bmcp = { api };
