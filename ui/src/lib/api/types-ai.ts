// Wire types of the user's own AI providers (crates/ai/src/commands.rs): ai.provider_add,
// ai.provider_remove and the fields ai.providers / ai.provider_set gained for them. Params come
// from the generated protocol (sdk/protocol/src/generated/commands.ts, `cargo xtask gen-sdk`).
// The `declare module` below adds them to RpcMethods and the AI provider types of types.ts
// (declaration merging), so types.ts itself stays untouched.

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';
import type { AiProvider } from './types';

/** Wire protocol of an API provider. */
export type AiApi = 'openai' | 'anthropic' | 'gemini';

export type AiProviderAdd = Commands['ai.provider_add']['args'];
/** Why ai.provider_test sent nothing: the provider is not set up. */
export type AiProblem = NonNullable<Commands['ai.provider_test']['output']['problem']>;
export type AiProviderRemoved = Commands['ai.provider_remove']['output'];

export interface AiProviderMethods {
  /** Add the user's own provider (from a template preset or blank). */
  'ai.provider_add': { params: AiProviderAdd; result: AiProvider };
  /** Remove a user provider (built-ins cannot be removed). */
  'ai.provider_remove': { params: Commands['ai.provider_remove']['args']; result: AiProviderRemoved };
}

declare module './types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends AiProviderMethods {}
  interface AiProvider {
    /** `false` for providers the user added (they can be renamed and removed). */
    builtin?: boolean;
    /** User providers: the preset they were made from; null = blank. */
    template?: string | null;
    /** `model` was chosen by the user; false = it is the provider's default and follows it. */
    model_set?: boolean;
    /** API providers: names of the extra HTTP headers (values are never returned, like keys). */
    header_names?: string[];
  }
  interface AiProviderSet {
    /** User providers: new display name. */
    name?: string;
    /** User providers: wire protocol. */
    api?: AiApi;
    /** Replaces every extra header (`{}` removes them); a null value keeps the stored one. */
    headers?: Record<string, string | null>;
  }
  interface AiProviderTest {
    /** Set when nothing was sent because the provider is not set up. */
    problem?: AiProblem | null;
  }
}
