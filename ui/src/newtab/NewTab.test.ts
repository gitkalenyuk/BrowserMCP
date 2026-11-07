// The new tab page renders in the UI language (server render: markup only, no effects).
import { render } from 'svelte/server';
import { afterEach, describe, expect, it } from 'vitest';
import type { BrowserApi } from '../lib/api';
import { settings } from '../lib/settings.svelte';
import NewTab from './NewTab.svelte';

const api = { call: () => new Promise(() => {}), on: () => () => {} } as unknown as BrowserApi;
const page = () => render(NewTab, { props: { api, embedded: false } }).body;

afterEach(() => settings.update({ language: 'system' }));

describe('new tab page', () => {
  it('is English by default', () => {
    const html = page();
    expect(html).toContain('What should I do?');
    expect(html).toContain('Customize');
  });

  it('switches to Ukrainian', () => {
    settings.update({ language: 'uk' });
    const html = page();
    expect(html).toContain('Що мені зробити?');
    expect(html).toContain('Налаштувати');
    expect(html).toContain('Швидкий запуск');
    expect(html).not.toContain('What should I do?');
  });
});
