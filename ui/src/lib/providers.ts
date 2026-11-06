export interface Provider {
  id: string;
  name: string;
  /** How the core talks to it (shown as a hint). */
  via: string;
}

export const PROVIDERS: Provider[] = [
  { id: 'claude-code', name: 'Claude Code', via: 'claude -p, stream-json' },
  { id: 'codex', name: 'Codex', via: 'codex app-server' },
  { id: 'anthropic', name: 'Anthropic API', via: 'native' },
  { id: 'openai', name: 'OpenAI', via: 'OpenAI-compatible' },
  { id: 'gemini', name: 'Gemini', via: 'native' },
  { id: 'openrouter', name: 'OpenRouter', via: 'OpenAI-compatible' },
  { id: 'ollama', name: 'Ollama', via: 'OpenAI-compatible, local' },
  { id: 'custom', name: 'Custom endpoint', via: 'OpenAI-compatible' },
];
