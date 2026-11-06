import { describe, expect, it } from 'vitest';
import type { GovernorLimits, HintsConfig } from '../api/types-settings';
import {
  addSite,
  governorErrors,
  governorForm,
  governorPatch,
  hintKeyLabel,
  hintsErrors,
  hintsForm,
  hintsPatch,
  idleLabel,
  mbLabel,
  minToSecs,
  nextTriState,
  normalizeHost,
  policyRows,
  secsToMin,
  topCounts,
  withProfile,
} from './forms';
import { parseSettingsRequest } from './open';

const limits: GovernorLimits = {
  profile: 'normal',
  enabled: true,
  freeze_after_secs: 300,
  discard_after_secs: 1800,
  max_live_tabs: 20,
  busy_secs: 30,
  pressure_free_pct: 15,
  tick_secs: 15,
};

describe('performance form', () => {
  it('converts minutes and seconds', () => {
    expect(secsToMin(300)).toBe(5);
    expect(secsToMin(90)).toBe(1.5);
    expect(minToSecs(1.5)).toBe(90);
    expect(minToSecs(0)).toBe(0);
  });

  it('sends only the changed limits, in seconds', () => {
    const f = governorForm(limits);
    expect(f).toEqual({ enabled: true, profile: 'normal', freezeMin: 5, discardMin: 30, maxLiveTabs: 20 });
    expect(governorPatch(limits, f)).toEqual({});
    expect(governorPatch(limits, { ...f, freezeMin: 2.5, maxLiveTabs: 0 })).toEqual({ freeze_after_secs: 150, max_live_tabs: 0 });
    // Picking a profile fills in its times, which then are sent as well.
    expect(governorPatch(limits, withProfile(f, 'low'))).toEqual({
      profile: 'low',
      freeze_after_secs: 120,
      discard_after_secs: 600,
      max_live_tabs: 8,
    });
    expect(governorPatch(limits, withProfile(f, 'normal'))).toEqual({});
  });

  it('validates the form', () => {
    const f = governorForm(limits);
    expect(governorErrors(f)).toEqual({});
    expect(governorErrors({ ...f, freezeMin: -1 }).freezeMin).toMatch(/0 or more/);
    expect(governorErrors({ ...f, discardMin: 2000 }).discardMin).toMatch(/1440/);
    expect(governorErrors({ ...f, discardMin: 2 }).discardMin).toMatch(/longer/);
    expect(governorErrors({ ...f, discardMin: 0 })).toEqual({}); // never discard is fine
    expect(governorErrors({ ...f, maxLiveTabs: 2.5 }).maxLiveTabs).toBeDefined();
  });

  it('formats idle times and sizes', () => {
    expect(idleLabel(42)).toBe('42 s');
    expect(idleLabel(720)).toBe('12 min');
    expect(idleLabel(3900)).toBe('1 h 5 min');
    expect(idleLabel(7200)).toBe('2 h');
    expect(mbLabel(null)).toBe('—');
    expect(mbLabel(312.4)).toBe('312 MB');
    expect(mbLabel(16_303)).toBe('15.9 GB');
  });
});

describe('sites', () => {
  it('normalizes hosts from what people paste', () => {
    expect(normalizeHost('https://www.Example.com/a?b')).toBe('example.com');
    expect(normalizeHost('news.example.com:8080/x')).toBe('news.example.com');
    expect(normalizeHost('*.github.com')).toBe('github.com');
    expect(normalizeHost('*')).toBe('*');
    expect(normalizeHost('')).toBeNull();
    expect(normalizeHost('not a host')).toBeNull();
    expect(normalizeHost('-bad-.com')).toBeNull();
  });

  it('adds sites without duplicates', () => {
    expect(addSite(['b.com'], 'https://a.com/')).toEqual({ list: ['a.com', 'b.com'], error: null });
    expect(addSite(['a.com'], 'www.a.com').error).toMatch(/already/);
    expect(addSite([], '*').error).toMatch(/like example/);
  });

  it('cycles policy cells and sorts rows', () => {
    expect(nextTriState(undefined)).toBe(false);
    expect(nextTriState(false)).toBe(true);
    expect(nextTriState(true)).toBeNull();
    expect(policyRows({ 'b.com': {}, '*': {}, 'a.com': { js: false } }).map((r) => r.host)).toEqual(['*', 'a.com', 'b.com']);
    expect(topCounts({ a: 1, b: 5, c: 5, d: 2 }, 3)).toEqual([
      { key: 'b', count: 5 },
      { key: 'c', count: 5 },
      { key: 'd', count: 2 },
    ]);
  });
});

describe('link hints form', () => {
  const config: HintsConfig = {
    enabled: true,
    alphabet: 'asdfghjkl',
    keys: { click: 'f', newtab: 'F', focus: null, copy: null },
    disabled_sites: [],
  };

  it('validates keys and the alphabet', () => {
    const f = hintsForm(config);
    expect(hintsErrors(f)).toEqual({});
    expect(hintsErrors({ ...f, keys: { ...f.keys, copy: 'f' } })['key.copy']).toMatch(/Same key as "Click a link"/);
    expect(hintsErrors({ ...f, keys: { ...f.keys, focus: 'ab' } })['key.focus']).toMatch(/One character/);
    expect(hintsErrors({ ...f, alphabet: 'aa' }).alphabet).toBeDefined();
    expect(hintsErrors({ ...f, alphabet: 'ABC' }).alphabet).toBeDefined();
  });

  it('sends keys whole, empty as null, only when they changed', () => {
    const f = hintsForm(config);
    expect(hintsPatch(config, f)).toEqual({});
    expect(hintsPatch(config, { ...f, enabled: false })).toEqual({ enabled: false });
    expect(hintsPatch(config, { ...f, keys: { ...f.keys, newtab: '', copy: 'y' } })).toEqual({
      keys: { click: 'f', newtab: null, focus: null, copy: 'y' },
    });
    expect(hintKeyLabel('F')).toBe('Shift+F');
    expect(hintKeyLabel('f')).toBe('f');
  });
});

describe('settings page request', () => {
  it('accepts a fresh, known section only', () => {
    const now = 1_000_000;
    expect(parseSettingsRequest({ section: 'notifications', at: now - 1000 }, now)).toBe('notifications');
    expect(parseSettingsRequest({ section: 'notifications', at: now - 60_000 }, now)).toBeNull();
    expect(parseSettingsRequest({ section: 'evil', at: now }, now)).toBeNull();
    expect(parseSettingsRequest(null, now)).toBeNull();
  });
});
