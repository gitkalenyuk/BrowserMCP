// Fakes of the AI provider commands (ai.providers, ai.provider_add / _set / _remove / _test)
// for MockApi (`pnpm dev`, tests). Shapes and the main checks follow crates/ai/src/config.rs and
// commands.rs: built-ins first, then the user's own providers; unique names; http(s) URLs.

import { RpcError } from './client';
import type { AiProvider, Params, Result } from './types';
import type { AiApi } from './types-ai';

type ProviderMethod = 'ai.providers' | 'ai.provider_add' | 'ai.provider_set' | 'ai.provider_remove' | 'ai.provider_test';
export type ProviderHandlers = { [M in ProviderMethod]: (params: Params<M>) => Result<M> | Promise<Result<M>> };

function provider(id: string, name: string, kind: 'api' | 'cli', protocol: AiProvider['protocol'], base_url: string, opts: Partial<AiProvider> = {}): AiProvider {
  const p: AiProvider = {
    id,
    name,
    kind,
    protocol,
    needs_key: kind === 'api',
    has_key: false,
    base_url,
    model: null,
    models: [],
    path: null,
    configured: false,
    is_default: false,
    builtin: true,
    template: null,
    headers: {},
    ...opts,
  };
  return withConfigured(p);
}

function withConfigured(p: AiProvider): AiProvider {
  const configured = p.kind === 'cli' ? Boolean(p.path) : Boolean(p.base_url) && (!p.needs_key || p.has_key);
  return { ...p, configured };
}

/** The built-in presets plus one user provider, so the "My providers" list is not empty. */
export function seedProviders(): AiProvider[] {
  return [
    provider('claude-code', 'Claude Code', 'cli', 'claude-code', '', {
      path: 'C:\\Users\\you\\.local\\bin\\claude.exe',
      models: ['opus', 'sonnet', 'haiku', 'fable'],
    }),
    provider('codex', 'Codex', 'cli', 'codex', ''),
    provider('anthropic', 'Anthropic', 'api', 'anthropic', 'https://api.anthropic.com', {
      model: 'claude-opus-5-5',
      models: ['claude-opus-5-5', 'claude-sonnet-5-5', 'claude-haiku-5-5', 'claude-fable-5-1'],
    }),
    provider('openai', 'OpenAI', 'api', 'openai', 'https://api.openai.com/v1'),
    provider('gemini', 'Google Gemini', 'api', 'gemini', 'https://generativelanguage.googleapis.com'),
    provider('openrouter', 'OpenRouter', 'api', 'openai', 'https://openrouter.ai/api/v1'),
    provider('mistral', 'Mistral', 'api', 'openai', 'https://api.mistral.ai/v1'),
    provider('deepseek', 'DeepSeek', 'api', 'openai', 'https://api.deepseek.com/v1', {
      model: 'deepseek-chat',
      models: ['deepseek-chat', 'deepseek-reasoner'],
    }),
    provider('xai', 'xAI', 'api', 'openai', 'https://api.x.ai/v1'),
    provider('groq', 'Groq', 'api', 'openai', 'https://api.groq.com/openai/v1'),
    provider('ollama', 'Ollama', 'api', 'openai', 'http://localhost:11434/v1', { needs_key: false, model: 'qwen3:8b', models: ['qwen3:8b', 'llama3.3:70b'] }),
    provider('lmstudio', 'LM Studio', 'api', 'openai', 'http://localhost:1234/v1', { needs_key: false }),
    provider('home-server', 'Home server', 'api', 'openai', 'http://192.168.1.20:11434/v1', {
      builtin: false,
      template: 'ollama',
      needs_key: false,
      model: 'qwen3:32b',
      models: ['qwen3:32b', 'gemma3:27b'],
    }),
  ];
}

const invalid = (message: string) => new RpcError('INVALID_PARAMS', message);

function checkUrl(url: string): string {
  try {
    const u = new URL(url);
    if ((u.protocol === 'http:' || u.protocol === 'https:') && u.hostname) return url.replace(/\/+$/, '');
  } catch {
    // reported below
  }
  throw invalid(`bad base URL '${url}': it must start with http:// or https://`);
}

function slug(name: string): string {
  const s = name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return s || 'provider';
}

