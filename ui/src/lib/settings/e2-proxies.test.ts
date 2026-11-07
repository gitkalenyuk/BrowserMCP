import { describe, expect, it } from 'vitest';
import type { ProfileEntry } from '../api/types';
import type { PoolProxy, ProfileRoutes, ProxyPool } from '../api/types-e2-settings';
import {
  buildProxyUrl,
  emptyProxyForm,
  isMasked,
  parseProxyList,
  poolHealth,
  profileRouteRows,
  profilesUsingPool,
  proxyFormErrors,
  routeChoices,
  ruleErrors,
} from './e2-proxies';

const profile = (id: string, extra: Partial<ProfileEntry> = {}): ProfileEntry => ({
  id,
  label: id,
  color: null,
  proxy: null,
  proxy_server: null,
  accept_language: null,
  identity: null,
  stored: true,
  open: false,
  incognito: false,
  tabs: [],
  launch: false,
  ...extra,
});

describe('proxy form', () => {
  it('validates host, port and login', () => {
    expect(proxyFormErrors(emptyProxyForm())).toEqual({ host: 'settings.proxies.err.hostEmpty', port: 'settings.proxies.err.port' });
    const ok = { scheme: 'socks5' as const, host: 'de1.example.com', port: '1080', user: '', pass: '' };
    expect(proxyFormErrors(ok)).toEqual({});
    expect(proxyFormErrors({ ...ok, host: 'socks5://x' }).host).toBe('settings.proxies.err.hostBad');
    expect(proxyFormErrors({ ...ok, port: '0' }).port).toBe('settings.proxies.err.port');
    expect(proxyFormErrors({ ...ok, port: '65536' }).port).toBe('settings.proxies.err.port');
    expect(proxyFormErrors({ ...ok, pass: 'x' }).user).toBe('settings.proxies.err.userForPass');
  });

  it('builds URLs with encoded logins and bracketed IPv6', () => {
    expect(buildProxyUrl({ scheme: 'http', host: ' 1.2.3.4 ', port: '3128', user: '', pass: '' })).toBe('http://1.2.3.4:3128');
    expect(buildProxyUrl({ scheme: 'socks5', host: 'h', port: '1080', user: 'u@x', pass: 'p:ss/1' })).toBe('socks5://u%40x:p%3Ass%2F1@h:1080');
    expect(buildProxyUrl({ scheme: 'socks5', host: '2001:db8::1', port: '1080', user: 'u', pass: '' })).toBe('socks5://u@[2001:db8::1]:1080');
  });

  it('reads pasted lists', () => {
    expect(parseProxyList('  a:1 \n\n# comment\nb:2:u:p\r\na:1\n')).toEqual(['a:1', 'b:2:u:p']);
  });

  it('recognises masked URLs', () => {
    expect(isMasked('socks5://u:***@h:1080')).toBe(true);
    expect(isMasked('socks5://u:pw@h:1080')).toBe(false);
    expect(isMasked('pool:de')).toBe(false);
  });
});

describe('pools', () => {
  const px = (healthy: boolean | null, latency: number | null, at: number | null): PoolProxy => ({
    proxy: 'x',
    healthy,
    latency_ms: latency,
    checked_at: at,
    error: null,
  });

  it('summarises health', () => {
    const h = poolHealth([px(true, 300, 10), px(true, 100, 30), px(true, 200, 20), px(false, null, 40), px(null, null, null)]);
    expect(h).toEqual({ total: 5, healthy: 3, unhealthy: 1, unchecked: 1, medianMs: 200, lastCheck: 40 });
    expect(poolHealth([])).toMatchObject({ total: 0, medianMs: null, lastCheck: null });
  });

  it('finds the profiles that use a pool (proxy, default route or a rule)', () => {
    const profiles = [profile('a', { proxy: 'pool:de' }), profile('b'), profile('c')];
    const routes: ProfileRoutes[] = [
      { profile: 'b', rules: [{ match: '*.x', via: 'pool:de' }], default: 'direct' },
      { profile: 'c', rules: [], default: 'pool:us' },
    ];
    expect(profilesUsingPool('de', profiles, routes)).toEqual(['a', 'b']);
    expect(profilesUsingPool('us', profiles, routes)).toEqual(['c']);
    expect(profilesUsingPool('none', profiles, routes)).toEqual([]);
  });

  it('lists every stored profile with its route', () => {
    const rows = profileRouteRows(
      [profile('a', { label: 'A', proxy: 'http://p:1' }), profile('tmp', { incognito: true }), profile('b')],
      [{ profile: 'a', rules: [{ match: 'x', via: 'direct' }], default: 'http://p:1' }],
    );
    expect(rows).toEqual([
      { id: 'a', label: 'A', via: 'http://p:1', rules: 1 },
      { id: 'b', label: 'b', via: 'direct', rules: 0 },
    ]);
  });

  it('offers direct, every pool and what is in use', () => {
    const pools = [{ name: 'de' }, { name: 'us' }] as ProxyPool[];
    expect(routeChoices(pools, ['direct', 'socks5://u:***@h:1', 'pool:de'])).toEqual(['direct', 'pool:de', 'pool:us', 'socks5://u:***@h:1']);
  });
});

describe('ruleErrors', () => {
  it('flags empty patterns, bad regexes and missing routes', () => {
    expect(
      ruleErrors([
        { match: '*.example.com', via: 'direct' },
        { match: ' ', via: 'direct' },
        { match: 're:([', via: 'direct' },
        { match: 're:\\.ru/', via: '' },
      ]),
    ).toEqual({ 1: 'settings.proxies.rules.errMatch', 2: 'settings.proxies.rules.errRegex', 3: 'settings.proxies.rules.errVia' });
  });
});
