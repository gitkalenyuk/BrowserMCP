// Fake of `ai.models { provider }` for MockApi (`pnpm dev`, tests): the models a provider offers
// for a new chat. CLI agents get their model aliases; API providers their known model list (or
// two made-up names, like ai.provider_test does).

import { RpcError } from './client';
import type { AiModel, AiModels, Method, Params, Result } from './types';

type Call = <M extends Method>(method: M, params: Params<M>) => Promise<Result<M>>;

const CLI_MODELS: Record<string, AiModels> = {
  'claude-code': {
    models: [
      { id: 'opus', name: 'Opus', hint: 'most capable' },
      { id: 'sonnet', name: 'Sonnet', hint: 'fast, for everyday tasks' },
      { id: 'haiku', name: 'Haiku', hint: 'fastest' },
    ],
    default: 'sonnet',
  },
  codex: {
    models: [
      { id: 'gpt-5-codex', name: 'GPT-5 Codex' },
      { id: 'gpt-5', name: 'GPT-5' },
    ],
    default: 'gpt-5-codex',
  },
};

export function createModelMocks(call: Call) {
  return {
    'ai.models': async ({ provider }: { provider: string }): Promise<AiModels> => {
      const { providers } = await call('ai.providers', undefined as Params<'ai.providers'>);
      const p = providers.find((x) => x.id === provider);
      if (!p) throw new RpcError('NOT_FOUND', `unknown AI provider '${provider}'`, { hint: 'ai_providers lists them' });
      const cli = CLI_MODELS[p.id];
      if (cli) return structuredClone(cli);
      const ids = p.models.length ? p.models : [`${p.id}-large`, `${p.id}-small`];
      const models: AiModel[] = ids.map((id) => ({ id, name: id }));
      return { models, default: p.model ?? ids[0] };
    },
  };
}
