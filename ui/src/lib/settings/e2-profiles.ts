// Settings → Profiles & identity: ids, the edit form → profile.create arguments, preset groups,
// and which proxy reference an identity can be fitted to. Pure functions (e2-profiles.test.ts).

import type { ProfileEntry } from '../api/types';
import type { IdentityPreset, ProxyForwarder } from '../api/types-e2-settings';
import { isMasked } from './e2-proxies';

const ID_RE = /^[a-z0-9_-]{1,64}$/;

/** "Work (DE) #2" → "work-de-2": a valid profile id (1-64 of a-z, 0-9, '-', '_'). */
export function profileIdFromName(name: string): string {
  const id = name
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .replace(/-{2,}/g, '-')
    .slice(0, 64)
    .replace(/-+$/, '');
  return id;
}

/** i18n key of what is wrong with a new id ('' = fine). */
export function profileIdError(id: string, existing: readonly string[]): string {
  if (!id) return 'settings.profiles.err.idEmpty';
  if (!ID_RE.test(id)) return 'settings.profiles.err.idChars';
  if (existing.includes(id)) return 'settings.profiles.err.idTaken';
  return '';
}

/** What the edit form holds for one profile. */
export interface ProfileForm {
  label: string;
  /** '' = direct; a proxy URL, `pool:NAME` or `fwd-N` otherwise. Masked = unchanged. */
  proxy: string;
  acceptLanguage: string;
}

export function profileForm(p: ProfileEntry): ProfileForm {
  return { label: p.label === p.id ? '' : p.label, proxy: p.proxy ?? '', acceptLanguage: p.accept_language ?? '' };
}

export interface ProfileSave {
  /** profile.update {label}, when the name changed. */
  label?: string;
  /** profile.create fields that changed (proxy "" removes it). */
  create?: { proxy?: string; accept_language?: string };
}

/**
 * Only what changed. A proxy the core showed masked (`user:***@`) is never sent back: the
 * stored password would be replaced by the stars.
 */
export function profileSave(p: ProfileEntry, f: ProfileForm): ProfileSave {
  const before = profileForm(p);
  const out: ProfileSave = {};
  if (f.label.trim() !== before.label) out.label = f.label.trim();
  const create: NonNullable<ProfileSave['create']> = {};
  const proxy = f.proxy.trim();
  if (proxy !== before.proxy && !(isMasked(proxy) && proxy === before.proxy)) create.proxy = proxy;
  if (f.acceptLanguage.trim() !== before.acceptLanguage) create.accept_language = f.acceptLanguage.trim();
  if (Object.keys(create).length) out.create = create;
  return out;
}

/** i18n key when the proxy field cannot be saved ('' = fine). */
export function profileProxyError(proxy: string, original: string): string {
  const p = proxy.trim();
  if (isMasked(p) && p !== original) return 'settings.profiles.err.proxyMasked';
  if (/^https:\/\//i.test(p)) return 'settings.profiles.err.proxyHttps';
  if (/^socks4a?:\/\//i.test(p)) return 'settings.profiles.err.proxySocks4';
  return '';
}

export type PresetGroup = 'desktop' | 'mobile' | 'emulation';

/** Presets by what they are for: desktop stealth, mobile stealth (Android), device emulation. */
export function presetGroups(presets: IdentityPreset[]): { group: PresetGroup; presets: IdentityPreset[] }[] {
  const of = (p: IdentityPreset): PresetGroup =>
    p.kind === 'device_emulation' ? 'emulation' : p.os === 'android' || p.os === 'ios' ? 'mobile' : 'desktop';
  const groups: PresetGroup[] = ['desktop', 'mobile', 'emulation'];
  return groups
    .map((group) => ({ group, presets: presets.filter((p) => of(p) === group) }))
    .filter((g) => g.presets.length > 0);
}

/** Id for the identity generated for a profile: `<profile>-<preset>` within 64 chars. */
export function identityIdFor(profile: string, preset: string): string {
  return `${profile}-${preset}`.toLowerCase().replace(/[^a-z0-9_-]+/g, '-').slice(0, 64);
}

/**
 * The proxy reference identity.fit_to_proxy should look up for a profile: its pool, the
 * local forwarder its traffic goes through (the password stays in the core), its plain URL,
 * or `direct`. null when none of those can be named.
 */
export function fitProxyRef(p: ProfileEntry, forwarders: ProxyForwarder[]): string | null {
  const proxy = p.proxy?.trim() ?? '';
  if (!proxy || proxy === 'direct') return 'direct';
  if (proxy.startsWith('pool:') || /^fwd-\d+$/.test(proxy)) return proxy;
  const fwd = p.proxy_server ? forwarders.find((f) => f.socks5 === p.proxy_server || f.http === p.proxy_server) : undefined;
  if (fwd) return fwd.id;
  return isMasked(proxy) ? null : proxy;
}

/** "WebRTC: limited to the proxy" when proxied (the core does it by itself). */
export function webrtcLimited(p: ProfileEntry): boolean {
  return !!p.proxy && p.proxy !== 'direct';
}
