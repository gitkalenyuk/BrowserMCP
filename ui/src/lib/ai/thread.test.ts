import { beforeAll, describe, expect, it } from 'vitest';
import type { AiPermission } from '../api/types';
import { settings } from '../settings.svelte';
import {
  applyAiEvent,
  argsPreview,
  doneLabel,
  failureReason,
  noticeText,
  groupTools,
  lastUserIndex,
  queuedCount,
  settleQueued,
  summaryText,
  threadFromTranscript,
  threadMarkdown,
  toolLabel,
  truncateAtUser,
  userIndexAt,
  withPending,
  type AiEvent,
  type ChatItem,
} from './thread';

const S = 'anthropic-1';
const msg = (
  role: 'user' | 'assistant' | 'system',
  p: { text?: string; text_delta?: string; queued?: boolean; taken?: boolean; index?: number },
): AiEvent => ({
  type: 'ai.message',
  payload: { session: S, role, ...p },
});
const fold = (events: AiEvent[], start: ChatItem[] = []) => events.reduce((items, e) => applyAiEvent(items, e), start);

// Labels are checked in English, whatever the machine's language.
beforeAll(() => {
  settings.value = { ...settings.value, language: 'en' };
});

describe('applyAiEvent: streaming', () => {
  it('accumulates text_delta chunks into one streaming row', () => {
    const items = fold([msg('user', { text: 'hi' }), msg('assistant', { text_delta: 'Hel' }), msg('assistant', { text_delta: 'lo' })]);
    expect(items).toEqual([
      { kind: 'user', text: 'hi' },
      { kind: 'assistant', text: 'Hello', streaming: true },
    ]);
  });

  it('a whole `text` replaces the streamed text and closes the row', () => {
    const items = fold([msg('assistant', { text_delta: 'Ping' }), msg('assistant', { text_delta: 'ing' }), msg('assistant', { text: 'Pinging.' })]);
    expect(items).toEqual([{ kind: 'assistant', text: 'Pinging.', streaming: false }]);
  });

  it('deltas after a closed row start a new row', () => {
    const items = fold([msg('assistant', { text: 'one' }), msg('assistant', { text_delta: 'two' })]);
    expect(items.map((i) => i.kind === 'assistant' && i.text)).toEqual(['one', 'two']);
  });

  it('two text blocks streamed back to back end up as two rows (Claude Code)', () => {
    const items = fold([
      msg('assistant', { text_delta: 'A' }),
      msg('assistant', { text_delta: 'B' }),
      msg('assistant', { text: 'A' }),
      msg('assistant', { text: 'B' }),
    ]);
    expect(items).toEqual([
      { kind: 'assistant', text: 'A', streaming: false },
      { kind: 'assistant', text: 'B', streaming: false },
    ]);
  });

  it('a text without deltas is a new finished row', () => {
    expect(fold([msg('assistant', { text: 'done' })])).toEqual([{ kind: 'assistant', text: 'done', streaming: false }]);
  });

  it('a tool call closes the streaming row, its result fills the call', () => {
    const items = fold([
      msg('assistant', { text_delta: 'Opening' }),
      { type: 'ai.tool_call', payload: { session: S, name: 'nav_goto', args: { url: 'https://a.test/' } } },
      { type: 'ai.tool_result', payload: { session: S, name: 'nav_goto', ok: true, summary: '{"tab":3}' } },
      msg('assistant', { text_delta: 'Done' }),
    ]);
    expect(items).toEqual([
      { kind: 'assistant', text: 'Opening', streaming: false },
      { kind: 'tool', name: 'nav_goto', args: { url: 'https://a.test/' }, pending: false, ok: true, summary: '{"tab":3}' },
      { kind: 'assistant', text: 'Done', streaming: true },
    ]);
  });

  it('results go to the newest pending call of the same tool', () => {
    const items = fold([
      { type: 'ai.tool_call', payload: { session: S, name: 'page_read', args: {} } },
      { type: 'ai.tool_call', payload: { session: S, name: 'tabs_open', args: {} } },
      { type: 'ai.tool_result', payload: { session: S, name: 'page_read', ok: false, summary: 'TIMEOUT' } },
    ]);
    expect(items[0]).toMatchObject({ name: 'page_read', pending: false, ok: false, summary: 'TIMEOUT' });
    expect(items[1]).toMatchObject({ name: 'tabs_open', pending: true });
  });

  it('done settles streaming rows and pending calls', () => {
    const items = fold([
      msg('assistant', { text_delta: 'half' }),
      { type: 'ai.tool_call', payload: { session: S, name: 'x', args: {} } },
      { type: 'ai.done', payload: { session: S, reason: 'stopped' } },
    ]);
    expect(items).toEqual([
      { kind: 'assistant', text: 'half', streaming: false },
      { kind: 'tool', name: 'x', args: {}, pending: false, ok: null, summary: null },
      { kind: 'done', reason: 'stopped' },
    ]);
  });

  it('system notices are their own rows and do not break a stream', () => {
    const items = fold([msg('assistant', { text_delta: 'a' }), msg('system', { text: 'warning' })]);
    expect(items[1]).toEqual({ kind: 'system', text: 'warning' });
  });

  it('never mutates its input', () => {
    const start: ChatItem[] = [{ kind: 'assistant', text: 'x', streaming: true }];
    const frozen = structuredClone(start);
    applyAiEvent(start, msg('assistant', { text_delta: 'y' }));
    expect(start).toEqual(frozen);
  });
});

