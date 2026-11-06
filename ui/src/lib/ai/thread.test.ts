import { describe, expect, it } from 'vitest';
import { applyAiEvent, argsPreview, doneLabel, threadFromTranscript, toolLabel, type AiEvent, type ChatItem } from './thread';

const S = 'anthropic-1';
const msg = (role: 'user' | 'assistant' | 'system', p: { text?: string; text_delta?: string }): AiEvent => ({
  type: 'ai.message',
  payload: { session: S, role, ...p },
});
const fold = (events: AiEvent[], start: ChatItem[] = []) => events.reduce(applyAiEvent, start);

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
