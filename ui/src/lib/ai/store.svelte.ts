import type { BrowserApi } from '../api/client';
import type { AiProvider, AiProviderSet, AiProviderTest, AiSession, AiSessionStatus } from '../api/types';
import { load, save } from '../storage';
import { applyAiEvent, threadFromTranscript, type AiEvent, type ChatItem } from './thread';

/**
 * AI sessions as the chrome sees them: providers, sessions (agents) and one chat thread per
 * session, folded from ai.* events. Lives as long as the chrome (events keep arriving while
 * the panel is closed) and is shared by every component through ChromeController.ai.
 */
export class AiStore {
  readonly api: BrowserApi;
  providers = $state<AiProvider[]>([]);
  providersError = $state<string | null>(null);
  sessions = $state<AiSession[]>([]);
  threads = $state<Record<string, ChatItem[]>>({});
  /** Session shown in the panel; null = a new chat. */
  activeId = $state<string | null>(null);
  /** Provider used for the next new chat. */
  provider = $state(load('ai.provider', 'claude-code'));
  #unsubs: Array<() => void> = [];
  #loaded = new Set<string>();
  #refreshTimer: ReturnType<typeof setTimeout> | null = null;

  active = $derived(this.sessions.find((s) => s.id === this.activeId) ?? null);
  currentProvider = $derived(this.providers.find((p) => p.id === this.provider) ?? null);

  constructor(api: BrowserApi) {
    this.api = api;
  }

  start(): void {
    const fold = (session: string, event: AiEvent) => {
      this.threads[session] = applyAiEvent(this.threads[session] ?? [], event);
      if (!this.sessions.some((s) => s.id === session)) this.#refreshSoon();
    };
    this.#unsubs.push(
      this.api.on('ai.message', (p) => fold(p.session, { type: 'ai.message', payload: p })),
      this.api.on('ai.tool_call', (p) => fold(p.session, { type: 'ai.tool_call', payload: p })),
      this.api.on('ai.tool_result', (p) => fold(p.session, { type: 'ai.tool_result', payload: p })),
      this.api.on('ai.done', (p) => fold(p.session, { type: 'ai.done', payload: p })),
      this.api.on('ai.session', (p) => this.#setStatus(p.session, p.status)),
      this.api.onStatus((s) => {
        if (s === 'open') void this.refreshSessions();
      }),
    );
  }

  stop(): void {
    this.#unsubs.forEach((u) => u());
    this.#unsubs = [];
  }

  setProvider(id: string): void {
    this.provider = id;
    save('ai.provider', id);
  }

  async refreshProviders(): Promise<void> {
    try {
      const { providers } = await this.api.call('ai.providers');
      this.providers = providers;
      this.providersError = null;
      if (!providers.some((p) => p.id === this.provider) && providers[0]) this.setProvider(providers[0].id);
    } catch (err) {
      this.providersError = (err as Error).message;
    }
  }

  async refreshSessions(): Promise<void> {
    try {
      const { sessions } = await this.api.call('ai.sessions_list');
      this.sessions = sessions.sort((a, b) => b.created_ms - a.created_ms);
    } catch {
      // No AI service in this core: the panel still works for providers.
    }
  }

  /** Load a session's transcript once if no events were seen for it (e.g. after a reload). */
  async open(id: string | null): Promise<void> {
    this.activeId = id;
    if (!id || this.#loaded.has(id) || (this.threads[id]?.length ?? 0) > 0) return;
    this.#loaded.add(id);
    try {
      const { entries } = await this.api.call('ai.session_transcript', { id });
      if ((this.threads[id]?.length ?? 0) === 0) this.threads[id] = threadFromTranscript(entries);
    } catch {
      this.#loaded.delete(id);
    }
  }

  /** New chat with the selected provider, or the next message of the active session. */
  async send(text: string): Promise<void> {
    const prompt = text.trim();
    if (!prompt) return;
    const s = this.active;
    if (s && (s.status === 'idle' || s.status === 'running')) {
      this.#upsert(await this.api.call('ai.session_send', { id: s.id, text: prompt }));
      return;
    }
    const session = await this.api.call('ai.session_start', { provider: this.provider, prompt });
    this.#loaded.add(session.id);
    this.#upsert(session);
    this.activeId = session.id;
  }

  async stopSession(id: string): Promise<void> {
    this.#upsert(await this.api.call('ai.session_stop', { id }));
  }

  async saveProvider(args: AiProviderSet): Promise<AiProvider> {
    const p = await this.api.call('ai.provider_set', args);
    this.providers = this.providers.map((x) => (x.id === p.id ? p : x));
    return p;
  }

  async testProvider(id: string): Promise<AiProviderTest> {
    const result = await this.api.call('ai.provider_test', { id });
    // A successful test may have saved a live model list.
    if (result.ok) void this.refreshProviders();
    return result;
  }

  providerName(id: string): string {
    return this.providers.find((p) => p.id === id)?.name ?? id;
  }

  #upsert(session: AiSession): void {
    const i = this.sessions.findIndex((s) => s.id === session.id);
    if (i < 0) this.sessions = [session, ...this.sessions];
    else this.sessions[i] = session;
  }

  #setStatus(id: string, status: AiSessionStatus): void {
    const s = this.sessions.find((x) => x.id === id);
    if (s) s.status = status;
    else this.#refreshSoon();
  }

  #refreshSoon(): void {
    if (this.#refreshTimer) return;
    this.#refreshTimer = setTimeout(() => {
      this.#refreshTimer = null;
      void this.refreshSessions();
    }, 150);
  }
}
