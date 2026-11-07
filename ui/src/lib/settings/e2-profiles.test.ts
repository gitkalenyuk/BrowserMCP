import { describe, expect, it } from 'vitest';
import type { IdentityPreset } from '../api/types-e2-settings';
import {
  fitProxyRef,
  identityIdFor,
  presetGroups,
  profileForm,
  profileIdError,
  profileIdFromName,
  profileProxyError,
  profileSave,
  webrtcLimited,
} from './e2-profiles';
import type { ProfileEntry } from '../api/types';

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

describe('profile ids', () => {
  it('derives a valid id from a name', () => {
    expect(profileIdFromName('Work (DE) #2')).toBe('work-de-2');
    expect(profileIdFromName('  Café  Ünïcode ')).toBe('cafe-unicode');
    expect(profileIdFromName('Магазин')).toBe('');
    expect(profileIdFromName('x'.repeat(80))).toHaveLength(64);
  });

  it('checks new ids', () => {
    expect(profileIdError('', [])).toBe('settings.profiles.err.idEmpty');
    expect(profileIdError('Work', [])).toBe('settings.profiles.err.idChars');
    expect(profileIdError('work', ['work'])).toBe('settings.profiles.err.idTaken');
    expect(profileIdError('work_2', ['work'])).toBe('');
  });
});

describe('profileSave', () => {
  const p = profile('shop', { label: 'Shop', proxy: 'socks5://u:***@h:1080', accept_language: 'de-DE,de' });

  it('sends nothing when nothing changed, a masked proxy included', () => {
    expect(profileSave(p, profileForm(p))).toEqual({});
  });

  it('sends only the changed fields', () => {
    expect(profileSave(p, { ...profileForm(p), label: ' Shop DE ' })).toEqual({ label: 'Shop DE' });
    expect(profileSave(p, { ...profileForm(p), proxy: '' })).toEqual({ create: { proxy: '' } });
    expect(profileSave(p, { ...profileForm(p), proxy: 'pool:de', acceptLanguage: '' })).toEqual({
      create: { proxy: 'pool:de', accept_language: '' },
    });
  });

  it('treats a label equal to the id as no label', () => {
    const plain = profile('work');
    expect(profileForm(plain).label).toBe('');
    expect(profileSave(plain, { ...profileForm(plain), label: 'Work' })).toEqual({ label: 'Work' });
  });

  it('refuses masked URLs typed by hand and unsupported schemes', () => {
    expect(profileProxyError('socks5://u:***@h:1080', 'socks5://u:***@h:1080')).toBe('');
    expect(profileProxyError('socks5://u:***@other:1080', 'socks5://u:***@h:1080')).toBe('settings.profiles.err.proxyMasked');
    expect(profileProxyError('https://h:443', '')).toBe('settings.profiles.err.proxyHttps');
    expect(profileProxyError('socks4://h:1080', '')).toBe('settings.profiles.err.proxySocks4');
    expect(profileProxyError('http://u:p@h:3128', '')).toBe('');
  });
});

describe('identities', () => {
  const preset = (name: string, os: IdentityPreset['os'], kind: IdentityPreset['kind']): IdentityPreset => ({
    name,
    title: name,
    os,
    kind,
    device: null,
    screen: { width: 1, height: 1, dpr: 1 },
  });

  it('groups presets by purpose', () => {
    const groups = presetGroups([preset('win', 'windows', 'stealth'), preset('iphone', 'ios', 'device_emulation'), preset('pixel', 'android', 'stealth')]);
    expect(groups.map((g) => [g.group, g.presets.map((p) => p.name)])).toEqual([
      ['desktop', ['win']],
      ['mobile', ['pixel']],
      ['emulation', ['iphone']],
    ]);
  });

  it('names generated identities after the profile', () => {
    expect(identityIdFor('shop', 'windows-1080p')).toBe('shop-windows-1080p');
    expect(identityIdFor('a'.repeat(60), 'windows-1080p')).toHaveLength(64);
  });

  it('picks the proxy reference to fit an identity to', () => {
    const fwd = [{ id: 'fwd-2', socks5: 'socks5://127.0.0.1:5000', http: 'http://127.0.0.1:5000', upstream: 'x' }];
    expect(fitProxyRef(profile('a'), fwd)).toBe('direct');
    expect(fitProxyRef(profile('a', { proxy: 'pool:de' }), fwd)).toBe('pool:de');
    expect(fitProxyRef(profile('a', { proxy: 'socks5://u:***@h:1', proxy_server: 'socks5://127.0.0.1:5000' }), fwd)).toBe('fwd-2');
    expect(fitProxyRef(profile('a', { proxy: 'http://h:3128' }), [])).toBe('http://h:3128');
    expect(fitProxyRef(profile('a', { proxy: 'socks5://u:***@h:1' }), [])).toBeNull();
  });

  it('says when WebRTC is limited', () => {
    expect(webrtcLimited(profile('a'))).toBe(false);
    expect(webrtcLimited(profile('a', { proxy: 'pool:de' }))).toBe(true);
  });
});
