// Wire types of Settings → Connections & API, Profiles & identity, Proxies, Recording & scripts:
// api.* (crates/api/src/info.rs), identity.* / proxy.* / geo.* (crates/identity),
// record.settings (crates/record/src/settings.rs), script.settings (crates/jobs/src/settings.rs).
// Params and results come from the generated protocol (`cargo xtask gen-sdk`).
//
// The `declare module` below adds the methods to RpcMethods (declaration merging), like
// types-settings.ts does. profile.* is declared in types.ts.

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';

/** `{}` arguments (`Record<string, never>`) become `void`. */
type NoArgs<A> = string extends keyof A ? (A[string & keyof A] extends never ? void : A) : A;

type Cmd<K extends keyof Commands> = {
  params: NoArgs<Commands[K]['args']>;
  result: Commands[K]['output'];
};

export interface E2SettingsMethods {
  'api.info': Cmd<'api.info'>;
  'api.config': Cmd<'api.config'>;
  'record.settings': Cmd<'record.settings'>;
  'script.settings': Cmd<'script.settings'>;
  'identity.list': Cmd<'identity.list'>;
  'identity.presets': Cmd<'identity.presets'>;
  'identity.generate': Cmd<'identity.generate'>;
  'identity.assign': Cmd<'identity.assign'>;
  'identity.remove': Cmd<'identity.remove'>;
  'identity.fit_to_proxy': Cmd<'identity.fit_to_proxy'>;
  'proxy.pool_list': Cmd<'proxy.pool_list'>;
  'proxy.pool_add': Cmd<'proxy.pool_add'>;
  'proxy.pool_remove': Cmd<'proxy.pool_remove'>;
  'proxy.pool_check': Cmd<'proxy.pool_check'>;
  'proxy.forward_list': Cmd<'proxy.forward_list'>;
  'proxy.rules_get': Cmd<'proxy.rules_get'>;
  'proxy.rules_set': Cmd<'proxy.rules_set'>;
  'proxy.rules_test': Cmd<'proxy.rules_test'>;
  'geo.lookup': Cmd<'geo.lookup'>;
  'stealth.check': Cmd<'stealth.check'>;
}

export type ApiInfo = Commands['api.info']['output'];
export type ApiToolset = ApiInfo['toolsets'][number];
export type CdpInfo = ApiInfo['cdp'];
export type RecordSettingsOut = Commands['record.settings']['output'];
export type RecordingSettings = RecordSettingsOut['settings'];
export type RecordSettingsArgs = Commands['record.settings']['args'];
export type ScriptSettingsOut = Commands['script.settings']['output'];
export type ScriptSettings = ScriptSettingsOut['settings'];
export type ScriptSettingsArgs = Commands['script.settings']['args'];
export type IdentitySummary = Commands['identity.list']['output']['identities'][number];
export type IdentityPreset = Commands['identity.presets']['output']['presets'][number];
export type IdentityOs = IdentityPreset['os'];
export type GeoInfo = Commands['geo.lookup']['output']['geo'];
export type ProxyPool = Commands['proxy.pool_list']['output']['pools'][number];
export type PoolProxy = ProxyPool['proxies'][number];
export type PoolStrategy = ProxyPool['strategy'];
export type ProxyForwarder = Commands['proxy.forward_list']['output']['forwarders'][number];
export type ProfileRoutes = Commands['proxy.rules_get']['output']['routes'][number];
export type ProxyRule = ProfileRoutes['rules'][number];
export type RulesTestResult = Commands['proxy.rules_test']['output'];
export type StealthCheck = Commands['stealth.check']['output'];

declare module './types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends E2SettingsMethods {}
}
