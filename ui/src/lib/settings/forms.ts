// Small pure helpers of the Performance, Privacy and Link hints sections (unit-tested).

import type {
  GovernorConfigArgs,
  GovernorLimits,
  HintsConfig,
  HintsConfigArgs,
  SitePolicy,
  SitePolicyKey,
} from '../api/types-settings';

// ---- performance (crates/governor) -----------------------------------------------------------

export interface GovernorForm {
  enabled: boolean;
  profile: GovernorLimits['profile'];
  /** Minutes in the background before a tab is frozen / discarded (0 = never). */
  freezeMin: number;
  discardMin: number;
  /** 0 = no limit. */
  maxLiveTabs: number;
}

/** Seconds → minutes for the form (one decimal at most, so 90 s shows as 1.5). */
export function secsToMin(secs: number): number {
  return Math.round((secs / 60) * 10) / 10;
}

export function minToSecs(min: number): number {
  return Math.round(min * 60);
}

export function governorForm(l: GovernorLimits): GovernorForm {
  return {
    enabled: l.enabled,
    profile: l.profile,
    freezeMin: secsToMin(l.freeze_after_secs),
    discardMin: secsToMin(l.discard_after_secs),
    maxLiveTabs: l.max_live_tabs,
  };
}

/** Each profile's times (crates/governor `Profile::defaults`). */
export const PROFILE_DEFAULTS: Record<GovernorLimits['profile'], Pick<GovernorForm, 'freezeMin' | 'discardMin' | 'maxLiveTabs'>> = {
  low: { freezeMin: 2, discardMin: 10, maxLiveTabs: 8 },
  normal: { freezeMin: 5, discardMin: 30, maxLiveTabs: 20 },
  high: { freezeMin: 10, discardMin: 60, maxLiveTabs: 40 },
};

/** Pick a profile: its times replace the ones in the form. */
export function withProfile(f: GovernorForm, profile: GovernorLimits['profile']): GovernorForm {
  return { ...f, profile, ...PROFILE_DEFAULTS[profile] };
}

export function governorErrors(f: GovernorForm): Record<string, string> {
  const out: Record<string, string> = {};
  const minutes = (k: string, v: number) => {
    if (!Number.isFinite(v) || v < 0) out[k] = 'Minutes, 0 or more';
    else if (v > 24 * 60) out[k] = 'At most 1440 minutes (a day)';
  };
  minutes('freezeMin', f.freezeMin);
  minutes('discardMin', f.discardMin);
  if (!out.freezeMin && !out.discardMin && f.freezeMin > 0 && f.discardMin > 0 && f.discardMin < f.freezeMin) {
    out.discardMin = 'Discarding before freezing skips the freeze: use a longer time';
  }
  if (!Number.isInteger(f.maxLiveTabs) || f.maxLiveTabs < 0 || f.maxLiveTabs > 500) out.maxLiveTabs = 'A whole number 0-500 (0 = no limit)';
  return out;
}

/** `governor.config` arguments for the fields that differ from the limits in force. */
export function governorPatch(l: GovernorLimits, f: GovernorForm): GovernorConfigArgs {
  const args: GovernorConfigArgs = {};
  if (f.enabled !== l.enabled) args.enabled = f.enabled;
  if (f.profile !== l.profile) args.profile = f.profile;
  if (minToSecs(f.freezeMin) !== l.freeze_after_secs) args.freeze_after_secs = minToSecs(f.freezeMin);
  if (minToSecs(f.discardMin) !== l.discard_after_secs) args.discard_after_secs = minToSecs(f.discardMin);
  if (f.maxLiveTabs !== l.max_live_tabs) args.max_live_tabs = f.maxLiveTabs;
  return args;
}

/** "12 min", "1 h 5 min", "40 s". */
export function idleLabel(secs: number): string {
  if (secs < 60) return `${Math.round(secs)} s`;
  const min = Math.floor(secs / 60);
  if (min < 60) return `${min} min`;
  const h = Math.floor(min / 60);
  return min % 60 ? `${h} h ${min % 60} min` : `${h} h`;
}

