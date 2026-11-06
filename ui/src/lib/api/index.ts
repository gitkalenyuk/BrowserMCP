import type { BrowserApi } from './client';
import { MockApi } from './mock';
import { DEFAULT_WS_URL, WsApi } from './ws';

export type { BrowserApi } from './client';
export { RpcError } from './client';
export * from './types';

/**
 * Picks the API implementation at startup. Mock is the default.
 *   window.__BMCP__.ws  injected by the browser into every bmcp://ui/ page: the real core
 *   ?api=ws             use the real core at ws://127.0.0.1:47800/v1/ws
 *   ?api=ws&ws=<url>    ... at another URL
 *   VITE_BMCP_API=ws    same, at build time
 */
export function createApi(
  location: { search: string } = window.location,
  injected: { ws?: string | null } | undefined = typeof window === 'undefined' ? undefined : window.__BMCP__,
): BrowserApi {
  const params = new URLSearchParams(location.search);
  if (!params.get('api') && injected?.ws) return new WsApi({ url: injected.ws });
  const kind = params.get('api') ?? import.meta.env.VITE_BMCP_API ?? 'mock';
  if (kind === 'ws') {
    return new WsApi({ url: params.get('ws') ?? import.meta.env.VITE_BMCP_WS_URL ?? DEFAULT_WS_URL });
  }
  return new MockApi();
}
