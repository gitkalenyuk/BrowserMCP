import type { BrowserApi } from '../api/client';
import type {
  AiAgentAccess,
  AiAgentAccessSet,
  AiCodexApproval,
  AiCodexSandbox,
  AiPermission,
  AiPermissionDecision,
  AiPermissionMode,
  AiProvider,
  AiProviderSet,
  AiModels,
  AiProviderTest,
  AiSession,
  AiSessionStart,
  ApiEvents,
} from '../api/types';
import { useLiveProviders } from '../providers';
import { load, save } from '../storage';
import { afterClose, applyPermission, chatTitle, finishedIds, pickModel, sendTarget } from './sessions';
import {
  applyAiEvent,
  queuedCount,
  settleQueued,
  threadFromTranscript,
  threadMarkdown,
  truncateAtUser,
  withPending,
  type AiEvent,
  type ChatItem,
} from './thread';

/** SessionInfo fields an `ai.session` event may carry besides the status. */
const SESSION_FIELDS = ['title', 'title_set', 'archived', 'updated_ms', 'queued', 'model', 'tabs', 'access', 'color'] as const;

/** Overrides for the next new chat of a CLI provider (null / "" = the provider's default). */
export interface NewChatAccess {
  permission_mode: AiPermissionMode | null;
  sandbox: AiCodexSandbox | null;
  approval_policy: AiCodexApproval | null;
  cwd: string;
  /** Claude Code's extra folders, one per line; null = the provider's (untouched), "" = none. */
  add_dirs: string | null;
}

/** A textarea of folders, one per line, as a list (blank lines dropped). */
export function folderLines(text: string): string[] {
  return text
    .split('\n')
    .map((d) => d.trim())
    .filter(Boolean);
}

const NO_OVERRIDES: NewChatAccess = { permission_mode: null, sandbox: null, approval_policy: null, cwd: '', add_dirs: null };

/**
 * AI sessions as the chrome sees them: providers, sessions (agents), one chat thread per
 * session folded from ai.* events, and the approval requests waiting for an answer. Lives as
 * long as the chrome (events keep arriving while the panel is closed) and is shared by every
 * component through ChromeController.ai.
 */
export class AiStore {
  readonly api: BrowserApi;
  providers = $state<AiProvider[]>([]);
  providersError = $state<string | null>(null);
  sessions = $state<AiSession[]>([]);
  threads = $state<Record<string, ChatItem[]>>({});
  /** Approval requests of CLI agents waiting for an answer (all sessions), oldest first. */
  pending = $state<AiPermission[]>([]);
  /** CLI providers' defaults for new chats (`ai.agent_access`), by provider id. */
  access = $state<Record<string, AiAgentAccess>>({});
  /** Why `ai.agent_access` failed, by provider id (cleared by a successful load). */
  accessErrors = $state<Record<string, string>>({});
  /** Session shown in the panel; null = a new chat. */
  activeId = $state<string | null>(null);
  /** Text in the panel's input (kept here so "Ask AI about this" can fill it). */
  draft = $state('');
  /** Provider used for the next new chat. */
  provider = $state(load('ai.provider', 'claude-code'));
  /** Per-chat overrides of the next new chat (reset once it starts). */
  newChat = $state<NewChatAccess>({ ...NO_OVERRIDES });
  /** Closed (archived) chats, newest first; loaded when the History drawer opens. */
  history = $state<AiSession[]>([]);
  historyLoaded = $state(false);
  historyError = $state<string | null>(null);
  /** `ai.models` answers by provider id (the model pickers). */
  models = $state<Record<string, AiModels>>({});
  modelErrors = $state<Record<string, string>>({});
  /** The model picked for new chats, by provider (remembered across restarts). */
  newChatModel = $state<Record<string, string>>(load('ai.newChatModel', {}));
  #unsubs: Array<() => void> = [];
  #models = new Map<string, Promise<AiModels | null>>();
  #loaded = new Set<string>();
  #refreshTimer: ReturnType<typeof setTimeout> | null = null;

  active = $derived(this.sessions.find((s) => s.id === this.activeId) ?? null);
  currentProvider = $derived(this.providers.find((p) => p.id === this.provider) ?? null);

  constructor(api: BrowserApi) {
    this.api = api;
    // Hints of the user's own providers (providerHint) come from this live list.
    useLiveProviders(() => this.providers);
  }

