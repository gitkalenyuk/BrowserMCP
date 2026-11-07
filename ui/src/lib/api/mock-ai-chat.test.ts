import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPanelMocks } from './mock-panels';
import type { AiTranscriptEntry, ApiEvents, EventName } from './types';

type Seen = { event: EventName; payload: unknown };

function setup() {
  const seen: Seen[] = [];
  const emit = <E extends EventName>(event: E, payload: ApiEvents[E]) => void seen.push({ event, payload });
  const h = createPanelMocks(emit, () => null);
  const of = <E extends EventName>(event: E) => seen.filter((s) => s.event === event).map((s) => s.payload as ApiEvents[E]);
  return { h, seen, of };
}

/** The mock seeds two closed chats for `pnpm dev`; these tests look at their own chats only. */
const SEEDED = new Set(['claude-code-90', 'codex-91']);
const own = <T extends { id: string }>(list: T[]) => list.filter((s) => !SEEDED.has(s.id));

const users = (entries: AiTranscriptEntry[]) => entries.flatMap((e) => (e.type === 'user' ? [e.text] : []));

describe('AI chat mocks (follow crates/ai)', () => {
  beforeEach(() => void vi.useFakeTimers());
  afterEach(() => void vi.useRealTimers());

  it('titles a chat from its first message, renames it and never shows the id', async () => {
    const { h, of } = setup();
    const s = await h['ai.session_start']({ provider: 'claude-code', prompt: '  Compare   laptop prices on three shops and pick the best one please ' });
    expect(s.title).toBe('Compare laptop prices on three shops and pick t…');
    expect([...(s.title ?? '')].length).toBe(48);
    expect(s).toMatchObject({ title_set: false, archived: false, queued: 0 });
    const renamed = await h['ai.session_rename']({ id: s.id, title: ' Laptops ' });
    expect(renamed).toMatchObject({ title: 'Laptops', title_set: true });
    expect(of('ai.session').at(-1)).toMatchObject({ session: s.id, title: 'Laptops' });
    const back = await h['ai.session_rename']({ id: s.id, title: '' });
    expect(back).toMatchObject({ title: s.title, title_set: false });
  });

  it('queues messages sent while busy and takes them into the running turn', async () => {
    const { h, of } = setup();
    const s = await h['ai.session_start']({ provider: 'ollama', prompt: 'find cheap laptops' });
    await vi.advanceTimersByTimeAsync(300);
    const sent = await h['ai.session_send']({ id: s.id, text: 'only 32 GB' });
    expect(sent).toMatchObject({ status: 'running', queued: 1 });
    expect(of('ai.message').at(-1)).toMatchObject({ role: 'user', text: 'only 32 GB', index: 1, queued: true });
    const { entries } = await h['ai.session_transcript']({ id: s.id });
    expect(entries.at(-1)).toMatchObject({ type: 'user', index: 1, queued: true });
    // The mock takes it when the turn ends (like Codex without turn/steer; Claude Code folds it
    // into the running turn): either way ai.message_taken names it by its number.
    await vi.runAllTimersAsync();
    expect(of('ai.message_taken')).toEqual([{ session: s.id, index: 1 }]);
    const [chat] = own((await h['ai.sessions_list']()).sessions);
    expect(chat).toMatchObject({ status: 'idle', queued: 0 });
    expect(of('ai.done')).toHaveLength(2);
  });

  it('interrupts a turn but keeps the chat; changes the model', async () => {
    const { h, of } = setup();
    const s = await h['ai.session_start']({ provider: 'claude-code', prompt: 'look around' });
    const stopped = await h['ai.session_interrupt']({ id: s.id });
    expect(stopped.status).toBe('idle');
    expect(of('ai.done').at(-1)).toEqual({ session: s.id, reason: 'stopped' });
    expect((await h['ai.session_set_model']({ id: s.id, model: 'haiku' })).model).toBe('haiku');
    expect((await h['ai.session_set_model']({ id: s.id, model: 'default' })).model).toBeNull();
    const { models, default: def } = await h['ai.models']({ provider: 'claude-code' });
    expect(models.map((m) => m.id)).toEqual(expect.arrayContaining(['default', 'opus', 'sonnet', 'haiku', 'fable', 'claude-opus-5-5']));
    expect(def).toBe('default');
    const api = await h['ai.models']({ provider: 'anthropic' });
    expect(api.models.map((m) => m.id)).toContain('claude-sonnet-5-5');
  });

  it('edits a message: the transcript is cut there and the new text follows', async () => {
    const { h, of } = setup();
    const s = await h['ai.session_start']({ provider: 'ollama', prompt: 'one' });
    await vi.runAllTimersAsync();
    await h['ai.session_send']({ id: s.id, text: 'two' });
    await vi.runAllTimersAsync();
    await h['ai.session_send']({ id: s.id, text: 'three' });
    await vi.runAllTimersAsync();
    expect(() => h['ai.session_edit']({ id: s.id, index: 5, text: 'x' })).toThrow(/no message 5/);
    await h['ai.session_edit']({ id: s.id, index: 1, text: 'TWO' });
    expect(of('ai.session_rewound')).toEqual([{ session: s.id, index: 1 }]);
    await vi.runAllTimersAsync();
    const { entries } = await h['ai.session_transcript']({ id: s.id });
    expect(users(entries)).toEqual(['one', 'TWO']);
    expect(entries.filter((e) => e.type === 'user').map((e) => (e.type === 'user' ? e.index : -1))).toEqual([0, 1]);
    await h['ai.session_edit']({ id: s.id, index: 0, text: 'fresh' });
    expect((await h['ai.sessions_list']()).sessions[0].title).toBe('fresh');
  });

  it('archives on close, reopens, and deletes for good', async () => {
    const { h, of } = setup();
    const a = await h['ai.session_start']({ provider: 'claude-code', prompt: 'first chat' });
    await vi.advanceTimersByTimeAsync(10);
    const b = await h['ai.session_start']({ provider: 'claude-code', prompt: 'second chat' });
    await vi.runAllTimersAsync();
    expect(await h['ai.session_close']({ id: a.id })).toEqual({ closed: a.id });
    expect(of('ai.session_closed')).toEqual([{ session: a.id }]);
    expect((await h['ai.sessions_list']()).sessions.map((s) => s.id)).toEqual([b.id]);
    const archived = own((await h['ai.sessions_list']({ archived: true })).sessions);
    expect(archived.map((s) => s.id)).toEqual([a.id]);
    expect(archived[0]).toMatchObject({ archived: true, status: 'stopped' });
    // The history stays readable; writing needs a reopen.
    expect(users((await h['ai.session_transcript']({ id: a.id })).entries)).toEqual(['first chat']);
    expect(() => h['ai.session_send']({ id: a.id, text: 'x' })).toThrow(/closed/);
    expect((await h['ai.session_reopen']({ id: a.id })).archived).toBe(false);
    expect((await h['ai.sessions_list']()).sessions.map((s) => s.id)).toEqual([a.id, b.id]);

    expect(await h['ai.session_delete']({ id: b.id })).toEqual({ deleted: true });
    expect(of('ai.session_deleted')).toEqual([{ session: b.id }]);
    expect(() => h['ai.session_transcript']({ id: b.id })).toThrow(/no AI session/);
    await h['ai.session_close']({ id: a.id, delete_transcript: true });
    expect(own((await h['ai.sessions_list']({ archived: true })).sessions)).toEqual([]);
  });
});
