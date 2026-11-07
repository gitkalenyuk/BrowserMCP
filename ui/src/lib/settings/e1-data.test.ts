import { describe, expect, it } from 'vitest';
import { areaLabel, bytesLabel, diagnostics, fileUrl, groupRules, share, siteOrigin, sinceFor, uptimeParts } from './e1-data';
import type { About, PermissionRule } from './e1-types';

describe('clear browsing data', () => {
  it('turns a time range into `since`', () => {
    const now = 1_700_000_000_000;
    expect(sinceFor('hour', now)).toBe(now - 3_600_000);
    expect(sinceFor('week', now)).toBe(now - 7 * 86_400_000);
    expect(sinceFor('all', now)).toBeUndefined();
  });
});

describe('storage usage', () => {
  it('formats sizes with localized units', () => {
    expect(bytesLabel(0)).toBe('0 byte');
    expect(bytesLabel(1536)).toBe('1.5 kB');
    expect(bytesLabel(412_000_000)).toBe('393 MB');
    expect(bytesLabel(1536, 'uk')).toMatch(/^1,5\s[кК]Б$/);
  });

  it('labels profiles, known folders and the rest', () => {
    expect(areaLabel({ id: 'profiles/work' })).toEqual({ kind: 'profile', profile: 'work' });
    expect(areaLabel({ id: 'db' })).toEqual({ kind: 'known', key: 'db' });
    expect(areaLabel({ id: '(files)' })).toEqual({ kind: 'known', key: 'files' });
    expect(areaLabel({ id: 'mystery' })).toEqual({ kind: 'other', name: 'mystery' });
  });

  it('bar widths: a sliver for small areas, nothing for empty ones', () => {
    expect(share(50, 100)).toBe(50);
    expect(share(1, 10_000)).toBe(1);
    expect(share(0, 100)).toBe(0);
    expect(share(5, 0)).toBe(0);
  });
});

describe('site permissions', () => {
  const rules: PermissionRule[] = [
    { origin: 'https://b.test', kind: 'midi', allow: false },
    { origin: 'https://b.test', kind: 'camera', allow: true },
    { origin: '*', kind: 'notifications', allow: false },
    { origin: 'https://a.test', kind: 'geolocation', allow: true },
  ];

  it('groups by site, every site first, common kinds first', () => {
    const g = groupRules(rules);
    expect(g.map((s) => s.origin)).toEqual(['*', 'https://a.test', 'https://b.test']);
    expect(g[2].rules.map((r) => r.kind)).toEqual(['camera', 'midi']);
  });

  it('reads a typed site as an origin', () => {
    expect(siteOrigin('example.com')).toBe('https://example.com');
    expect(siteOrigin('http://Example.com:8080/x')).toBe('http://example.com:8080');
    expect(siteOrigin(' * ')).toBe('*');
    expect(siteOrigin('localhost')).toBe('https://localhost');
    expect(siteOrigin('nodot')).toBeNull();
    expect(siteOrigin('ftp://x.test')).toBeNull();
    expect(siteOrigin('')).toBeNull();
  });
});

describe('about', () => {
  const about: About = {
    name: 'BrowserMCP',
    version: '0.1.0',
    cef_version: '156.0.15+g1+chromium-156.0.8078.80',
    chromium_version: '156.0.8078.80',
    os: 'windows x86_64',
    pid: 7,
    uptime_secs: 3700,
    exe_dir: 'C:\\B',
    data_dir: 'C:\\B\\data',
    config_dir: 'C:\\B\\data\\config',
    logs_dir: 'C:\\B\\data\\logs',
    api: { port: 47800, mcp: 'http://127.0.0.1:47800/mcp', rest: 'r', ws: 'w' },
    licenses: [],
  };

  it('diagnostics name every version, folder and endpoint', () => {
    const text = diagnostics(about, { connected: true, uiLocale: 'uk' });
    expect(text).toContain('BrowserMCP 0.1.0');
    expect(text).toContain('Chromium 156.0.8078.80');
    expect(text).toContain('Data folder: C:\\B\\data');
    expect(text).toContain('API port 47800: MCP http://127.0.0.1:47800/mcp');
    expect(text).toContain('UI connected: yes');
    expect(text).toContain('UI language: uk');
  });

  it('uptime parts and file URLs', () => {
    expect(uptimeParts(3700)).toEqual({ d: 0, h: 1, m: 1 });
    expect(uptimeParts(90_061)).toEqual({ d: 1, h: 1, m: 1 });
    expect(fileUrl('C:\\Program Files\\B\\CREDITS.html')).toBe('file:///C:/Program%20Files/B/CREDITS.html');
  });
});
