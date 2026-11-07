// The browser window's strings follow the language setting: spot checks of the helpers that
// build text in code (not markup), and a scan that every literal `t('key')` in the UI exists.
import { afterEach, describe, expect, it } from 'vitest';
import { COMMANDS, commandTitle } from '../commands';
import { stateLabel } from '../data/format';
import { CLEAR_RANGES, dayLabel, startOfDay } from '../data/history';
import { ROOT_TITLES } from '../data/tree';
import { resultSummary } from '../data/wizard';
import { findLabel } from '../find';
import { TYPE_CHIPS, buildRule, EMPTY_RULE } from '../net/logic';
import { describeStep } from '../record/logic';
import { cronPreview, describeCron, parseCron } from '../runs/cron';
import { formatDuration, pageRange } from '../runs/logic';
import { endEntry } from '../scripts/logic';
import { settings } from '../settings.svelte';
import { permissionText, proxyLabel } from '../state/workspaces';
import { fallbackTitle } from '../urls';
import { DICTS, tp } from './index.svelte';

const lang = (language: 'en' | 'uk' | 'system') => settings.update({ language });
afterEach(() => lang('system'));

describe('strings built in code switch language', () => {
  it('commands (palette, keyboard settings)', () => {
    const settingsCmd = COMMANDS.find((c) => c.id === 'settings.open')!;
    expect(settingsCmd.title).toBe('Settings');
    lang('uk');
    // A getter: the same object follows the language.
    expect(settingsCmd.title).toBe('Налаштування');
    expect(commandTitle('tab.new')).toBe('Нова вкладка');
    expect(COMMANDS.every((c) => !c.title.startsWith('commands.'))).toBe(true);
  });

  it('chrome helpers', () => {
    lang('uk');
    expect(permissionText(['camera', 'microphone'])).toBe('користуватися камерою і користуватися мікрофоном');
    expect(permissionText([])).toBe('скористатися функцією браузера');
    expect(proxyLabel(null)).toBe('Без проксі');
    expect(findLabel('x', 0, 0)).toBe('Нічого не знайдено');
    expect(fallbackTitle('bmcp://ui/newtab.html')).toBe('Нова вкладка');
  });

  it('library helpers', () => {
    const now = new Date(2026, 10, 7, 12).getTime();
    lang('uk');
    expect(stateLabel('complete')).toBe('Готово');
    expect(stateLabel('weird')).toBe('weird');
    expect(dayLabel(startOfDay(now), now)).toBe('Сьогодні');
    expect(dayLabel(new Date(2026, 10, 3).getTime(), now)).toBe('вівторок, 3 листопада 2026 р.');
    expect(CLEAR_RANGES[0].label).toBe('Остання година');
    expect(ROOT_TITLES[1]).toBe('Панель закладок');
    expect(resultSummary({ part: 'history', ok: true, imported: 12, skipped: 3 } as never)).toBe('імпортовано: 12, пропущено: 3');
  });

  it('panels', () => {
    lang('uk');
    expect(TYPE_CHIPS.find((c) => c.id === 'img')!.label).toBe('Зображення');
    expect(buildRule({ ...EMPTY_RULE, kind: 'rewrite_body' })).toEqual({ error: 'Вкажіть текст для пошуку.' });
    expect(describeStep({ seq: 1, ts_ms: 0, action: 'click', source: 'human', target: { role: 'button', name: 'OK' } } as never).verb).toBe('Клік');
    expect(formatDuration(125_000)).toBe('2 хв 5 с');
    expect(formatDuration(3_720_000)).toBe('1 год 2 хв');
    expect(pageRange({ cursors: [null, 50], index: 1 }, 50, 50, 230)).toBe('51–100 з 230');
    expect(endEntry(true, 120, null, false).text).toBe('виконано за 120 мс');
  });
});