describe('threadFromTranscript', () => {
  it('rebuilds the same rows as the live events', () => {
    const items = threadFromTranscript([
      { type: 'user', text: 'go', ts_ms: 1 },
      { type: 'tool_call', name: 'nav_goto', args: { url: 'u' } },
      { type: 'tool_result', name: 'nav_goto', ok: true, summary: 'ok' },
      { type: 'assistant', text: 'there' },
      { type: 'notice', text: 'note' },
      { type: 'done', reason: 'end_turn' },
    ]);
    expect(items.map((i) => i.kind)).toEqual(['user', 'tool', 'assistant', 'system', 'done']);
  });
});

describe('labels', () => {
  it('doneLabel hides normal turns', () => {
    expect(doneLabel('end_turn')).toBeNull();
    expect(doneLabel('error: 401 unauthorized')).toBe('Error: 401 unauthorized');
  });
  it('argsPreview cuts long JSON', () => {
    expect(argsPreview({})).toBe('');
    expect(argsPreview({ a: 'x'.repeat(200) }, 20)).toHaveLength(20);
  });
  it('toolLabel strips MCP prefixes', () => {
    expect(toolLabel('mcp__browsermcp__nav_goto')).toBe('nav_goto');
    expect(toolLabel('shell')).toBe('shell');
  });
});

describe('notes and results the core words itself', () => {
  it('a notice with a known code is shown translated, live and from the transcript', () => {
    const live = fold([
      { type: 'ai.message', payload: { session: S, role: 'system', text: 'English', code: 'resume_lost', params: { agent: 'Codex' } } },
    ]);
    const saved = threadFromTranscript([{ type: 'notice', text: 'English', code: 'resume_lost', params: { agent: 'Codex' } }]);
    expect(saved).toEqual(live);
    const note = live[0] as Extract<ChatItem, { kind: 'system' }>;
    expect(noticeText(note)).toMatch(/^Codex could not continue/);
    settings.value = { ...settings.value, language: 'uk' };
    expect(noticeText(note)).toMatch(/^Codex не вдалося продовжити/);
    settings.value = { ...settings.value, language: 'en' };
    // Unknown codes and plain notices: the text as sent.
    expect(noticeText({ kind: 'system', text: 'stderr line', code: 'other' })).toBe('stderr line');
    expect(noticeText({ kind: 'system', text: 'stderr line' })).toBe('stderr line');
  });

  it('known tool summaries are translated, everything else is kept', () => {
    expect(summaryText('Exited with code 2')).toBe('Exited with code 2');
    expect(summaryText('Files changed')).toBe('Files changed');
    settings.value = { ...settings.value, language: 'uk' };
    expect(summaryText('Exited with code 2')).toBe('Завершилася з кодом 2');
    expect(summaryText('Declined: the command was not approved')).toBe('Відхилено: команду не дозволили');
    expect(summaryText('The user denied this action: not now')).toBe('Ви відхилили цю дію: not now');
    expect(summaryText('{"tab":2}')).toBe('{"tab":2}');
    settings.value = { ...settings.value, language: 'en' };
  });
});

