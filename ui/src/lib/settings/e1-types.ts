// Wire types of Settings → Startup & tabs, Search, Downloads, Site settings, Data & storage and
// About: startup.config, search.* (crates/settings), downloads.config / zoom.* /
// browsing_data.* / system.about (crates/settings), downloads.clear (crates/data),
// tabs.closed* (crates/core), permissions.list / set (crates/profiles). Params and results come
// from the generated protocol (`cargo xtask gen-sdk`); the `declare module` below adds them to
// RpcMethods (declaration merging), like types-settings.ts does for the other sections.

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';

/** `{}` arguments (`Record<string, never>`) become `void`. */
type NoArgs<A> = string extends keyof A ? (A[string & keyof A] extends never ? void : A) : A;

type Cmd<K extends keyof Commands> = {
  params: NoArgs<Commands[K]['args']>;
  result: Commands[K]['output'];
};

/** All arguments optional: `call('startup.config')` reads, `call('startup.config', {…})` changes. */
type OptionalCmd<K extends keyof Commands> = {
  params: Commands[K]['args'] | void;
  result: Commands[K]['output'];
};

export interface E1Methods {
  'startup.config': OptionalCmd<'startup.config'>;
  'search.config': OptionalCmd<'search.config'>;
  'search.url': Cmd<'search.url'>;
  'downloads.config': OptionalCmd<'downloads.config'>;
  'downloads.clear': OptionalCmd<'downloads.clear'>;
  'zoom.list': Cmd<'zoom.list'>;
  'zoom.reset': OptionalCmd<'zoom.reset'>;
  'browsing_data.clear': Cmd<'browsing_data.clear'>;
  'browsing_data.usage': Cmd<'browsing_data.usage'>;
  'system.about': Cmd<'system.about'>;
  'tabs.closed': Cmd<'tabs.closed'>;
  'tabs.closed_reopen': Cmd<'tabs.closed_reopen'>;
  'tabs.closed_clear': Cmd<'tabs.closed_clear'>;
  'permissions.list': Cmd<'permissions.list'>;
  'permissions.set': Cmd<'permissions.set'>;
}

export type StartupConfig = Commands['startup.config']['output']['config'];
export type StartupArgs = Commands['startup.config']['args'];
export type OnStartup = StartupConfig['on_startup'];
export type NewTabPosition = StartupConfig['new_tab_position'];
export type SearchConfig = Commands['search.config']['output'];
export type SearchEngine = SearchConfig['engines'][number];
export type SearchArgs = Commands['search.config']['args'];
export type DownloadsConfig = Commands['downloads.config']['output'];
export type SiteZoom = Commands['zoom.list']['output']['sites'][number];
export type ClearArgs = Commands['browsing_data.clear']['args'];
export type ClearResult = Commands['browsing_data.clear']['output'];
export type DataUsage = Commands['browsing_data.usage']['output'];
export type UsageArea = DataUsage['areas'][number];
export type About = Commands['system.about']['output'];
export type ClosedTab = Commands['tabs.closed']['output']['tabs'][number];
export type PermissionRule = Commands['permissions.list']['output']['rules'][number];
export type PermissionKind = PermissionRule['kind'];

/** `settings.changed` (crates/settings): which area changed; the page re-reads it. */
export interface SettingsChangedEvent {
  area: 'startup' | 'search' | 'downloads';
  [key: string]: unknown;
}

declare module '../api/types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends E1Methods {}
  interface ApiEvents {
    'settings.changed': SettingsChangedEvent;
  }
}
