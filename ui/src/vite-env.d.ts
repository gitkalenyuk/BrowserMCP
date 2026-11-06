/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_BMCP_API?: 'mock' | 'ws';
  readonly VITE_BMCP_WS_URL?: string;
  /** Side panel open on first start (no saved settings yet), e.g. for screenshots of a fresh profile. */
  /** `1`: the mock starts with no bookmarks/history (first-run import wizard). */
  readonly VITE_BMCP_EMPTY_DATA?: string;
  readonly VITE_BMCP_PANEL?: 'traffic' | 'recording' | 'ai' | 'terminal' | 'scripts' | 'runs';
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