describe('approval requests', () => {
  const request = (status: AiPermission['status'], extra: Partial<AiPermission> = {}): AiPermission => ({
    id: 'claude-code-1.p1',
    session: 'claude-code-1',
    tool: 'Bash',
    kind: 'tool',
    title: null,
    reason: null,
    input: { command: 'ls' },
    status,
    always: false,
    answer: null,
    created_ms: 1,
    ...extra,
  });
  const ask = (r: AiPermission): AiEvent => ({ type: 'ai.permission', payload: r });

  it('a pending request is a card; its answer replaces it in place', () => {
    const items = fold([
      { type: 'ai.tool_call', payload: { session: S, name: 'Bash', args: { command: 'ls' } } },
      ask(request('pending')),
      ask(request('denied', { answer: 'not here' })),
      { type: 'ai.tool_result', payload: { session: S, name: 'Bash', ok: false, summary: 'The user denied this action: not here' } },
    ]);
    expect(items.map((i) => i.kind)).toEqual(['tool', 'permission']);
    const card = items[1] as Extract<ChatItem, { kind: 'permission' }>;
    expect(card.request.status).toBe('denied');
    // The refusal reason is shown on the failed call (worded for the person who refused).
    const tool = items[0] as Extract<ChatItem, { kind: 'tool' }>;
    expect(failureReason(tool)).toBe('You denied this action: not here');
  });

  it('transcript entries and still-pending requests rebuild the cards', () => {
    const items = threadFromTranscript([{ type: 'user', text: 'go' }, { type: 'permission', ...request('allowed', { always: true }) }]);
    expect(items.map((i) => i.kind)).toEqual(['user', 'permission']);
    const more = withPending(items, [request('pending', { id: 'claude-code-1.p2' }), request('pending')]);
    // p1 is already there (answered): only p2 is added, once.
    expect(more.filter((i) => i.kind === 'permission')).toHaveLength(2);
    expect(withPending(more, [request('pending', { id: 'claude-code-1.p2' })])).toEqual(more);
  });

  it('a failed call without text still says it failed', () => {
    const tool: Extract<ChatItem, { kind: 'tool' }> = { kind: 'tool', name: 'x', args: null, pending: false, ok: false, summary: '' };
    expect(failureReason(tool)).toBe('Failed without saying why');
    expect(failureReason({ ...tool, ok: true })).toBeNull();
  });
});

const call = (name: string): AiEvent => ({ type: 'ai.tool_call', payload: { session: S, name, args: { n: name } } });