  start(): void {
    const fold = (session: string, event: AiEvent) => {
      this.threads[session] = applyAiEvent(this.threads[session] ?? [], event, Date.now());
      if (!this.sessions.some((s) => s.id === session)) this.#refreshSoon();
    };
    this.#unsubs.push(
      this.api.on('ai.message', (p) => fold(p.session, { type: 'ai.message', payload: p })),
      this.api.on('ai.tool_call', (p) => fold(p.session, { type: 'ai.tool_call', payload: p })),
      this.api.on('ai.tool_result', (p) => fold(p.session, { type: 'ai.tool_result', payload: p })),
      this.api.on('ai.done', (p) => fold(p.session, { type: 'ai.done', payload: p })),
      this.api.on('ai.permission', (p) => {
        this.pending = applyPermission(this.pending, p);
        this.#countPending(p.session);
        fold(p.session, { type: 'ai.permission', payload: p });
      }),
      this.api.on('ai.session', (p) => this.#applySession(p)),
      this.api.on('ai.session_closed', (p) => {
        this.#forget(p.session);
        if (this.historyLoaded) void this.loadHistory();
      }),
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
    this.newChat = { ...NO_OVERRIDES };
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

  /** Sessions and the approval requests still waiting (a reloaded UI shows their cards again). */
  async refreshSessions(): Promise<void> {
    try {
      const { sessions } = await this.api.call('ai.sessions_list', {});
      this.sessions = sessions.filter((s) => !s.archived).sort((a, b) => b.created_ms - a.created_ms);
    } catch {
      // No AI service in this core: the panel still works for providers.
      return;
    }
    try {
      const { permissions } = await this.api.call('ai.permissions_list', {});
      this.pending = permissions;
      for (const id of Object.keys(this.threads)) this.#mergePending(id);
    } catch {
      // An older core without approvals.
    }
  }

  /** Show a chat (null = the new-chat page), loading its transcript if needed. */
  async open(id: string | null): Promise<void> {
    this.activeId = id;
    if (id) await this.ensureThread(id);
  }

  /** What a chat is called (its title, else its provider's name). */
  title(s: AiSession): string {
    return chatTitle(s, this.providerName(s.provider));
  }

  /** A chat (open or closed) as Markdown, loading its transcript if needed. */
  async markdownOf(id: string): Promise<string> {
    await this.ensureThread(id);
    const s = this.sessions.find((x) => x.id === id) ?? this.history.find((x) => x.id === id);
    return threadMarkdown(this.threads[id] ?? [], s ? this.title(s) : '');
  }

  /** Load a session's transcript once if no events were seen for it (e.g. after a reload). */
  async ensureThread(id: string): Promise<void> {
    if (this.#loaded.has(id) || (this.threads[id]?.length ?? 0) > 0) return;
    this.#loaded.add(id);
    try {
      const { entries } = await this.api.call('ai.session_transcript', { id });
      if ((this.threads[id]?.length ?? 0) === 0) this.threads[id] = threadFromTranscript(entries);
      this.#mergePending(id);
    } catch {
      this.#loaded.delete(id);
    }
  }

  /**
   * The next message of the shown chat (a stopped or failed chat continues with its history),
   * or a new chat with the selected provider when none is shown.
   */
  async send(text: string): Promise<void> {
    const prompt = text.trim();
    if (!prompt) return;
    const target = sendTarget(this.active);
    if (target.kind === 'continue') {
      this.#upsert(await this.api.call('ai.session_send', { id: target.id, text: prompt }));
      return;
    }
    const model = this.newModelFor(this.provider);
    const session = await this.api.call('ai.session_start', {
      provider: this.provider,
      prompt,
      ...(model ? { model } : {}),
      ...this.#newChatArgs(),
    });
    this.#loaded.add(session.id);
    this.#upsert(session);
    this.activeId = session.id;
    this.newChat = { ...NO_OVERRIDES };
  }

  /** Ends the agent's process; the chat stays and the next message continues it. */
  async stopSession(id: string): Promise<void> {
    this.#upsert(await this.api.call('ai.session_stop', { id }));
  }

  /** Stops the current turn only (Esc): the agent stays ready for the next message. */
  async interrupt(id: string): Promise<void> {
    this.#upsert(await this.api.call('ai.session_interrupt', { id }));
  }

  /** Messages of a chat waiting for the agent to take them. */
  queuedFor(id: string): number {
    return this.sessions.find((s) => s.id === id)?.queued ?? queuedCount(this.threads[id] ?? []);
  }

  /**
   * Replace user message number `ordinal` (0-based) with `text` and resend it: the thread is
   * cut to just before it at once; the new message and answer arrive as events.
   */
  async edit(id: string, ordinal: number, text: string): Promise<void> {
    const prompt = text.trim();
    if (!prompt) return;
    const before = this.threads[id] ?? [];
    this.threads[id] = truncateAtUser(before, ordinal);
    try {
      this.#upsert(await this.api.call('ai.session_edit', { id, index: ordinal, text: prompt }));
    } catch (err) {
      this.threads[id] = before;
      throw err;
    }
  }

  /** "" = back to the automatic title. */
  async rename(id: string, title: string): Promise<void> {
    this.#upsert(await this.api.call('ai.session_rename', { id, title: title.trim() }));
  }

  /** Close = archive: the chat leaves the tab strip and waits in History; nothing is deleted. */
  async closeSession(id: string, deleteTranscript = false): Promise<void> {
    await this.api.call('ai.session_close', deleteTranscript ? { id, delete_transcript: true } : { id });
    const closed = this.sessions.find((s) => s.id === id);
    this.#forget(id);
    if (closed && this.historyLoaded) {
      this.history = [{ ...closed, archived: true, status: closed.status === 'running' ? 'stopped' : closed.status }, ...this.history.filter((s) => s.id !== id)];
    }
  }

  /** Closed chats, newest first (`ai.sessions_list {archived: true}`). */
  async loadHistory(): Promise<void> {
    try {
      const { sessions } = await this.api.call('ai.sessions_list', { archived: true });
      this.history = sessions;
      this.historyError = null;
    } catch (err) {
      this.historyError = (err as Error).message;
    }
    this.historyLoaded = true;
  }

  /** Bring a closed chat back to the tab strip with its history, and show it. */
  async reopen(id: string): Promise<void> {
    const session = await this.api.call('ai.session_reopen', { id });
    this.history = this.history.filter((s) => s.id !== id);
    delete this.threads[id];
    this.#loaded.delete(id);
    this.#upsert({ ...session, archived: false });
    await this.open(id);
  }

  /** Permanently delete a chat, open or closed, with every file of it. */
  async deleteSession(id: string): Promise<void> {
    await this.api.call('ai.session_delete', { id });
    this.history = this.history.filter((s) => s.id !== id);
    this.#forget(id);
  }

  /** Permanently delete every closed chat. Returns how many were deleted. */
  async deleteAllClosed(): Promise<number> {
    let n = 0;
    for (const s of [...this.history]) {
      await this.deleteSession(s.id);
      n++;
    }
    return n;
  }

  /** A provider's models for the pickers (loaded once; `modelErrors` says why one failed). */
  loadModels(provider: string, force = false): Promise<AiModels | null> {
    const cached = this.#models.get(provider);
    if (cached && !force) return cached;
    const job = this.api.call('ai.models', { provider }).then(
      (models) => {
        this.models[provider] = models;
        delete this.modelErrors[provider];
        return models;
      },
      (err: Error) => {
        this.modelErrors[provider] = err.message;
        this.#models.delete(provider);
        return null;
      },
    );
    this.#models.set(provider, job);
    return job;
  }

  /** The model a new chat with `provider` starts with ("" = the provider's default). */
  newModelFor(provider: string): string {
    return pickModel(this.models[provider]?.models, this.newChatModel[provider]);
  }

  /** The model of new chats with `provider` ("" = the provider's default); remembered. */
  setNewChatModel(provider: string, model: string): void {
    if (model) this.newChatModel[provider] = model;
    else delete this.newChatModel[provider];
    save('ai.newChatModel', $state.snapshot(this.newChatModel));
  }

  /** Change a chat's model; it takes effect from the next turn. */
  async setModel(id: string, model: string): Promise<void> {
    this.#upsert(await this.api.call('ai.session_set_model', { id, model }));
  }

  /** Close every chat that is not working (see `finishedIds`). Returns how many closed. */
  async closeFinished(): Promise<number> {
    const ids = finishedIds(this.sessions, this.pending);
    for (const id of ids) await this.closeSession(id);
    return ids.length;
  }

  async answerPermission(id: string, decision: AiPermissionDecision, reason?: string): Promise<void> {
    const { resolved } = await this.api.call('ai.permission_answer', { id, decision, reason: reason?.trim() || undefined });
    // The ai.permission events do the same; this keeps the card from lingering if they are late.
    for (const r of resolved) {
      this.pending = applyPermission(this.pending, r);
      this.threads[r.session] = applyAiEvent(this.threads[r.session] ?? [], { type: 'ai.permission', payload: r });
      this.#countPending(r.session);
    }
  }

  pendingFor(session: string): AiPermission[] {
    return this.pending.filter((p) => p.session === session);
  }

  /** A provider's defaults; null when they could not be read (`accessErrors` says why). */
  async loadAccess(provider: string): Promise<AiAgentAccess | null> {
    try {
      const access = await this.api.call('ai.agent_access', { id: provider });
      this.access[provider] = access;
      delete this.accessErrors[provider];
      return access;
    } catch (err) {
      this.accessErrors[provider] = (err as Error).message;
      return null;
    }
  }

  async saveAccess(args: AiAgentAccessSet): Promise<AiAgentAccess> {
    const access = await this.api.call('ai.agent_access_set', args);
    this.access[args.id] = access;
    return access;
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

  /** ai.session_start fields for the overrides the user picked for this chat. */
  #newChatArgs(): Partial<AiSessionStart> {
    const kind = this.currentProvider?.protocol;
    const args: Partial<AiSessionStart> = {};
    if (kind !== 'claude-code' && kind !== 'codex') return args;
    const pick = this.newChat;
    if (kind === 'claude-code') {
      if (pick.permission_mode) args.permission_mode = pick.permission_mode;
      // Sent once edited, even empty: [] drops the provider's extra folders for this chat.
      if (pick.add_dirs !== null) args.add_dirs = folderLines(pick.add_dirs);
    } else {
      if (pick.sandbox) args.sandbox = pick.sandbox;
      if (pick.approval_policy) args.approval_policy = pick.approval_policy;
    }
    if (pick.cwd.trim()) args.cwd = pick.cwd.trim();
    return args;
  }

  #mergePending(id: string): void {
    const pending = this.pendingFor(id);
    if (pending.length && this.threads[id]) this.threads[id] = withPending(this.threads[id], pending);
  }

  #countPending(id: string): void {
    const s = this.sessions.find((x) => x.id === id);
    if (s) s.pending_permissions = this.pendingFor(id).length;
  }

  #forget(id: string): void {
    const order = this.sessions.map((s) => s.id);
    this.activeId = afterClose(order, id, this.activeId);
    this.sessions = this.sessions.filter((s) => s.id !== id);
    this.pending = this.pending.filter((p) => p.session !== id);
    delete this.threads[id];
    this.#loaded.delete(id);
  }

