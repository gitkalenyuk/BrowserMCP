// The mock behind Settings → Connections, Profiles, Proxies, Recording behaves like the core
// where the sections depend on it (pnpm dev and the other tests use it).
import { afterEach, describe, expect, it } from 'vitest';
import { MockApi } from '../api/mock';

let api: MockApi;
const make = () => (api = new MockApi({ simulate: false }));
afterEach(() => api?.close());

describe('E2 settings mock', () => {
  it('reports connection info and keeps the CDP setting', async () => {
    make();
    const info = await api.call('api.info');
    expect(info.urls?.mcp).toBe('http://127.0.0.1:47800/mcp');
    expect(info.toolsets.filter((t) => t.default).map((t) => t.name)).toEqual(['core', 'system']);
    // A profile of the demo data has a stealth identity: the port stays shut.
    await expect(api.call('api.config', { cdp_enabled: true })).rejects.toThrow(/stealth/);
    await api.call('identity.assign', { profile: 'stealth-de' });
    const r = await api.call('api.config', { cdp_enabled: true, cdp_port: 9229 });
    expect(r.config).toEqual({ cdp_enabled: true, cdp_port: 9229 });
    expect(r.cdp.restart_needed).toBe(true);
    await expect(api.call('api.config', { cdp_port: 80 })).rejects.toThrow(/1024/);
  });

  it('profile.create changes only the given fields and masks passwords', async () => {
    make();
    await api.call('profile.create', { id: 'shop', proxy: 'socks5://u:secret@h:1080', accept_language: 'de-DE' });
    let shop = (await api.call('profile.list')).profiles.find((p) => p.id === 'shop')!;
    expect(shop.proxy).toBe('socks5://u:***@h:1080');
    expect(shop.accept_language).toBe('de-DE');
    await api.call('profile.create', { id: 'shop', proxy: '' });
    shop = (await api.call('profile.list')).profiles.find((p) => p.id === 'shop')!;
    expect(shop.proxy).toBeNull();
    expect(shop.accept_language).toBe('de-DE');
    await expect(api.call('profile.create', { id: 'Bad Id' })).rejects.toThrow(/invalid/);
  });

  it('identities: generate, assign (profile.list follows), fit to a proxy, remove', async () => {
    make();
    const gen = await api.call('identity.generate', { preset: 'windows-1440p', id: 'shop-win', save: true });
    expect(gen.identity.preset).toBe('windows-1440p');
    await api.call('identity.assign', { profile: 'work', id: 'shop-win' });
    expect((await api.call('profile.list')).profiles.find((p) => p.id === 'work')?.identity).toBe('shop-win');
    const fit = await api.call('identity.fit_to_proxy', { identity_id: 'shop-win', proxy_id: 'pool:datacenter-us' });
    expect(fit.identity.timezone).toBe('America/New_York');
    expect(fit.changes.map((c) => c.field)).toContain('timezone');
    await api.call('identity.remove', { id: 'shop-win' });
    expect((await api.call('profile.list')).profiles.find((p) => p.id === 'work')?.identity).toBeNull();
    expect((await api.call('identity.list')).profiles.work).toBeUndefined();
  });

  it('pools: add, strategy, check, remove; rules: masked round trip and test', async () => {
    make();
    const add = await api.call('proxy.pool_add', { pool: 'new', proxies: ['socks5://a:b@h1:1080', 'socks5://a:b@h1:1080', 'h2:8000:u:p'], strategy: 'sticky' });
    expect(add).toMatchObject({ added: 2, total: 2, strategy: 'sticky' });
    await expect(api.call('proxy.pool_add', { pool: 'new', proxies: ['https://h:443'] })).rejects.toThrow(/socks5/);
    const listed = (await api.call('proxy.pool_list')).pools.find((p) => p.name === 'new')!;
    expect(listed.proxies[0].proxy).toBe('socks5://a:***@h1:1080');
    await api.call('proxy.pool_add', { pool: 'new', proxies: [], strategy: 'random' });
    expect((await api.call('proxy.pool_list')).pools.find((p) => p.name === 'new')?.strategy).toBe('random');
    const checked = await api.call('proxy.pool_check', { pool: 'residential-de' });
    expect(checked.unhealthy).toBe(1);
    expect((await api.call('proxy.pool_remove', { pool: 'new', proxy: 'socks5://a:***@h1:1080' })).remaining).toBe(1);

    await api.call('proxy.rules_set', { profile: 'work', rules: [{ match: '*.example.com', via: 'pool:new' }], default: 'direct' });
    const test = await api.call('proxy.rules_test', { profile: 'work', url: 'https://www.example.com/a' });
    expect(test).toMatchObject({ rule: 1, via: 'pool:new', pac_url: 'https://www.example.com/' });
    expect((await api.call('proxy.rules_test', { profile: 'work', url: 'https://other.org/' })).rule).toBeNull();
    await expect(api.call('proxy.rules_set', { profile: 'work', rules: [{ match: 'x', via: 'pool:missing' }] })).rejects.toThrow(/missing/);
  });

  it('recording and script defaults validate and reset', async () => {
    make();
    expect((await api.call('record.settings', { video_format: 'mp4' })).settings.video_format).toBe('mp4');
    await expect(api.call('record.settings', { video_fps: 0 })).rejects.toThrow(/video_fps/);
    expect((await api.call('record.settings', { reset: true })).settings.video_format).toBe('webm');
    const s = await api.call('script.settings', { run_parallel: 8 });
    expect(s.settings.run_parallel).toBe(8);
    expect(s.host.found).toBe(true);
    await expect(api.call('script.settings', { run_retries: 11 })).rejects.toThrow(/run_retries/);
  });
});
