// Wire types of the settings page and the heal UI: notify.* (crates/notify), run.heal_*
// (crates/heal), governor.* (crates/governor), hints.config (crates/hints), adblock.* /
// simplify.* (crates/adblock), plugins.* / userscripts.* (crates/plugins). Params and results
// come from the generated protocol (sdk/protocol/src/generated/commands.ts, `cargo xtask
// gen-sdk`); the run.heal_* event payloads are built with `json!` in crates/heal/src/watch.rs and
// are written out here.
//
// types.ts re-exports this file; the `declare module` below adds the methods and events to
// RpcMethods / ApiEvents (declaration merging), and `is_default` / `default` to the AI provider
// types of types.ts.

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';

/** `{}` arguments (`Record<string, never>`) become `void`. */
type NoArgs<A> = string extends keyof A ? (A[string & keyof A] extends never ? void : A) : A;

type Cmd<K extends keyof Commands> = {
  params: NoArgs<Commands[K]['args']>;
  result: Commands[K]['output'];
};

export interface SettingsMethods {
  'notify.config_get': Cmd<'notify.config_get'>;
  'notify.config_set': Cmd<'notify.config_set'>;
  'notify.test': Cmd<'notify.test'>;
  'run.heal_propose': Cmd<'run.heal_propose'>;
  'run.heal_accept': Cmd<'run.heal_accept'>;
  'run.heal_reject': Cmd<'run.heal_reject'>;
  'run.heal_proposals': Cmd<'run.heal_proposals'>;
  'governor.config': Cmd<'governor.config'>;
  'governor.status': Cmd<'governor.status'>;
  'hints.config': Cmd<'hints.config'>;
  'adblock.toggle': Cmd<'adblock.toggle'>;
  'adblock.update_lists': Cmd<'adblock.update_lists'>;
  'adblock.stats': Cmd<'adblock.stats'>;
  'simplify.site': Cmd<'simplify.site'>;
  'simplify.get': Cmd<'simplify.get'>;
  'simplify.list': Cmd<'simplify.list'>;
  'plugins.list': Cmd<'plugins.list'>;
  'plugins.enable': Cmd<'plugins.enable'>;
  'plugins.disable': Cmd<'plugins.disable'>;
  'plugins.reload': Cmd<'plugins.reload'>;
  'plugins.install': Cmd<'plugins.install'>;
  'plugins.logs': Cmd<'plugins.logs'>;
  'plugins.palette': Cmd<'plugins.palette'>;
  'plugins.palette_run': Cmd<'plugins.palette_run'>;
  'userscripts.list': Cmd<'userscripts.list'>;
  'userscripts.get': Cmd<'userscripts.get'>;
  'userscripts.save': Cmd<'userscripts.save'>;
  'userscripts.delete': Cmd<'userscripts.delete'>;
  'userscripts.enable': Cmd<'userscripts.enable'>;
}

// ---- shapes the settings page passes around -------------------------------------------------

export type NotifyConfig = Commands['notify.config_get']['output'];
export type NotifyRule = NotifyConfig['rules'][number];
export type NotifyChannel = NotifyRule['channels'][number];
export type NotifyWhen = NotifyRule['when'];
export type NotifyConfigPatch = Commands['notify.config_set']['args'];
export type GovernorStatus = Commands['governor.status']['output'];
export type GovernorLimits = GovernorStatus['limits'];
export type GovernorProfile = GovernorLimits['profile'];
export type GovernorConfigArgs = Commands['governor.config']['args'];
export type GovernorTab = GovernorStatus['tabs'][number];
export type HintsConfig = Commands['hints.config']['output']['config'];
export type HintsConfigArgs = Commands['hints.config']['args'];
export type AdblockSettings = Commands['adblock.toggle']['output'];
export type AdblockStats = Commands['adblock.stats']['output'];
export type AdblockListUpdate = Commands['adblock.update_lists']['output'];
export type SitePolicy = Commands['simplify.list']['output']['sites'][string];
export type SitePolicyKey = keyof SitePolicy;
export type SimplifyCheck = Commands['simplify.get']['output'];
export type SimplifySiteArgs = Commands['simplify.site']['args'];
export type PluginInfo = Commands['plugins.list']['output']['plugins'][number];
export type PluginLogLine = Commands['plugins.logs']['output']['lines'][number];
export type PluginPaletteEntry = Commands['plugins.palette']['output']['commands'][number];
export type UserscriptInfo = Commands['userscripts.list']['output']['userscripts'][number];
/** A heal proposal as `run.heal_proposals` lists it (with the patched code). */
export type HealProposal = Commands['run.heal_proposals']['output']['proposals'][number];
export type HealProposalStatus = HealProposal['status'];
export type HealMode = 'ask' | 'auto' | 'off';

// ---- events (crates/heal/src/watch.rs) -----------------------------------------------------

export interface HealProposedEvent {
  /** The run. */
  id: string;
  proposal_id: string;
  row: number;
  script: string;
  diff: string;
  patched_code: string;
  explanation: string;
  confidence: number;
  attempt: number;
  max_attempts: number;
  checks: string[];
  mode: HealMode;
  /** "auto" mode did not apply it (low confidence, failed dry run, limit reached). */
  note?: string;
}

export interface HealAppliedEvent {
  id: string;
  proposal_id: string;
  script: string;
  requeued: number;
  auto: boolean;
}

export interface HealFailedEvent {
  id: string;
  row: number;
  attempts: number;
  error: string;
}

export interface SettingsEvents {
  'run.heal_proposed': HealProposedEvent;
  'run.heal_applied': HealAppliedEvent;
  'run.heal_failed': HealFailedEvent;
}

declare module './types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends SettingsMethods {}
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface ApiEvents extends SettingsEvents {}
  interface AiProvider {
    /** Used by background AI features (self-healing, page.extract) when they name no provider. */
    is_default?: boolean;
  }
  interface AiProviderSet {
    /** true: make it the default of background AI features; false: stop being the default. */
    default?: boolean;
  }
}