describe('messages sent while the agent works', () => {
  it('a queued message stays at the bottom while the answer streams on above it', () => {
    const items = fold([
      msg('user', { text: 'find flights' }),
      msg('assistant', { text_delta: 'Look' }),
      msg('user', { text: 'only direct ones', queued: true }),
      msg('assistant', { text_delta: 'ing…' }),
      call('tabs_open'),
      msg('assistant', { text: 'Found 3.' }),
    ]);
    expect(items.map((i) => i.kind)).toEqual(['user', 'assistant', 'tool', 'assistant', 'user']);
    expect(items[1]).toEqual({ kind: 'assistant', text: 'Looking…', streaming: false });
    expect(items[4]).toEqual({ kind: 'user', text: 'only direct ones', queued: true });
    expect(queuedCount(items)).toBe(1);
  });

  it('taken: the message moves to the end of the conversation, no longer queued', () => {
    const base = fold([msg('user', { text: 'a' }), msg('user', { text: 'b', queued: true }), msg('user', { text: 'c', queued: true })]);
    const taken = applyAiEvent(base, msg('user', { text: 'b', taken: true }));
    expect(taken).toEqual([
      { kind: 'user', text: 'a' },
      { kind: 'user', text: 'b' },
      { kind: 'user', text: 'c', queued: true },
    ]);
    // The same text again with queued:false counts as taken too (no duplicate row).
    const again = applyAiEvent(taken, msg('user', { text: 'c', queued: false }));
    expect(again.map((i) => [i.kind, (i as { queued?: boolean }).queued ?? false])).toEqual([
      ['user', false],
      ['user', false],
      ['user', false],
    ]);
    // A follow-up for a message this window never saw changes nothing.
    expect(applyAiEvent(again, msg('user', { text: 'zzz', taken: true }))).toEqual(again);
  });

  it('a message taken mid-answer goes before that answer, which keeps streaming (no split, no repeat)', () => {
    const items = fold([
      msg('user', { text: 'go', index: 0 }),
      msg('assistant', { text_delta: 'Hel' }),
      msg('user', { text: 'also this', queued: true, index: 1 }),
      { type: 'ai.message', payload: { session: S, role: 'user', taken: true, index: 1 } },
      msg('assistant', { text_delta: 'lo' }),
      msg('assistant', { text: 'Hello world' }),
    ]);
    expect(items).toEqual([
      { kind: 'user', text: 'go', index: 0 },
      { kind: 'user', text: 'also this', index: 1 },
      { kind: 'assistant', text: 'Hello world', streaming: false },
    ]);
    // Same when the core's queued count settles it.
    const settled = settleQueued(
      fold([msg('user', { text: 'go' }), msg('assistant', { text_delta: 'Hel' }), msg('user', { text: 'more', queued: true })]),
      0,
    );
    expect(fold([msg('assistant', { text_delta: 'lo' }), msg('assistant', { text: 'Hello.' })], settled)).toEqual([
      { kind: 'user', text: 'go' },
      { kind: 'user', text: 'more' },
      { kind: 'assistant', text: 'Hello.', streaming: false },
    ]);
  });

  it('events seen twice change nothing: a taken number not queued here, a message already shown', () => {
    const items = fold([msg('user', { text: 'a', index: 0 }), msg('user', { text: 'b', queued: true, index: 2 })]);
    // Message 1 was taken already (or sent from another window): b keeps waiting.
    expect(applyAiEvent(items, { type: 'ai.message', payload: { session: S, role: 'user', taken: true, index: 1 } })).toEqual(items);
    expect(applyAiEvent(items, msg('user', { text: 'a', index: 0 }))).toEqual(items);
    expect(applyAiEvent(items, msg('user', { text: 'b', queued: true, index: 2 }))).toEqual(items);
  });

  it('the queued count from the core settles messages whose follow-up was missed', () => {
    const items = fold([msg('user', { text: 'a' }), msg('user', { text: 'b', queued: true }), msg('user', { text: 'c', queued: true })]);
    expect(queuedCount(settleQueued(items, 2))).toBe(2);
    const one = settleQueued(items, 1);
    expect(one[1]).toEqual({ kind: 'user', text: 'b' });
    expect(one[2]).toEqual({ kind: 'user', text: 'c', queued: true });
    expect(queuedCount(settleQueued(items, 0))).toBe(0);
  });

  it('timestamps come with the events and the transcript', () => {
    const items = applyAiEvent([], msg('user', { text: 'hi' }), 1234);
    expect(items[0]).toEqual({ kind: 'user', text: 'hi', ts: 1234 });
    const t2 = threadFromTranscript([{ type: 'assistant', text: 'yo', ts_ms: 99 }]);
    expect(t2[0]).toEqual({ kind: 'assistant', text: 'yo', streaming: false, ts: 99 });
  });
});

