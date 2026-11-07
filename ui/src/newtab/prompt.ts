// Pure helpers of the new tab's task box (PromptWidget): the model picker's remembered choice
// and the short title shown once an agent took the task.

import type { AiModels } from '../lib/api';

/** Remembered model per provider id ('' = the provider's default). */
export type ModelChoices = Record<string, string>;

/**
 * The model to preselect for a provider: the remembered one while the provider still offers
 * it, otherwise '' (= its default, nothing is passed to ai.session_start).
 */
export function pickModel(choices: ModelChoices, provider: string, offered: AiModels | null): string {
  const remembered = choices[provider] ?? '';
  if (!remembered || !offered) return '';
  return offered.models.some((m) => m.id === remembered) ? remembered : '';
}

/** The choices with `model` remembered for `provider` ('' forgets it). */
export function rememberModel(choices: ModelChoices, provider: string, model: string): ModelChoices {
  const next = { ...choices };
  if (model) next[provider] = model;
  else delete next[provider];
  return next;
}

/** The models to list: the core's own `default` entry is the picker's "Default" option already. */
export function listedModels(offered: AiModels | null): AiModels['models'] {
  return (offered?.models ?? []).filter((m) => m.id !== 'default');
}

/** Name of the default model for the "Default (…)" option, if the core names a real one. */
export function defaultModelName(offered: AiModels | null): string | null {
  if (!offered?.default || offered.default === 'default') return null;
  return offered.models.find((m) => m.id === offered.default)?.name ?? offered.default;
}

/**
 * What the success message calls the new chat: the session's own title when the core gives
 * one, else the first line of the task, shortened to `max` characters.
 */
export function chatTitle(task: string, sessionTitle?: string | null, max = 60): string {
  const own = sessionTitle?.trim();
  if (own) return own;
  const line = task.trim().split('\n')[0].replace(/\s+/g, ' ');
  return line.length <= max ? line : `${line.slice(0, max - 1).trimEnd()}…`;
}