  #upsert(session: AiSession): void {
    if (session.archived) {
      // A closed chat (renamed in History, say) stays in History.
      this.history = this.history.map((s) => (s.id === session.id ? session : s));
      if (this.sessions.some((s) => s.id === session.id)) this.#forget(session.id);
      return;
    }
    const i = this.sessions.findIndex((s) => s.id === session.id);
    if (i < 0) this.sessions = [session, ...this.sessions];
    else this.sessions[i] = session;
  }

  /** An `ai.session` event: the status, plus whatever SessionInfo fields a newer core sends. */
  #applySession(p: ApiEvents['ai.session']): void {
    const s = this.sessions.find((x) => x.id === p.session);
    if (!s) {
      if (!p.archived) this.#refreshSoon();
      return;
    }
    if (p.archived) {
      this.#forget(p.session);
      if (this.historyLoaded) void this.loadHistory();
      return;
    }
    s.status = p.status;
    const fields = s as unknown as Record<string, unknown>;
    for (const key of SESSION_FIELDS) if (p[key] !== undefined) fields[key] = p[key];
    if (p.queued !== undefined && this.threads[p.session]) this.threads[p.session] = settleQueued(this.threads[p.session], p.queued);
  }

  #refreshSoon(): void {
    if (this.#refreshTimer) return;
    this.#refreshTimer = setTimeout(() => {
      this.#refreshTimer = null;
      void this.refreshSessions();
    }, 150);
  }
}
