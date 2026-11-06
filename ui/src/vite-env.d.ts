/// <reference types="svelte" />
/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_BMCP_API?: 'mock' | 'ws';
  readonly VITE_BMCP_WS_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
