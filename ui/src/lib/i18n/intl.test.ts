import { afterEach, describe, expect, it } from 'vitest';
import { settings } from '../settings.svelte';
import { formatList, formatTime, formatWhen, intlLocale, relativeTime } from './intl';

const lang = (language: 'en' | 'uk' | 'system') => settings.update({ language });
afterEach(() => lang('system'));

const MIN = 60_000;
const NOW = new Date(2026, 10, 7, 14, 5).getTime();

describe('intlLocale', () => {
  it('keeps the system region when it speaks the UI language', () => {
    expect(intlLocale('en', ['en-GB', 'uk-UA'])).toBe('en-GB');
    expect(intlLocale('uk', ['en-GB', 'uk-UA'])).toBe('uk-UA');
  });

  it('falls back to en-US / uk-UA', () => {
    expect(intlLocale('en', ['uk-UA'])).toBe('en-US');
    expect(intlLocale('uk', ['de-DE'])).toBe('uk-UA');
    // A bare language tag has no region to keep.
    expect(intlLocale('uk', ['uk'])).toBe('uk-UA');
  });

  it('follows the language setting', () => {
    expect(intlLocale()).toBe('en-US'); // tests pin the system to en-US
    lang('uk');
    expect(intlLocale()).toBe('uk-UA');
  });
});

describe('formatList', () => {
  it('joins with the language’s "and"', () => {
    expect(formatList(['a', 'b'])).toBe('a and b');
    lang('uk');
    expect(formatList(['a', 'b', 'c'])).toBe('a, b і c');
    expect(formatList(['a', 'b'], 'disjunction')).toBe('a або b');
  });
});

describe('relativeTime', () => {
  it('English', () => {
    expect(relativeTime(NOW - 3000, NOW)).toBe('now');
    expect(relativeTime(NOW - 5 * MIN, NOW)).toBe('5 min. ago');
    expect(relativeTime(NOW + 2 * 60 * MIN, NOW)).toBe('in 2 hr.');
    expect(relativeTime(NOW - 24 * 60 * MIN, NOW, 'long')).toBe('yesterday');
  });

  it('Ukrainian', () => {
    lang('uk');
    expect(relativeTime(NOW - 3000, NOW)).toBe('зараз');
    expect(relativeTime(NOW - 5 * MIN, NOW)).toBe('5 хв тому');
    expect(relativeTime(NOW - 24 * 60 * MIN, NOW)).toBe('учора');
    expect(relativeTime(NOW - 3 * 7 * 24 * 60 * MIN, NOW, 'long')).toBe('3 тижні тому');
  });
});

describe('formatWhen', () => {
  it('only the time for today, the day otherwise', () => {
    lang('uk');
    expect(formatWhen(NOW, NOW)).toBe(formatTime(NOW));
    expect(formatWhen(NOW, NOW)).toBe('14:05');
    expect(formatWhen(NOW - 2 * 24 * 60 * MIN, NOW)).toBe('5 лист., 14:05');
  });
});
