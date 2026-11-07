// Static facts about the core's built-in AI providers (crates/ai/src/config.rs, PRESETS), and
// short hints for every provider. The live list, with configured/key/model state and the
// user's own providers, comes from `ai.providers`; the static list is the fallback before it
// arrives (or against a core without the AI service).

import type { AiProvider } from './api';
import type { AiApi } from './api/types-ai';
import { t } from './i18n/index.svelte';

export interface Provider {
  id: string;
  name: string;
  /** i18n key of how the core talks to it (shown as a hint). */
  via: string;
}

export const PROVIDERS: Provider[] = [
  { id: 'claude-code', name: 'Claude Code', via: 'aiProviders.via.claudeCode' },
  { id: 'codex', name: 'Codex', via: 'aiProviders.via.codex' },
  { id: 'anthropic', name: 'Anthropic', via: 'aiProviders.via.anthropic' },
  { id: 'openai', name: 'OpenAI', via: 'aiProviders.via.openai' },
  { id: 'gemini', name: 'Google Gemini', via: 'aiProviders.via.gemini' },
  { id: 'openrouter', name: 'OpenRouter', via: 'aiProviders.via.compatible' },
  { id: 'mistral', name: 'Mistral', via: 'aiProviders.via.compatible' },
  { id: 'deepseek', name: 'DeepSeek', via: 'aiProviders.via.compatible' },
  { id: 'xai', name: 'xAI', via: 'aiProviders.via.compatible' },
  { id: 'groq', name: 'Groq', via: 'aiProviders.via.compatible' },
  { id: 'ollama', name: 'Ollama', via: 'aiProviders.via.local' },
  { id: 'lmstudio', name: 'LM Studio', via: 'aiProviders.via.local' },
];

/** Template id of a blank OpenAI-compatible endpoint (`ai.provider_add {template}`). */
export const BLANK_TEMPLATE = 'custom';

/** The API types a user provider can speak, with their i18n label keys. */
export const APIS: { id: AiApi; label: string }[] = [
  { id: 'openai', label: 'aiProviders.api.openai' },
  { id: 'anthropic', label: 'aiProviders.api.anthropic' },
  { id: 'gemini', label: 'aiProviders.api.gemini' },
];

export function apiLabel(api: string): string {
  const entry = APIS.find((a) => a.id === api);
  return entry ? t(entry.label) : api;
}

// The live provider list of this page's AI store, so hints also cover the user's own
// providers. A getter (not a copy) keeps `providerHint` reactive in markup.
let live: () => readonly AiProvider[] = () => [];

/** Called by the AI store: where `providerHint` finds the live providers. */
export function useLiveProviders(get: () => readonly AiProvider[]): void {
  live = get;
}

/** Host of an API root, for hints ("localhost:11434"). */
export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return '';
  }
}

/** Short "how it connects" hint; call it in markup or `$derived` (it is translated). */
export function providerHint(id: string): string {
  const known = PROVIDERS.find((p) => p.id === id);
  if (known) return t(known.via);
  const p = live().find((x) => x.id === id);
  if (!p) return '';
  const host = hostOf(p.base_url);
  return host ? t('aiProviders.via.user', { api: apiLabel(p.protocol), host }) : apiLabel(p.protocol);
}

/** Providers offered for a whole task on the new tab page (they work in their own tab group). */
export const TASK_AGENTS = ['claude-code', 'codex', 'anthropic'];
