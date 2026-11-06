import { describe, expect, it } from 'vitest';
import type { ChromeProfile } from '../api/types-data';
import { offerImportOnFirstRun } from './library';
import {
  canStart,
  cleanPath,
  importArgs,
  initialWizard,
  problems,
  resultSummary,
  wizardReducer,
  type WizardEvent,
  type WizardState,
} from './wizard';

const DEFAULT: ChromeProfile = { name: 'Default', path: 'C:\\Chrome\\Default', has_bookmarks: true, has_history: true };
const EMPTY: ChromeProfile = { name: 'Profile 2', path: 'C:\\Chrome\\Profile 2', has_bookmarks: false, has_history: false };
const BM_ONLY: ChromeProfile = { name: 'Profile 1', path: 'C:\\Chrome\\Profile 1', has_bookmarks: true, has_history: false };

const run = (...events: WizardEvent[]): WizardState => events.reduce(wizardReducer, initialWizard());
const detected = (...profiles: ChromeProfile[]): WizardEvent => ({ type: 'detected', profiles, userDataDirs: ['C:\\Chrome'] });

describe('import wizard', () => {
  it('starts detecting, then preselects the first profile with data', () => {
    expect(initialWizard().step).toBe('detecting');
    const s = run(detected(EMPTY, DEFAULT));
    expect(s.step).toBe('choose');
    expect(s.profile).toBe(DEFAULT.path);
    expect(s.parts).toEqual({ browsing: true, passwords: false, cookies: false });
    expect(canStart(s)).toBe(true);
    expect(importArgs(s)).toEqual({ parts: ['bookmarks', 'history'], profile_dir: DEFAULT.path });
  });

  it('without Chrome data, bookmarks/history cannot be picked and nothing is ready', () => {
    const s = run(detected(EMPTY), { type: 'toggle', part: 'browsing', on: true });
    expect(s.parts.browsing).toBe(false);
    expect(canStart(s)).toBe(false);
    expect(problems(s)).toEqual(['Pick at least one thing to import.']);
    const failed = run({ type: 'detect_failed', error: 'no LOCALAPPDATA' });
    expect(failed.step).toBe('choose');
    expect(failed.detectError).toBe('no LOCALAPPDATA');
  });

  it('only imports the parts a profile has', () => {
    const s = run(detected(DEFAULT, BM_ONLY), { type: 'select_profile', path: BM_ONLY.path });
    expect(importArgs(s)).toEqual({ parts: ['bookmarks'], profile_dir: BM_ONLY.path });
    // Unknown profiles are ignored.
    expect(wizardReducer(s, { type: 'select_profile', path: 'C:\\nope' }).profile).toBe(BM_ONLY.path);
  });

  it('needs the CSV / JSON paths before passwords and cookies can start', () => {
    let s = run(detected(DEFAULT), { type: 'toggle', part: 'passwords', on: true }, { type: 'toggle', part: 'cookies', on: true });
    expect(problems(s)).toEqual([
      'Enter the path of the exported passwords CSV.',
      'Enter the path of the cookies JSON file.',
    ]);
    expect(wizardReducer(s, { type: 'start' }).step).toBe('choose');
    s = wizardReducer(s, { type: 'set_path', part: 'passwords', path: '  "C:\\Users\\me\\Chrome Passwords.csv" ' });
    s = wizardReducer(s, { type: 'set_path', part: 'cookies', path: 'C:\\c.json' });
    expect(importArgs(s)).toEqual({
      parts: ['bookmarks', 'history', 'passwords_csv', 'cookies_json'],
      profile_dir: DEFAULT.path,
      passwords_csv_path: 'C:\\Users\\me\\Chrome Passwords.csv',
      cookies_json_path: 'C:\\c.json',
    });
  });

  it('runs: choose -> running -> done -> back, ignoring events out of turn', () => {
    let s = run(detected(DEFAULT), { type: 'toggle', part: 'browsing', on: true });
    // Results before a run are ignored.
    expect(wizardReducer(s, { type: 'finished', results: [] }).step).toBe('choose');
    s = wizardReducer(s, { type: 'start' });
    expect(s.step).toBe('running');
    // No edits while running.
    expect(wizardReducer(s, { type: 'toggle', part: 'cookies', on: true }).parts.cookies).toBe(false);
    expect(wizardReducer(s, { type: 'skip' }).step).toBe('running');
    const results = [
      { part: 'bookmarks' as const, ok: true, imported: 12, skipped: 3, source: 'C:\\Chrome\\Default\\Bookmarks', error: null },
      { part: 'history' as const, ok: false, imported: 0, skipped: 0, source: 'C:\\Chrome\\Default\\History', error: 'database is locked' },
    ];
    s = wizardReducer(s, { type: 'finished', results });
    expect(s.step).toBe('done');
    expect(s.results.map(resultSummary)).toEqual(['12 imported, 3 skipped', 'database is locked']);
    s = wizardReducer(s, { type: 'back' });
    expect(s.step).toBe('choose');
    expect(s.results).toEqual([]);
  });

  it('a failed run goes back to choosing with the error', () => {
    const s = run(detected(DEFAULT), { type: 'start' }, { type: 'failed', error: 'Connection lost' });
    expect(s.step).toBe('choose');
    expect(s.error).toBe('Connection lost');
    expect(wizardReducer(s, { type: 'start' }).error).toBeNull();
  });

  it('can be skipped ("nothing") and reopened', () => {
    expect(run({ type: 'skip' }).step).toBe('skipped');
    const s = run(detected(DEFAULT), { type: 'skip' });
    expect(s.step).toBe('skipped');
    expect(wizardReducer(s, { type: 'back' }).step).toBe('choose');
  });

  it('cleans pasted paths', () => {
    expect(cleanPath('"C:\\a b\\x.csv"')).toBe('C:\\a b\\x.csv');
    expect(cleanPath(" 'x' ")).toBe('x');
    expect(cleanPath('  /home/me/x.csv ')).toBe('/home/me/x.csv');
  });
});

describe('offerImportOnFirstRun', () => {
  const api = (empty: boolean, fail = false) => ({
    call: (method: string) => {
      if (fail) return Promise.reject(new Error('down'));
      if (method === 'history.search') return Promise.resolve({ entries: empty ? [] : [{}] });
      return Promise.resolve({ folder: 1, bookmarks: [], folders: [] });
    },
  }) as never;
  const flag = (initial = false) => {
    let v = initial;
    return { get: () => v, set: () => (v = true), value: () => v };
  };

  it('opens the wizard once, on an empty database', async () => {
    const f = flag();
    let opened = 0;
    expect(await offerImportOnFirstRun(api(true), () => opened++, f)).toBe('opened');
    expect(await offerImportOnFirstRun(api(true), () => opened++, f)).toBe('already');
    expect(opened).toBe(1);
  });

  it('never offers when there is data, and retries after a failed check', async () => {
    const f = flag();
    expect(await offerImportOnFirstRun(api(false), () => {}, f)).toBe('not_empty');
    expect(f.value()).toBe(true);
    const g = flag();
    expect(await offerImportOnFirstRun(api(true, true), () => {}, g)).toBe('failed');
    expect(g.value()).toBe(false);
  });
});
