import { describe, expect, it } from 'vitest';
import { DICTS, format, resolveLocale, systemLocale } from './index.svelte';

const placeholders = (s: string) =>
  [...s.matchAll(/\{(\w+)\}/g)]
    .map((m) => m[1])
    .sort()
    .join();

describe('i18n dictionaries', () => {
  it('Ukrainian has every English key and nothing else', () => {
    expect(
      Object.keys(DICTS.en).filter((k) => !(k in DICTS.uk)),
      'missing in uk',
    ).toEqual([]);
    expect(
      Object.keys(DICTS.uk).filter((k) => !(k in DICTS.en)),
      'only in uk',
    ).toEqual([]);
  });

  it('translations keep the placeholders', () => {
    const bad = Object.keys(DICTS.en).filter((k) => k in DICTS.uk && placeholders(DICTS.en[k]) !== placeholders(DICTS.uk[k]));
    expect(bad).toEqual([]);
  });

  it('has no empty strings', () => {
    for (const loc of ['en', 'uk'] as const) {
      expect(
        Object.entries(DICTS[loc])
          .filter(([, v]) => !v.trim())
          .map(([k]) => k),
      ).toEqual([]);
    }
  });
});

describe('locale', () => {
  it('follows the system for "system"', () => {
    expect(systemLocale(['uk-UA', 'en-US'])).toBe('uk');
    expect(systemLocale(['de-DE', 'en-GB'])).toBe('en');
    expect(systemLocale([])).toBe('en');
    expect(resolveLocale('uk')).toBe('uk');
    expect(resolveLocale('en')).toBe('en');
  });

  it('fills placeholders and leaves unknown ones', () => {
    expect(format('Hi {name}, {n}', { name: 'Ann' })).toBe('Hi Ann, {n}');
  });
});