describe('cron in Ukrainian', () => {
  const d = (spec: string) => describeCron(parseCron(spec));

  it('describes schedules', () => {
    lang('uk');
    expect(d('0 9 * * Mon-Fri')).toBe('О 09:00 у будні');
    expect(d('30 8,12 * * *')).toBe('О 08:30 і 12:30 щодня');
    expect(d('*/15 9-17 * * Mon-Fri')).toBe('Раз на 15 хв, з 09:00 до 17:59 у будні');
    expect(d('* * * * *')).toBe('Щохвилини');
    expect(d('0 7 * * Mon,Wed,Fri')).toBe('О 07:00 у понеділок, середу і п’ятницю');
    expect(d('@yearly')).toBe('О 00:00 1 числа кожного місяця у січні');
  });

  it('weekday spans, «об 11:00» and day-of-month-or-weekday', () => {
    lang('uk');
    expect(d('0 9 * * Mon-Wed')).toBe('О 09:00 з понеділка по середу');
    expect(d('0 9 * * Mon-Wed,Sat')).toBe('О 09:00 у понеділок, вівторок, середу і суботу');
    expect(d('0 11 * * *')).toBe('Об 11:00 щодня');
    expect(d('0 9 13 * Mon,Fri')).toBe('О 09:00 13 числа кожного місяця, але лише якщо це понеділок або п’ятниця');
  });

  it('errors and hints', () => {
    lang('uk');
    expect(() => parseCron('61 * * * *')).toThrow('хвилини: 61 поза межами 0-59');
    const bad = cronPreview('0 9 * * 0');
    expect(bad.ok).toBe(false);
    if (!bad.ok) expect(bad.hint).toMatch(/1 = неділя/);
    const numeric = cronPreview('0 9 * * 1-5', new Date(2026, 10, 7));
    expect(numeric.ok && numeric.warning).toMatch(/Нд–Чт/);
  });

  it('English stays the default', () => {
    expect(d('0 7 * * Mon,Wed,Fri')).toBe('At 07:00 on Monday, Wednesday and Friday');
    expect(d('0 9 * * Mon-Wed')).toBe('At 09:00 on Monday–Wednesday');
    expect(d('0 11 * * *')).toBe('At 11:00 every day');
    expect(d('0 9 13 * Mon,Fri')).toBe('At 09:00 on day 13 of the month, only if it is a Monday or Friday');
  });
});

describe('every literal key used by the UI exists', () => {
  // Every UI source except tests and lib/i18n itself (its docs show sample keys).
  const sources = import.meta.glob<string>(['../../**/*.{ts,svelte}', '!../../**/*.test.ts', '!./**'], {
    query: '?raw',
    import: 'default',
    eager: true,
  });

  it('t() and tp() keys', () => {
    const missing: string[] = [];
    for (const [file, src] of Object.entries(sources)) {
      for (const m of src.matchAll(/\b(t|tr|tp)\(\s*'([a-z][\w.-]*)'/g)) {
        const key = m[1] === 'tp' ? `${m[2]}.other` : m[2];
        if (!(key in DICTS.en)) missing.push(`${file}: ${m[2]}`);
      }
    }
    expect(missing).toEqual([]);
  });
});

describe('counted strings agree with the number', () => {
  it('in English', () => {
    expect(tp('scripts.bytes', 1)).toBe('1 byte');
    expect(tp('scripts.bytes', 2)).toBe('2 bytes');
    expect(tp('runs.detail.progress', 1, { done: 0 })).toBe('0 of 1 row done');
    expect(tp('runs.detail.sent', 1, { rows: '3 rows' })).toBe('Sent 3 rows in 1 webhook request');
  });

  it('in Ukrainian', () => {
    lang('uk');
    expect(['1', '3', '5', '21'].map((n) => tp('scripts.bytes', Number(n)))).toEqual(['1 байт', '3 байти', '5 байтів', '21 байт']);
    expect(tp('runs.detail.progress', 1, { done: 0 })).toBe('Виконано 0 з 1 рядка даних');
    expect(tp('runs.detail.progress', 5, { done: 2 })).toBe('Виконано 2 з 5 рядків даних');
    expect(tp('runs.detail.sent', 1, { rows: '3 рядки' })).toBe('Надіслано 3 рядки в 1 запиті до вебхука');
    expect(tp('runs.detail.sent', 4, { rows: '3 рядки' })).toBe('Надіслано 3 рядки у 4 запитах до вебхука');
  });
});