export function mbLabel(mb: number | null | undefined): string {
  if (mb === null || mb === undefined) return '—';
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`;
  return `${Math.round(mb)} MB`;
}

// ---- sites (adblock allowlist, site policies, hint-disabled sites) ---------------------------

/** "https://www.example.com/a" or "Example.com" → "example.com"; "*" stays; invalid → null. */
export function normalizeHost(input: string): string | null {
  const s = input.trim().toLowerCase();
  if (!s) return null;
  if (s === '*') return '*';
  let host = s;
  if (/^[a-z][a-z0-9+.-]*:\/\//.test(s)) {
    try {
      host = new URL(s).hostname;
    } catch {
      return null;
    }
  } else {
    host = s.split(/[/?#]/)[0].replace(/:\d+$/, '');
  }
  host = host.replace(/^\*\./, '').replace(/^www\./, '').replace(/\.$/, '');
  if (!/^[a-z0-9-]+(\.[a-z0-9-]+)*$/.test(host) || host.split('.').some((p) => !p || p.startsWith('-') || p.endsWith('-'))) {
    return null;
  }
  return host;
}

/** Add a site to a list (normalized, no duplicates, sorted). */
export function addSite(list: string[], input: string): { list: string[]; error: string | null } {
  const host = normalizeHost(input);
  if (!host || host === '*') return { list, error: 'Enter a site like example.com' };
  if (list.includes(host)) return { list, error: `${host} is already in the list` };
  return { list: [...list, host].sort(), error: null };
}

export const POLICY_KEYS: { key: SitePolicyKey; label: string }[] = [
  { key: 'js', label: 'JavaScript' },
  { key: 'images', label: 'Images' },
  { key: 'fonts', label: 'Fonts' },
  { key: 'media', label: 'Media' },
  { key: 'third_party', label: 'Third-party' },
  { key: 'animations', label: 'Animations' },
];

/** A stored policy value: allowed, blocked, or inherited from a less specific pattern. */
export type TriState = boolean | null;

/** Inherit → block → allow → inherit (the click order of a policy cell). */
export function nextTriState(v: TriState | undefined): TriState {
  if (v === null || v === undefined) return false;
  if (v === false) return true;
  return null;
}

/** Policies sorted with "*" first, then by host. */
export function policyRows(sites: Record<string, SitePolicy>): { host: string; policy: SitePolicy }[] {
  return Object.entries(sites)
    .map(([host, policy]) => ({ host, policy }))
    .sort((a, b) => (a.host === '*' ? -1 : b.host === '*' ? 1 : a.host.localeCompare(b.host)));
}

/** The `n` biggest entries of a counter record. */
export function topCounts(record: Record<string, number>, n = 8): { key: string; count: number }[] {
  return Object.entries(record)
    .map(([key, count]) => ({ key, count }))
    .sort((a, b) => b.count - a.count || a.key.localeCompare(b.key))
    .slice(0, n);
}

// ---- link hints (crates/hints) ---------------------------------------------------------------

export type HintMode = keyof HintsConfig['keys'];
export const HINT_MODES: { mode: HintMode; label: string }[] = [
  { mode: 'click', label: 'Click a link' },
  { mode: 'newtab', label: 'Open in a background tab' },
  { mode: 'focus', label: 'Focus a field' },
  { mode: 'copy', label: 'Copy the link' },
];

export interface HintsForm {
  enabled: boolean;
  alphabet: string;
  keys: Record<HintMode, string>;
}

export function hintsForm(c: HintsConfig): HintsForm {
  return {
    enabled: c.enabled,
    alphabet: c.alphabet,
    keys: {
      click: c.keys.click ?? '',
      newtab: c.keys.newtab ?? '',
      focus: c.keys.focus ?? '',
      copy: c.keys.copy ?? '',
    },
  };
}

export function hintsErrors(f: HintsForm): Record<string, string> {
  const out: Record<string, string> = {};
  const seen = new Map<string, HintMode>();
  for (const { mode } of HINT_MODES) {
    const k = f.keys[mode];
    if (!k) continue;
    if ([...k].length !== 1 || /\s/.test(k)) {
      out[`key.${mode}`] = 'One character (case matters: F is Shift+F)';
      continue;
    }
    const other = seen.get(k);
    if (other) out[`key.${mode}`] = `Same key as "${HINT_MODES.find((m) => m.mode === other)!.label}"`;
    else seen.set(k, mode);
  }
  const a = f.alphabet.trim();
  if (!/^[a-z0-9]+$/.test(a) || new Set(a).size < 2 || new Set(a).size !== a.length) {
    out.alphabet = '2 or more different lower-case letters or digits';
  }
  return out;
}

/** `hints.config` arguments for what changed (keys always sent whole: modes left out reset). */
export function hintsPatch(c: HintsConfig, f: HintsForm): HintsConfigArgs {
  const args: HintsConfigArgs = {};
  if (f.enabled !== c.enabled) args.enabled = f.enabled;
  if (f.alphabet.trim() !== c.alphabet) args.alphabet = f.alphabet.trim();
  const keys = Object.fromEntries(HINT_MODES.map(({ mode }) => [mode, f.keys[mode] || null])) as Record<HintMode, string | null>;
  if (HINT_MODES.some(({ mode }) => keys[mode] !== c.keys[mode])) args.keys = keys;
  return args;
}

/** "F" → "Shift+F", "f" → "f", "" → "". */
export function hintKeyLabel(k: string): string {
  return /^[A-Z]$/.test(k) ? `Shift+${k}` : k;
}
