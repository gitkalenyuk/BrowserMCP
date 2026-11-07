// Wire types of the language settings: browser.languages_get / browser.languages_set
// (crates/languages). Params and results come from the generated protocol
// (sdk/protocol/src/generated/commands.ts, `cargo xtask gen-sdk`); the `declare module` below
// adds the methods to RpcMethods (declaration merging). types.ts re-exports this file.

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';

export interface LanguageMethods {
  'browser.languages_get': { params: void; result: Commands['browser.languages_get']['output'] };
  'browser.languages_set': { params: Commands['browser.languages_set']['args']; result: Commands['browser.languages_set']['output'] };
}

export type LanguagesInfo = Commands['browser.languages_get']['output'];
export type LanguagesSetArgs = Commands['browser.languages_set']['args'];

declare module './types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends LanguageMethods {}
}
