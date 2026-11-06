import { describe, expect, it } from 'vitest';
import { isSafeLinkUrl } from './safeurl';
import { createApi } from './api';

describe('isSafeLinkUrl', () => {
  it('allows only http(s)', () => {
    expect(isSafeLinkUrl('https://example.com/')).toBe(true);
    expect(isSafeLinkUrl('http://127.0.0.1:8000/x')).toBe(true);
    for (const bad of ['javascript:alert(1)', 'JAVASCRIPT:fetch(1)', ' javascript:x', 'data:text/html,x', 'bmcp://ui/x', 'file:///c:/x', '', null, undefined, 'not a url']) {
      expect(isSafeLinkUrl(bad as string)).toBe(false);
    }
  });
});

describe('createApi', () => {
  it('injected endpoint wins over ?api/?ws (no override from the URL)', () => {
    const api = createApi({ search: '?api=ws&ws=wss://evil.example/rpc' }, { ws: 'ws://127.0.0.1:47800/v1/ws' }, false);
    expect(api.kind).toBe('ws');
    expect((api as unknown as { url?: string }).url ?? JSON.stringify(api)).not.toContain('evil');
  });
  it('ignores ?api in production builds without injection', () => {
    expect(createApi({ search: '?api=ws&ws=wss://evil.example/rpc' }, undefined, false).kind).toBe('mock');
  });
  it('honours ?api in dev builds', () => {
    expect(createApi({ search: '?api=ws' }, undefined, true).kind).toBe('ws');
  });
});