export function createProviderMocks(): { get: (id: string) => AiProvider; handlers: ProviderHandlers } {
  let providers = seedProviders();
  const seed = seedProviders();

  const get = (id: string) => {
    const p = providers.find((x) => x.id === id);
    if (!p) throw new RpcError('NOT_FOUND', `unknown AI provider '${id}'`);
    return p;
  };
  const nameTaken = (name: string, except?: string) => providers.some((p) => p.id !== except && p.name.trim().toLowerCase() === name.trim().toLowerCase());
  const checkName = (name: string, except?: string) => {
    const n = name.trim();
    if (!n) throw invalid('the provider name is empty');
    if (nameTaken(n, except)) throw invalid(`a provider named '${n}' already exists`);
    return n;
  };
  const freeId = (name: string) => {
    const base = slug(name);
    for (let n = 1; ; n++) {
      const id = n === 1 ? base : `${base}-${n}`;
      if (!providers.some((p) => p.id === id)) return id;
    }
  };
  const replace = (next: AiProvider, isDefault?: boolean) => {
    providers = providers.map((x) => (x.id === next.id ? next : isDefault ? { ...x, is_default: false } : x));
    return next;
  };

  const handlers: ProviderHandlers = {
    'ai.providers': () => ({ providers }),
    'ai.provider_add': ({ name, template, api, base_url, api_key, model, headers }) => {
      const clean = checkName(name);
      const from = template && template !== 'custom' ? providers.find((p) => p.id === template && p.builtin !== false && p.kind === 'api') : undefined;
      if (template && template !== 'custom' && !from) throw new RpcError('NOT_FOUND', `unknown template '${template}'`);
      const url = base_url?.trim() ? checkUrl(base_url.trim()) : from?.base_url;
      if (!url) throw invalid('base_url is required for a blank provider');
      const p = provider(freeId(clean), clean, 'api', (api ?? from?.protocol ?? 'openai') as AiApi, url, {
        builtin: false,
        template: from?.id ?? null,
        needs_key: from?.needs_key ?? false,
        has_key: Boolean(api_key?.trim()),
        model: model?.trim() || from?.model || null,
        models: from?.models ?? [],
        headers: headers ?? {},
      });
      providers = [...providers, p];
      return p;
    },
    'ai.provider_set': ({ id, name, api, api_key, base_url, model, path, headers, default: isDefault }) => {
      const p = get(id);
      const user = p.builtin === false;
      if (!user && (name !== undefined || api !== undefined)) throw invalid(`'${id}' is a built-in provider: its name and API type are fixed`);
      const next: AiProvider = { ...p };
      if (name !== undefined) next.name = checkName(name, id);
      if (api !== undefined) next.protocol = api;
      if (api_key !== undefined) next.has_key = api_key.trim() !== '';
      if (base_url !== undefined) {
        const url = base_url.trim();
        next.base_url = url ? checkUrl(url) : user ? '' : seed.find((x) => x.id === id)!.base_url;
      }
      if (model !== undefined) next.model = model.trim() || null;
      if (path !== undefined && p.kind === 'cli') next.path = path.trim() || null;
      if (headers !== undefined) next.headers = headers;
      if (isDefault !== undefined) next.is_default = isDefault;
      return replace(withConfigured(next), isDefault);
    },
    'ai.provider_remove': ({ id }) => {
      const p = get(id);
      if (p.builtin !== false) throw invalid(`'${id}' is a built-in provider and cannot be removed`);
      providers = providers.filter((x) => x.id !== id);
      return { id, was_default: Boolean(p.is_default) };
    },
    'ai.provider_test': async ({ id }) => {
      const p = get(id);
      await new Promise((r) => setTimeout(r, 450));
      if (p.kind === 'cli') {
        if (!p.path) return { id, ok: false, models: [], version: null, error: `${p.name} not found` };
        return { id, ok: true, models: [], version: p.id === 'codex' ? 'codex-cli 0.98.0' : '2.4.1 (Claude Code)', error: null };
      }
      if (!p.base_url) return { id, ok: false, models: [], version: null, error: 'no base URL set' };
      if (p.needs_key && !p.has_key) return { id, ok: false, models: [], version: null, error: 'no API key set' };
      const models = p.models.length ? p.models : [`${p.id}-large`, `${p.id}-small`];
      providers = providers.map((x) => (x.id === id ? { ...x, models, model: x.model ?? models[0] } : x));
      return { id, ok: true, models, version: null, error: null };
    },
  };
  return { get, handlers };
}
