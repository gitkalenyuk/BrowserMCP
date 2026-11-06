/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_BMCP_API?: 'mock' | 'ws';
  readonly VITE_BMCP_WS_URL?: string;
  /** Side panel open on first start (no saved settings yet), e.g. for screenshots of a fresh profile. */
  readonly VITE_BMCP_PANEL?: 'traffic' | 'recording' | 'ai' | 'terminal' | 'scripts' | 'runs';
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
