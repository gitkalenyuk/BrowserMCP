// Wire types of voice dictation: voice.* (crates/voice). Params and results come from the
// generated protocol (sdk/protocol/src/generated/commands.ts, `cargo xtask gen-sdk`); the
// `declare module` below adds the methods to RpcMethods (declaration merging). types.ts
// re-exports this file.

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';

type C<N extends keyof Commands> = Commands[N];

export interface VoiceMethods {
  'voice.settings_get': { params: void; result: C<'voice.settings_get'>['output'] };
  'voice.settings_set': { params: C<'voice.settings_set'>['args']; result: C<'voice.settings_set'>['output'] };
  'voice.engines': { params: void; result: C<'voice.engines'>['output'] };
  'voice.transcribe': { params: C<'voice.transcribe'>['args']; result: C<'voice.transcribe'>['output'] };
  'voice.stream_start': { params: C<'voice.stream_start'>['args']; result: C<'voice.stream_start'>['output'] };
  'voice.stream_push': { params: C<'voice.stream_push'>['args']; result: C<'voice.stream_push'>['output'] };
  'voice.stream_stop': { params: C<'voice.stream_stop'>['args']; result: C<'voice.stream_stop'>['output'] };
  'voice.local_status': { params: void; result: C<'voice.local_status'>['output'] };
  'voice.local_download': { params: C<'voice.local_download'>['args']; result: C<'voice.local_download'>['output'] };
  'voice.local_cancel': { params: void; result: C<'voice.local_cancel'>['output'] };
  'voice.local_delete': { params: C<'voice.local_delete'>['args']; result: C<'voice.local_delete'>['output'] };
}

export type VoiceSettings = C<'voice.settings_get'>['output'];
export type VoiceSettingsPatch = C<'voice.settings_set'>['args'];
export type VoiceEngineId = VoiceSettings['engine'];
export type VoiceEngineInfo = C<'voice.engines'>['output']['engines'][number];
export type VoiceEngines = C<'voice.engines'>['output'];
export type VoiceLocalStatus = C<'voice.local_status'>['output'];
export type VoiceModelStatus = VoiceLocalStatus['models'][number];
export type VoiceTranscript = C<'voice.transcribe'>['output'];
/** The cloud engines with their own key / model / address settings. */
export type VoiceCloudEngine = Exclude<VoiceEngineId, 'local'>;

declare module './types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends VoiceMethods {}
}
