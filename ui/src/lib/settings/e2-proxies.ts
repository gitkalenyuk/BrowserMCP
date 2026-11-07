// Settings → Proxies: building proxy URLs from a form, proxy lists, health, who uses what,
// and routing rules. Pure functions (tested in e2-proxies.test.ts).
//
// The core (crates/identity/src/proxy/config.rs) accepts http:// and socks5:// upstreams
// (HTTPS sites work through both, via CONNECT); TLS to the proxy itself (https://) and SOCKS4
// are refused there, so the form does not offer them.

import type { ProfileEntry } from '../api/types';
import type { PoolProxy, ProfileRoutes, ProxyPool, ProxyRule } from '../api/types-e2-settings';

export type ProxyScheme = 'http' | 'socks5';

export interface ProxyForm {
  scheme: ProxyScheme;
  host: string;
  port: string;
  user: string;
  pass: string;
}

export const emptyProxyForm = (): ProxyForm => ({ scheme: 'http', host: '', port: '', user: '', pass: '' });

/** Errors per field, as i18n keys. */
export function proxyFormErrors(f: ProxyForm): Partial<Record<keyof ProxyForm, string>> {
  const e: Partial<Record<keyof ProxyForm, string>> = {};
  const host = f.host.trim();
  if (!host) e.host = 'settings.proxies.err.hostEmpty';
  else if (/[\s/@?#]/.test(host) || host.includes('://')) e.host = 'settings.proxies.err.hostBad';
  const port = f.port.trim();
  if (!/^\d+$/.test(port) || Number(port) < 1 || Number(port) > 65535) e.port = 'settings.proxies.err.port';
  if (f.pass && !f.user.trim()) e.user = 'settings.proxies.err.userForPass';
  return e;
}

/** `socks5://user:pass@host:port` (user and password percent-encoded, IPv6 in brackets). */
export function buildProxyUrl(f: ProxyForm): string {
  const host = f.host.trim();
  const hostPart = host.includes(':') && !host.startsWith('[') ? `[${host}]` : host;
  const user = f.user.trim();
  const auth = user ? `${encodeURIComponent(user)}${f.pass ? `:${encodeURIComponent(f.pass)}` : ''}@` : '';
  return `${f.scheme}://${auth}${hostPart}:${f.port.trim()}`;
}

/**
 * Lines of a pasted proxy list: trimmed, without blanks, `#` comments and duplicates.
 * Every form the core accepts is passed on as-is (it reports bad lines itself).
 */
export function parseProxyList(text: string): string[] {
  const seen = new Set<string>();
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (line && !line.startsWith('#')) seen.add(line);
  }
  return [...seen];
}

/** A URL shown by the core with the password replaced by `***`. */
export function isMasked(url: string): boolean {
  return /:\*\*\*@/.test(url);
}

export interface PoolHealth {
  total: number;
  healthy: number;
  unhealthy: number;
  unchecked: number;
  /** Median latency of the healthy proxies, ms. */
  medianMs: number | null;
  /** Unix seconds of the newest check. */
  lastCheck: number | null;
}

export function poolHealth(proxies: PoolProxy[]): PoolHealth {
  const healthy = proxies.filter((p) => p.healthy === true);
  const latencies = healthy
    .map((p) => p.latency_ms)
    .filter((n): n is number => typeof n === 'number')
    .sort((a, b) => a - b);
  const checks = proxies.map((p) => p.checked_at).filter((n): n is number => typeof n === 'number');
  return {
    total: proxies.length,
    healthy: healthy.length,
    unhealthy: proxies.filter((p) => p.healthy === false).length,
    unchecked: proxies.filter((p) => p.healthy == null).length,
    medianMs: latencies.length ? latencies[Math.floor((latencies.length - 1) / 2)] : null,
    lastCheck: checks.length ? Math.max(...checks) : null,
  };
}

/** `pool:NAME` — the route reference of a pool (profile proxy, rules, run.start). */
export const poolRef = (name: string) => `pool:${name}`;

/** Profiles whose proxy or routing rules use `pool`. */
export function profilesUsingPool(pool: string, profiles: ProfileEntry[], routes: ProfileRoutes[]): string[] {
  const ref = poolRef(pool);
  const users = new Set<string>();
  for (const p of profiles) if (p.proxy === ref) users.add(p.id);
  for (const r of routes) {
    if (r.default === ref || r.rules.some((rule) => rule.via === ref)) users.add(r.profile);
  }
  return [...users].sort();
}

/** One row of "which profile goes where": the default route plus the number of split rules. */
export interface ProfileRouteRow {
  id: string;
  label: string;
  via: string;
  rules: number;
}

export function profileRouteRows(profiles: ProfileEntry[], routes: ProfileRoutes[]): ProfileRouteRow[] {
  return profiles
    .filter((p) => !p.incognito)
    .map((p) => {
      const r = routes.find((x) => x.profile === p.id);
      return { id: p.id, label: p.label, via: r?.default ?? p.proxy ?? 'direct', rules: r?.rules.length ?? 0 };
    });
}

/** Errors per rule (index → i18n key). `re:` patterns must compile; vias must be non-empty. */
export function ruleErrors(rules: ProxyRule[]): Record<number, string> {
  const out: Record<number, string> = {};
  rules.forEach((r, i) => {
    const pattern = r.match.trim();
    if (!pattern) out[i] = 'settings.proxies.rules.errMatch';
    else if (pattern.startsWith('re:')) {
      try {
        new RegExp(pattern.slice(3), 'i');
      } catch {
        out[i] = 'settings.proxies.rules.errRegex';
      }
    }
    if (!out[i] && !r.via.trim()) out[i] = 'settings.proxies.rules.errVia';
  });
  return out;
}

/** Route choices for a rule / profile: direct, every pool, plus what is already in use. */
export function routeChoices(pools: ProxyPool[], current: string[] = []): string[] {
  const list = ['direct', ...pools.map((p) => poolRef(p.name))];
  for (const c of current) if (c && !list.includes(c)) list.push(c);
  return list;
}
