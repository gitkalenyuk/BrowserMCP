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
  openAnswer,
  queuedCount,
  settleQueued,
  threadFromTranscript,
  threadMarkdown,
  truncateAtUser,
  userIndexAt,
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

/** One change to a thread, kept while its transcript loads so it can be replayed on top. */
type ThreadStep = (items: ChatItem[]) => ChatItem[];

/** Chat statuses in which no agent runs: queued messages wait for the next message. */
const isEnded = (status: string | undefined): boolean => status === 'stopped' || status === 'failed';

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
  /** Threads that hold the whole chat (loaded from its transcript, or seen from its start). */
  #loaded = new Set<string>();
  /** Transcript loads in flight, by session. */
  #loading = new Map<string, Promise<void>>();
  /** Live changes that arrived while a session's transcript loads (replayed on top of it). */
  #buffers = new Map<string, ThreadStep[]>();
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
      const ts = Date.now();
      this.#step(session, (items) => applyAiEvent(items, event, ts));
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
      // The agent took a message sent while it was busy: it joins the conversation.
      this.api.on('ai.message_taken', (p) =>
        fold(p.session, { type: 'ai.message', payload: { session: p.session, role: 'user', taken: true, index: p.index } }),
      ),
      // An edit (here or in another window) cut the chat before user message `index`.
      this.api.on('ai.session_rewound', (p) => this.#step(p.session, (items) => truncateAtUser(items, p.index))),
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

  /**
   * Load a session's transcript once, unless this window saw the chat from its start. Live
   * events seen before (after a reload, in a second window) only hold the newest part of the
   * chat: the thread is rebuilt from the transcript, keeping the answer streaming now (deltas
   * are not in the transcript) and replaying what arrived while it loaded.
   */
  ensureThread(id: string): Promise<void> {
    if (this.#loaded.has(id)) return Promise.resolve();
    let job = this.#loading.get(id);
    if (!job) {
      job = this.#loadThread(id);
      this.#loading.set(id, job);
    }
    return job;
  }

  async #loadThread(id: string): Promise<void> {
    const buffer: ThreadStep[] = [];
    this.#buffers.set(id, buffer);
    const open = openAnswer(this.threads[id] ?? []);
    try {
      const { entries } = await this.api.call('ai.session_transcript', { id });
      if (this.#buffers.get(id) !== buffer) return; // forgotten or reset meanwhile
      let items = threadFromTranscript(entries);
      if (open) {
        items = applyAiEvent(items, { type: 'ai.message', payload: { session: id, role: 'assistant', text_delta: open.text } }, open.ts);
      }
      for (const step of buffer) items = step(items);
      this.threads[id] = items;
      this.#loaded.add(id);
      this.#mergePending(id);
    } catch {
      // The live rows stay; the next open tries again.
    } finally {
      if (this.#buffers.get(id) === buffer) {
        this.#buffers.delete(id);
        this.#loading.delete(id);
      }
    }
  }

  /**
   * The core's number of the user message at row `row` of a chat (what `ai.session_edit`
   * takes), or null when it cannot be told. A row without its number in a thread that holds
   * only part of the chat is looked up again in the freshly loaded transcript.
   */
  async userIndex(id: string, row: number): Promise<number | null> {
    const items = this.threads[id] ?? [];
    const item = items[row];
    if (item?.kind !== 'user' || item.queued) return null;
    if (item.index !== undefined) return item.index;
    if (this.#loaded.has(id)) return userIndexAt(items, row);
    const isUser = (i: ChatItem) => i.kind === 'user' && !i.queued;
    const fromEnd = items.slice(row + 1).filter(isUser).length;
    await this.ensureThread(id);
    if (!this.#loaded.has(id)) return null;
    const fresh = this.threads[id] ?? [];
    const users = fresh.flatMap((i, at) => (isUser(i) ? [at] : []));
    const at = users[users.length - 1 - fromEnd];
    const match = at === undefined ? undefined : fresh[at];
    if (match?.kind !== 'user' || match.text !== item.text) return null;
    const index = userIndexAt(fresh, at);
    return index < 0 ? null : index;
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
   * Replace user message number `index` (the core's number, see `userIndex`) with `text` and
   * resend it: the thread is cut to just before it at once; the new message and answer
   * arrive as events.
   */
  async edit(id: string, index: number, text: string): Promise<void> {
    const prompt = text.trim();
    if (!prompt) return;
    this.#step(id, (items) => truncateAtUser(items, index));
    try {
      this.#upsert(await this.api.call('ai.session_edit', { id, index, text: prompt }));
    } catch (err) {
      // The core may have cut the transcript before failing (it rewinds, then restarts the
      // agent): show what it has now rather than the old thread.
      this.#resetThread(id);
      await this.ensureThread(id);
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
    this.#resetThread(id);
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
      this.#step(r.session, (items) => applyAiEvent(items, { type: 'ai.permission', payload: r }));
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
    this.#resetThread(id);
  }

  /** Apply a change to a thread (and keep it for the replay when its transcript is loading). */
  #step(id: string, step: ThreadStep): void {
    this.threads[id] = step(this.threads[id] ?? []);
    this.#buffers.get(id)?.push(step);
  }

  /** Forget a thread; the next open loads it again (a load in flight is dropped). */
  #resetThread(id: string): void {
    delete this.threads[id];
    this.#loaded.delete(id);
    this.#buffers.delete(id);
    this.#loading.delete(id);
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
      if (p.archived) {
        // A closed chat changed (renamed, say): History shows it as it is now.
        this.history = this.history.map((h) => (h.id === p.session ? this.#merged(h, p) : h));
        return;
      }
      // Reopened elsewhere: it is open again, not in History (where Delete would remove it).
      if (p.archived === false) this.history = this.history.filter((h) => h.id !== p.session);
      this.#refreshSoon();
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
    // A stopped or failed chat keeps its queued messages: they go with the next message
    // (ai.message_taken then moves each into the conversation), so none is settled here.
    const queued = p.queued;
    if (queued !== undefined && !isEnded(p.status) && this.threads[p.session]) {
      this.#step(p.session, (items) => settleQueued(items, queued));
    }
  }

  /** A session with the SessionInfo fields an `ai.session` event carries. */
  #merged(s: AiSession, p: ApiEvents['ai.session']): AiSession {
    const next = { ...s, status: p.status } as unknown as Record<string, unknown>;
    for (const key of SESSION_FIELDS) if (p[key] !== undefined) next[key] = p[key];
    return next as unknown as AiSession;
  }

  #refreshSoon(): void {
    if (this.#refreshTimer) return;
    this.#refreshTimer = setTimeout(() => {
      this.#refreshTimer = null;
      void this.refreshSessions();
    }, 150);
  }
}
