import { describe, expect, it } from 'vitest';
import type { BrowserApi } from '../api/client';
import type { AiPermission, AiSession, ApiEvents, CallArgs, EventName, Method, Result } from '../api/types';
import { AiStore } from './store.svelte';
import type { ChatItem } from './thread';

/** Records calls, answers them from `replies`, and lets the test emit events. */
class FakeApi implements BrowserApi {
  readonly kind = 'mock' as const;
  readonly status = 'open' as const;
  calls: { method: string; params: unknown }[] = [];
  #handlers = new Map<string, Set<(p: unknown) => void>>();

  constructor(readonly replies: Partial<Record<Method, (params: never) => unknown>>) {}

  async connect(): Promise<void> {}
  close(): void {}

  async call<M extends Method>(method: M, ...args: CallArgs<M>): Promise<Result<M>> {
    this.calls.push({ method, params: args[0] });
    const reply = this.replies[method];
    if (!reply) throw new Error(`unexpected call ${method}`);
    return reply(args[0] as never) as Result<M>;
  }

  on<E extends EventName>(event: E, handler: (payload: ApiEvents[E]) => void): () => void {
    if (!this.#handlers.has(event)) this.#handlers.set(event, new Set());
    this.#handlers.get(event)!.add(handler as (p: unknown) => void);
    return () => this.#handlers.get(event)!.delete(handler as (p: unknown) => void);
  }

  onStatus(): () => void {
    return () => {};
  }

  emit<E extends EventName>(event: E, payload: ApiEvents[E]): void {
    this.#handlers.get(event)?.forEach((h) => h(payload));
  }

  methods(): string[] {
    return this.calls.map((c) => c.method);
  }
}

const session = (id: string, status: AiSession['status'], created_ms = 1): AiSession => ({
  id,
  provider: 'claude-code',
  model: null,
  color: '#3b82f6',
  status,
  tabs: [],
  created_ms,
});

const permission = (id: string, s: string, status: AiPermission['status'] = 'pending'): AiPermission => ({
  id,
  session: s,
  tool: 'Bash',
  kind: 'tool',
  title: null,
  reason: null,
  input: { command: 'ls' },
  status,
  always: false,
  answer: null,
  created_ms: 1,
});

describe('AiStore', () => {
  it('a message in a stopped chat continues that chat, never a new one', async () => {
    const api = new FakeApi({
      'ai.sessions_list': () => ({ sessions: [session('claude-code-1', 'stopped')] }),
      'ai.permissions_list': () => ({ permissions: [] }),
      'ai.session_send': (p: { id: string }) => ({ ...session(p.id, 'running') }),
    });
    const ai = new AiStore(api);
    await ai.refreshSessions();
    ai.activeId = 'claude-code-1';
    await ai.send('again');
    expect(api.methods()).not.toContain('ai.session_start');
    expect(api.calls.at(-1)).toEqual({ method: 'ai.session_send', params: { id: 'claude-code-1', text: 'again' } });
    expect(ai.activeId).toBe('claude-code-1');
    expect(ai.sessions[0].status).toBe('running');
  });

  it('a new chat of a CLI provider carries the chosen permissions and folder', async () => {
    const api = new FakeApi({
      'ai.providers': () => ({
        providers: [
          {
            id: 'claude-code',
            name: 'Claude Code',
            kind: 'cli',
            protocol: 'claude-code',
            configured: true,
            needs_key: false,
            has_key: false,
            base_url: '',
            model: null,
            models: [],
            path: 'claude.exe',
          },
        ],
      }),
      'ai.session_start': () => session('claude-code-2', 'running'),
    });
    const ai = new AiStore(api);
    await ai.refreshProviders();
    ai.setProvider('claude-code');
    ai.newChat.permission_mode = 'plan';
    ai.newChat.cwd = '  D:\\work ';
    await ai.send('hello');
    expect(api.calls.at(-1)?.params).toEqual({ provider: 'claude-code', prompt: 'hello', permission_mode: 'plan', cwd: 'D:\\work' });
    expect(ai.activeId).toBe('claude-code-2');
    expect(ai.newChat).toEqual({ permission_mode: null, sandbox: null, approval_policy: null, cwd: '', add_dirs: null });
  });

  it('extra folders are sent once edited (empty = none); Codex gets its approval policy', async () => {
    const cli = (id: string, protocol: string) => ({
      id,
      name: id,
      kind: 'cli',
      protocol,
      configured: true,
      needs_key: false,
      has_key: false,
      base_url: '',
      model: null,
      models: [],
      path: `${id}.exe`,
    });
    const api = new FakeApi({
      'ai.providers': () => ({ providers: [cli('claude-code', 'claude-code'), cli('codex', 'codex')] }),
      'ai.session_start': (p: { provider: string }) => session(`${p.provider}-9`, 'running'),
    });
    const ai = new AiStore(api);
    await ai.refreshProviders();
    ai.setProvider('claude-code');
    ai.newChat.add_dirs = ' \n ';
    ai.newChat.approval_policy = 'never'; // not a Claude Code setting
    await ai.send('one');
    expect(api.calls.at(-1)?.params).toEqual({ provider: 'claude-code', prompt: 'one', add_dirs: [] });

    await ai.open(null); // "+ New"
    ai.setProvider('codex');
    ai.newChat.approval_policy = 'untrusted';
    ai.newChat.sandbox = 'read_only';
    await ai.send('two');
    expect(api.calls.at(-1)?.params).toEqual({ provider: 'codex', prompt: 'two', sandbox: 'read_only', approval_policy: 'untrusted' });
  });

  it('a failed access load is remembered, a later one clears it', async () => {
    let fail = true;
    const api = new FakeApi({
      'ai.agent_access': () => {
        if (fail) throw new Error('core is busy');
        return {
          provider: 'claude-code',
          permission_mode: 'ask',
          cwd: null,
          default_cwd: 'C:/data/ai/workspace',
          add_dirs: [],
          sandbox: 'workspace_write',
          approval_policy: 'on_request',
        };
      },
    });
    const ai = new AiStore(api);
    expect(await ai.loadAccess('claude-code')).toBeNull();
    expect(ai.accessErrors['claude-code']).toBe('core is busy');
    fail = false;
    expect(await ai.loadAccess('claude-code')).not.toBeNull();
    expect(ai.accessErrors['claude-code']).toBeUndefined();
  });

  it('approval events fill the card list and the thread; answering resolves them', async () => {
    const api = new FakeApi({
      'ai.permission_answer': (p: { id: string }) => ({ resolved: [{ ...permission(p.id, 'claude-code-1', 'allowed') }] }),
    });
    const ai = new AiStore(api);
    ai.sessions = [session('claude-code-1', 'running')];
    ai.start();
    api.emit('ai.permission', permission('claude-code-1.p1', 'claude-code-1'));
    expect(ai.pendingFor('claude-code-1')).toHaveLength(1);
    expect(ai.sessions[0].pending_permissions).toBe(1);
    expect(ai.threads['claude-code-1'].at(-1)?.kind).toBe('permission');

    await ai.answerPermission('claude-code-1.p1', 'deny', '  ');
    expect(api.calls.at(-1)).toEqual({
      method: 'ai.permission_answer',
      params: { id: 'claude-code-1.p1', decision: 'deny', reason: undefined },
    });
    expect(ai.pending).toEqual([]);
    expect(ai.sessions[0].pending_permissions).toBe(0);
    ai.stop();
  });

  it('a reload lists pending requests and shows their cards in the opened chat', async () => {
    const api = new FakeApi({
      'ai.sessions_list': () => ({ sessions: [session('claude-code-1', 'running')] }),
      'ai.permissions_list': () => ({ permissions: [permission('claude-code-1.p1', 'claude-code-1')] }),
      'ai.session_transcript': () => ({ id: 'claude-code-1', entries: [{ type: 'user', text: 'run bash' }] }),
    });
    const ai = new AiStore(api);
    await ai.refreshSessions();
    await ai.open('claude-code-1');
    expect(ai.threads['claude-code-1'].map((i) => i.kind)).toEqual(['user', 'permission']);
  });

  it('closing shows the neighbour; "close finished" skips working chats', async () => {
    const api = new FakeApi({
      'ai.session_close': (p: { id: string }) => ({ closed: p.id }),
    });
    const ai = new AiStore(api);
    ai.start();
    ai.sessions = [session('a', 'idle', 4), session('b', 'stopped', 3), session('c', 'running', 2), session('d', 'failed', 1)];
    ai.activeId = 'b';
    await ai.closeSession('b');
    expect(ai.activeId).toBe('c');
    expect(ai.sessions.map((s) => s.id)).toEqual(['a', 'c', 'd']);

    expect(await ai.closeFinished()).toBe(2);
    expect(ai.sessions.map((s) => s.id)).toEqual(['c']);
    expect(api.calls.map((c) => (c.params as { id: string }).id)).toEqual(['b', 'a', 'd']);

    // Closed in another window: gone here too, and the new-chat page is shown.
    api.emit('ai.session_closed', { session: 'c' });
    expect(ai.sessions).toEqual([]);
    expect(ai.activeId).toBeNull();
    ai.stop();
  });
});

describe('AiStore: names, history, models, queue, edit', () => {
  const titled = (id: string, title: string, extra: Partial<AiSession> = {}): AiSession => ({ ...session(id, 'idle'), title, ...extra });

  it('ai.session events carry the new fields: title, model, queued; archived leaves the strip', async () => {
    const api = new FakeApi({});
    const ai = new AiStore(api);
    ai.start();
    ai.sessions = [titled('a', 'Claude Code'), titled('b', 'x')];
    ai.threads.a = [
      { kind: 'user', text: 'one' },
      { kind: 'user', text: 'two', queued: true },
    ];
    api.emit('ai.session', { session: 'a', status: 'running', title: 'Flights to Lviv', model: 'opus', queued: 0 });
    expect(ai.sessions[0]).toMatchObject({ title: 'Flights to Lviv', model: 'opus', status: 'running', queued: 0 });
    // The core says nothing waits any more: the queued mark goes.
    expect(ai.threads.a[1]).toEqual({ kind: 'user', text: 'two' });
    api.emit('ai.session', { session: 'b', status: 'stopped', archived: true });
    expect(ai.sessions.map((s) => s.id)).toEqual(['a']);
    ai.stop();
  });

  it('close archives into History; reopen brings it back with its transcript; delete is permanent', async () => {
    const api = new FakeApi({
      'ai.session_close': (p: { id: string }) => ({ closed: p.id }),
      'ai.sessions_list': (p: { archived?: boolean }) => ({ sessions: p.archived ? [titled('old', 'Old one', { archived: true, status: 'stopped' })] : [] }),
      'ai.session_reopen': (p: { id: string }) => titled(p.id, 'Old one', { status: 'stopped' }),
      'ai.session_transcript': (p: { id: string }) => ({ id: p.id, entries: [{ type: 'user', text: 'hello' }] }),
      'ai.session_delete': () => ({ deleted: true }),
    });
    const ai = new AiStore(api);
    ai.sessions = [titled('a', 'Prices')];
    ai.activeId = 'a';
    await ai.loadHistory();
    expect(api.calls.at(-1)).toEqual({ method: 'ai.sessions_list', params: { archived: true } });
    await ai.closeSession('a');
    expect(api.calls.at(-1)).toEqual({ method: 'ai.session_close', params: { id: 'a' } });
    expect(ai.sessions).toEqual([]);
    expect(ai.history.map((s) => [s.id, s.archived])).toEqual([
      ['a', true],
      ['old', true],
    ]);

    await ai.reopen('old');
    expect(ai.sessions.map((s) => s.id)).toEqual(['old']);
    expect(ai.activeId).toBe('old');
    expect(ai.threads.old).toEqual([{ kind: 'user', text: 'hello' }]);
    expect(ai.history.map((s) => s.id)).toEqual(['a']);

    await ai.deleteSession('old');
    expect(ai.sessions).toEqual([]);
    expect(await ai.deleteAllClosed()).toBe(1);
    expect(ai.history).toEqual([]);
    expect(api.methods().filter((m) => m === 'ai.session_delete')).toHaveLength(2);
  });

  it('rename sends the trimmed title; a renamed closed chat stays in History', async () => {
    const api = new FakeApi({
      'ai.session_rename': (p: { id: string; title: string }) => titled(p.id, p.title, p.id === 'h' ? { archived: true } : {}),
    });
    const ai = new AiStore(api);
    ai.sessions = [titled('a', 'Claude Code')];
    ai.history = [titled('h', 'Old', { archived: true })];
    await ai.rename('a', '  Trip  ');
    expect(api.calls[0].params).toEqual({ id: 'a', title: 'Trip' });
    expect(ai.sessions[0].title).toBe('Trip');
    await ai.rename('h', 'Older');
    expect(ai.history[0].title).toBe('Older');
    expect(ai.sessions.map((s) => s.id)).toEqual(['a']);
  });

  it('edit cuts the thread at once and puts it back if the core refuses', async () => {
    let fail = false;
    const api = new FakeApi({
      'ai.session_edit': () => {
        if (fail) throw new Error('nope');
        return titled('a', 't', { status: 'running' });
      },
    });
    const ai = new AiStore(api);
    ai.sessions = [titled('a', 't')];
    const thread: ChatItem[] = [
      { kind: 'user', text: 'one' },
      { kind: 'assistant', text: 'A', streaming: false },
      { kind: 'user', text: 'two' },
      { kind: 'assistant', text: 'B', streaming: false },
    ];
    ai.threads.a = thread;
    await ai.edit('a', 1, ' two, shorter ');
    expect(api.calls[0].params).toEqual({ id: 'a', index: 1, text: 'two, shorter' });
    expect(ai.threads.a.map((i) => i.kind)).toEqual(['user', 'assistant']);
    fail = true;
    ai.threads.a = thread;
    await expect(ai.edit('a', 0, 'x')).rejects.toThrow('nope');
    expect(ai.threads.a).toEqual(thread);
  });

  it('models: loaded once per provider; the new-chat model is passed and remembered', async () => {
    const api = new FakeApi({
      'ai.models': () => ({ models: [{ id: 'default', name: 'Default' }, { id: 'opus', name: 'Opus' }], default: 'default' }),
      'ai.session_start': (p: { provider: string }) => ({ ...session('claude-code-1', 'running'), provider: p.provider }),
      'ai.session_set_model': (p: { id: string; model: string }) => ({ ...session(p.id, 'idle'), model: p.model }),
      'ai.session_interrupt': (p: { id: string }) => session(p.id, 'idle'),
    });
    const ai = new AiStore(api);
    await Promise.all([ai.loadModels('claude-code'), ai.loadModels('claude-code')]);
    expect(api.methods().filter((m) => m === 'ai.models')).toHaveLength(1);
    expect(ai.models['claude-code'].models).toHaveLength(2);

    ai.setNewChatModel('claude-code', 'opus');
    await ai.send('hi');
    expect(api.calls.at(-1)).toMatchObject({ method: 'ai.session_start', params: { provider: 'claude-code', prompt: 'hi', model: 'opus' } });
    ai.setNewChatModel('claude-code', '');
    expect(ai.newChatModel).toEqual({});

    await ai.setModel('claude-code-1', 'haiku');
    expect(ai.sessions[0].model).toBe('haiku');
    await ai.interrupt('claude-code-1');
    expect(api.calls.at(-1)).toEqual({ method: 'ai.session_interrupt', params: { id: 'claude-code-1' } });
  });
});
