import { describe, expect, it } from 'vitest';
import type { BrowserApi } from '../api/client';
import type { AiPermission, AiSession, ApiEvents, CallArgs, EventName, Method, Result } from '../api/types';
import { AiStore } from './store.svelte';

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
  title: 'Claude Code',
  title_set: false,
  archived: false,
  updated_ms: created_ms,
  queued: 0,
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
