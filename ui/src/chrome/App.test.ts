// The browser window (tab strip, toolbar, omnibox) renders in the UI language. Server render
// with the mock core: markup only, so it checks the strings, not the behaviour.
import { render } from 'svelte/server';
import { afterEach, describe, expect, it } from 'vitest';
import { MockApi } from '../lib/api/mock';
import { settings } from '../lib/settings.svelte';
import { ChromeController } from '../lib/state/chrome.svelte';
import App from './App.svelte';

const page = () => render(App, { props: { chrome: new ChromeController(new MockApi(), { host: null }) } }).body;

afterEach(() => settings.update({ language: 'system' }));

describe('browser window', () => {
  it('is English by default', () => {
    const html = page();
    expect(html).toContain('aria-label="New tab"');
    expect(html).toContain('placeholder="Search or enter address"');
  });

  it('switches to Ukrainian', () => {
    settings.update({ language: 'uk' });
    const html = page();
    expect(html).toContain('aria-label="Нова вкладка"');
    expect(html).toContain('placeholder="Шукайте або введіть адресу"');
    expect(html).toContain('aria-label="Панель «Трафік»"');
    expect(html).not.toContain('aria-label="New tab"');
  });
});
