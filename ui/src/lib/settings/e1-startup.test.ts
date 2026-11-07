import { describe, expect, it } from 'vitest';
import { normalizePage, parsePages, startupErrors, startupForm, startupPatch } from './e1-startup';
import type { StartupConfig } from './e1-types';

const saved: StartupConfig = {
  on_startup: 'restore',
  pages: [],
  home_page: null,
  show_home_button: false,
  new_tab_url: null,
  new_tab_position: 'end',
  keep_window_on_last_tab: false,
};

describe('startup pages', () => {
  it('normalizes what people type, like the core', () => {
    expect(normalizePage('example.com')).toBe('https://example.com/');
    expect(normalizePage('localhost:3000')).toBe('http://localhost:3000/');
    expect(normalizePage('about:blank')).toBe('about:blank');
    expect(normalizePage('javascript:alert(1)')).toBeNull();
    expect(normalizePage('two words')).toBeNull();
  });

  it('one page per line, duplicates dropped, bad lines reported', () => {
    expect(parsePages('a.test\n\n  https://a.test/ \nb.test\nnot a url')).toEqual({
      pages: ['https://a.test/', 'https://b.test/'],
      bad: ['not a url'],
    });
  });
});

describe('startup form', () => {
  it('round-trips the saved settings without a change', () => {
    const f = startupForm(saved);
    expect(f.new_tab_mode).toBe('builtin');
    expect(startupPatch(saved, f)).toEqual({});
    expect(startupErrors(f)).toEqual({});
  });

  it('sends only what changed', () => {
    const f = startupForm(saved);
    f.on_startup = 'pages';
    f.pagesText = 'mail.test\nnews.test';
    f.show_home_button = true;
    f.home_page = 'home.test';
    f.new_tab_mode = 'blank';
    f.new_tab_position = 'after_active';
    expect(startupPatch(saved, f)).toEqual({
      on_startup: 'pages',
      pages: ['https://mail.test/', 'https://news.test/'],
      show_home_button: true,
      home_page: 'https://home.test/',
      new_tab_url: 'about:blank',
      new_tab_position: 'after_active',
    });
  });

  it('reads back a custom or blank new tab page', () => {
    expect(startupForm({ ...saved, new_tab_url: 'about:blank' }).new_tab_mode).toBe('blank');
    const f = startupForm({ ...saved, new_tab_url: 'https://start.test/' });
    expect(f.new_tab_mode).toBe('url');
    expect(f.new_tab_url).toBe('https://start.test/');
    // Back to the built-in page: "" clears the stored URL.
    f.new_tab_mode = 'builtin';
    expect(startupPatch({ ...saved, new_tab_url: 'https://start.test/' }, f)).toEqual({ new_tab_url: '' });
  });

  it('flags what the core would refuse', () => {
    const f = startupForm(saved);
    f.on_startup = 'pages';
    expect(startupErrors(f).pages).toBe('empty');
    f.pagesText = 'ok.test\nnot ok';
    expect(startupErrors(f).pages).toBe('bad');
    f.pagesText = Array.from({ length: 21 }, (_, i) => `p${i}.test`).join('\n');
    expect(startupErrors(f).pages).toBe('tooMany');
    f.pagesText = 'ok.test';
    f.home_page = 'javascript:x';
    f.new_tab_mode = 'url';
    expect(startupErrors(f)).toEqual({ home_page: 'bad', new_tab_url: 'empty' });
  });
});
