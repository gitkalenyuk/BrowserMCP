// Static facts about the core's AI providers (crates/ai/src/config.rs, PRESETS). The live list,
// with configured/key/model state, comes from `ai.providers`; this is the fallback before it
// arrives (or against a core without the AI service) and the source of the short hints.

export interface Provider {
  id: string;
  name: string;
  /** How the core talks to it (shown as a hint). */
  via: string;
}

export const PROVIDERS: Provider[] = [
  { id: 'claude-code', name: 'Claude Code', via: 'claude CLI, stream-json' },
  { id: 'codex', name: 'Codex', via: 'codex app-server' },
  { id: 'anthropic', name: 'Anthropic', via: 'Messages API' },
  { id: 'openai', name: 'OpenAI', via: 'OpenAI API' },
  { id: 'gemini', name: 'Google Gemini', via: 'Gemini API' },
  { id: 'openrouter', name: 'OpenRouter', via: 'OpenAI-compatible' },
  { id: 'mistral', name: 'Mistral', via: 'OpenAI-compatible' },
  { id: 'deepseek', name: 'DeepSeek', via: 'OpenAI-compatible' },
  { id: 'xai', name: 'xAI', via: 'OpenAI-compatible' },
  { id: 'groq', name: 'Groq', via: 'OpenAI-compatible' },
  { id: 'ollama', name: 'Ollama', via: 'OpenAI-compatible, local' },
  { id: 'lmstudio', name: 'LM Studio', via: 'OpenAI-compatible, local' },
  { id: 'custom', name: 'Custom (OpenAI-compatible)', via: 'OpenAI-compatible' },
];

export function providerHint(id: string): string {
  return PROVIDERS.find((p) => p.id === id)?.via ?? '';
}

/** Providers offered for a whole task on the new tab page (they work in their own tab group). */
export const TASK_AGENTS = ['claude-code', 'codex', 'anthropic'];