describe('editing an earlier message', () => {
  const thread = fold([
    msg('user', { text: 'one' }),
    msg('assistant', { text: 'A1' }),
    msg('user', { text: 'two' }),
    call('page_read'),
    msg('assistant', { text: 'A2' }),
    msg('user', { text: 'three', queued: true }),
  ]);

  it('user numbers skip other rows and queued messages', () => {
    expect(userIndexAt(thread, 0)).toBe(0);
    expect(userIndexAt(thread, 2)).toBe(1);
    expect(userIndexAt(thread, 1)).toBe(-1);
    expect(userIndexAt(thread, 5)).toBe(-1);
    expect(lastUserIndex(thread)).toBe(2);
  });

  // The thread of a chat seen only from its 8th message on (a second window, a reload).
  const partial = fold([
    msg('user', { text: 'eighth', index: 7 }),
    msg('assistant', { text: 'A8' }),
    msg('user', { text: 'ninth', index: 8 }),
    msg('assistant', { text: 'A9' }),
  ]);

  it('a row is the message the core numbered, not its place in what is shown', () => {
    expect(userIndexAt(partial, 0)).toBe(7);
    expect(userIndexAt(partial, 2)).toBe(8);
    // ai.session_rewound {index: 8} cuts at the ninth message, not at the 9th row.
    expect(truncateAtUser(partial, 8).map((i) => i.kind)).toEqual(['user', 'assistant']);
    // A cut before everything shown leaves nothing of it.
    expect(truncateAtUser(partial, 3)).toEqual([]);
    expect(truncateAtUser(partial, 9)).toBe(partial);
  });

  it('a transcript keeps each message number and what still waits', () => {
    const items = threadFromTranscript([
      { type: 'user', text: 'one', index: 0 },
      { type: 'assistant', text: 'A' },
      { type: 'user', text: 'two', index: 1, queued: true },
    ]);
    expect(items).toEqual([
      { kind: 'user', text: 'one', index: 0 },
      { kind: 'assistant', text: 'A', streaming: false },
      { kind: 'user', text: 'two', index: 1, queued: true },
    ]);
    expect(userIndexAt(items, 0)).toBe(0);
  });

  it('truncates to just before that message (later rows and queued ones go)', () => {
    expect(truncateAtUser(thread, 1).map((i) => i.kind)).toEqual(['user', 'assistant']);
    expect(truncateAtUser(thread, 0)).toEqual([]);
    expect(truncateAtUser(thread, 7)).toBe(thread);
    // The resent text then arrives as an ordinary user message.
    const resent = applyAiEvent(truncateAtUser(thread, 1), msg('user', { text: 'two, but shorter' }));
    expect(resent.at(-1)).toEqual({ kind: 'user', text: 'two, but shorter' });
  });
});

describe('tool groups and Markdown export', () => {
  it('folds runs of 4+ tool calls into one group', () => {
    const items = fold([msg('user', { text: 'go' }), call('a'), call('b'), call('c'), call('d'), msg('assistant', { text: 'ok' }), call('e')]);
    const rows = groupTools(items);
    expect(rows.map((r) => r.kind)).toEqual(['item', 'tools', 'item', 'item']);
    const group = rows[1] as Extract<ReturnType<typeof groupTools>[number], { kind: 'tools' }>;
    expect(group.start).toBe(1);
    expect(group.items.map((r) => r.index)).toEqual([1, 2, 3, 4]);
    expect(groupTools(fold([call('a'), call('b'), call('c')])).every((r) => r.kind === 'item')).toBe(true);
  });

  it('exports the chat as Markdown', () => {
    const items = fold([
      msg('user', { text: 'Compare prices' }),
      call('tabs_open'),
      call('page_read'),
      { type: 'ai.tool_result', payload: { session: S, name: 'page_read', ok: false, summary: 'boom' } },
      msg('assistant', { text: '**Done.**' }),
      { type: 'ai.done', payload: { session: S, reason: 'stopped' } },
    ]);
    expect(threadMarkdown(items, 'Prices')).toBe(
      '# Prices\n\n**You:**\n\nCompare prices\n\n- – `tabs_open` {"n":"tabs_open"}\n- ✗ `page_read` {"n":"page_read"}\n\n**Agent:**\n\n**Done.**\n\n_Stopped_\n',
    );
  });
});
