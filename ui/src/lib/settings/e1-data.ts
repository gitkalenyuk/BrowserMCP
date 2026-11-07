// Logic of Settings → Site settings, Data & storage and About (pure, unit-tested).

import type { About, PermissionKind, PermissionRule, UsageArea } from './e1-types';

// ---- clear browsing data ----------------------------------------------------------------

export const TIME_RANGES = ['hour', 'day', 'week', 'month', 'all'] as const;
export type TimeRange = (typeof TIME_RANGES)[number];

const RANGE_MS: Record<Exclude<TimeRange, 'all'>, number> = {
  hour: 3_600_000,
  day: 86_400_000,
  week: 7 * 86_400_000,
  month: 28 * 86_400_000,
};

/** `since` for browsing_data.clear (Unix ms), `undefined` = all time. */
export function sinceFor(range: TimeRange, now = Date.now()): number | undefined {
  return range === 'all' ? undefined : now - RANGE_MS[range];
}

export const CLEAR_PARTS = ['history', 'downloads', 'cookies', 'cache', 'site_data'] as const;
export type ClearPart = (typeof CLEAR_PARTS)[number];

// ---- sizes ----------------------------------------------------------------------------------

/** `1536` → "1.5 KB" (localized unit names, e.g. "1,5 КБ"). */
export function bytesLabel(bytes: number, locale = 'en'): string {
  const units = ['byte', 'kilobyte', 'megabyte', 'gigabyte', 'terabyte'] as const;
  let value = Math.max(0, bytes);
  let i = 0;
  while (value >= 1024 && i < units.length - 1) {
    value /= 1024;
    i++;
  }
  return new Intl.NumberFormat(locale, {
    style: 'unit',
    unit: units[i],
    unitDisplay: 'short',
    maximumFractionDigits: i === 0 || value >= 100 ? 0 : 1,
  }).format(value);
}

/** Area ids of browsing_data.usage that have their own label (`settings.data.area.<id>`). */
export const KNOWN_AREAS = [
  'db',
  'config',
  'downloads',
  'recordings',
  'videos',
  'har',
  'scripts',
  'plugins',
  'userscripts',
  'runs',
  'logs',
  'runtime',
  'filters',
  'extract-cache',
  '(files)',
] as const;

/** How to label an area: a profile (with its id), a known folder, or the bare folder name. */
export function areaLabel(area: Pick<UsageArea, 'id'>): { kind: 'profile'; profile: string } | { kind: 'known'; key: string } | { kind: 'other'; name: string } {
  if (area.id.startsWith('profiles/')) return { kind: 'profile', profile: area.id.slice('profiles/'.length) };
  if ((KNOWN_AREAS as readonly string[]).includes(area.id)) return { kind: 'known', key: area.id === '(files)' ? 'files' : area.id };
  return { kind: 'other', name: area.id };
}

/** Share of the total in percent (for the bars), at least a sliver for non-empty areas. */
export function share(bytes: number, total: number): number {
  if (total <= 0 || bytes <= 0) return 0;
  return Math.max(1, Math.round((bytes / total) * 100));
}

// ---- site permissions -----------------------------------------------------------------------

/** The kinds a person usually cares about first (the rest follow in the core's order). */
export const COMMON_KINDS: PermissionKind[] = ['camera', 'microphone', 'geolocation', 'notifications', 'clipboard', 'screen_capture'];

export interface SiteRules {
  origin: string;
  rules: PermissionRule[];
}

/** Rules grouped by site: "every site" (`*`) first, then by origin; common kinds first. */
export function groupRules(rules: PermissionRule[]): SiteRules[] {
  const by = new Map<string, PermissionRule[]>();
  for (const r of rules) by.set(r.origin, [...(by.get(r.origin) ?? []), r]);
  const rank = (k: PermissionKind) => {
    const i = COMMON_KINDS.indexOf(k);
    return i < 0 ? COMMON_KINDS.length : i;
  };
  return [...by.entries()]
    .sort(([a], [b]) => (a === '*' ? -1 : b === '*' ? 1 : a.localeCompare(b)))
    .map(([origin, list]) => ({ origin, rules: [...list].sort((x, y) => rank(x.kind) - rank(y.kind) || x.kind.localeCompare(y.kind)) }));
}

/** `example.com` / `https://example.com/x` → `https://example.com`; `*` stays; junk → null. */
export function siteOrigin(text: string): string | null {
  const t = text.trim();
  if (t === '*') return '*';
  if (!t) return null;
  try {
    const u = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(t) ? t : `https://${t}`);
    if (u.protocol !== 'https:' && u.protocol !== 'http:') return null;
    if (!u.hostname.includes('.') && u.hostname !== 'localhost') return null;
    return u.origin;
  } catch {
    return null;
  }
}

// ---- about ----------------------------------------------------------------------------------

/** "3 d 4 h", "2 h 5 min", "12 min" (units translated by the caller's labels). */
export function uptimeParts(secs: number): { d: number; h: number; m: number } {
  const s = Math.max(0, Math.floor(secs));
  return { d: Math.floor(s / 86_400), h: Math.floor((s % 86_400) / 3600), m: Math.floor((s % 3600) / 60) };
}

/** Plain-text diagnostics for bug reports ("Copy diagnostics"). English on purpose: it is read by developers. */
export function diagnostics(about: About, extra: { userAgent?: string; uiLocale?: string; connected?: boolean } = {}): string {
  const lines = [
    `${about.name} ${about.version}`,
    `CEF ${about.cef_version || 'unknown'}`,
    `Chromium ${about.chromium_version || 'unknown'}`,
    `OS ${about.os}`,
    `PID ${about.pid}, uptime ${about.uptime_secs} s`,
    `Exe folder: ${about.exe_dir}`,
    `Data folder: ${about.data_dir}`,
    `Logs folder: ${about.logs_dir}`,
    `API port ${about.api.port}: MCP ${about.api.mcp}, REST ${about.api.rest}, WS ${about.api.ws}`,
  ];
  if (extra.connected !== undefined) lines.push(`UI connected: ${extra.connected ? 'yes' : 'no'}`);
  if (extra.uiLocale) lines.push(`UI language: ${extra.uiLocale}`);
  if (extra.userAgent) lines.push(`User agent: ${extra.userAgent}`);
  return lines.join('\n');
}

/** `C:\x\CREDITS.html` → `file:///C:/x/CREDITS.html` (to open a license file in a tab). */
export function fileUrl(path: string): string {
  const slashed = path.replace(/\\/g, '/');
  const encoded = slashed
    .split('/')
    .map((part, i) => (i === 0 && /^[a-z]:$/i.test(part) ? part : encodeURIComponent(part)))
    .join('/');
  return `file:///${encoded.replace(/^\/+/, '')}`;
}
