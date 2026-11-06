import type { BrowserApi } from './client';
import { MockApi } from './mock';
import { DEFAULT_WS_URL, WsApi } from './ws';

export type { BrowserApi } from './client';
export { RpcError } from './client';
export * from './types';

/**
 * Picks the API implementation at startup.
 *   window.__BMCP__.ws  injected by the browser into every bmcp://ui/ page: ALWAYS wins.
 *                       SECURITY: a privileged bmcp://ui page must never take its API endpoint
 *                       from the URL, or a web page could navigate a tab to
 *                       bmcp://ui/newtab.html?api=ws&ws=wss://evil/ and feed it hostile data.
 *   ?api=ws[&ws=<url>]  dev builds only (Vite / pnpm dev without the browser)
 *   VITE_BMCP_API=ws    build-time default; mock otherwise
 */
export function createApi(
  location: { search: string } = window.location,
  injected: { ws?: string | null } | undefined = typeof window === 'undefined' ? undefined : window.__BMCP__,
  dev: boolean = import.meta.env.DEV,
): BrowserApi {
  if (injected?.ws) return new WsApi({ url: injected.ws });
  const params = dev ? new URLSearchParams(location.search) : new URLSearchParams();
  const kind = params.get('api') ?? import.meta.env.VITE_BMCP_API ?? 'mock';
  if (kind === 'ws') {
    return new WsApi({ url: params.get('ws') ?? import.meta.env.VITE_BMCP_WS_URL ?? DEFAULT_WS_URL });
  }
  return new MockApi();
}
