import { describe, expect, it } from 'vitest';
import type { AiPermission, AiSession } from '../api/types';
import { afterClose, applyPermission, autoTitle, chatTitle, closeNeedsConfirm, filterHistory, finishedIds, pickModel, sendTarget } from './sessions';

const session = (id: string, status: AiSession['status']): AiSession => ({
  id,
  provider: 'claude-code',
  model: null,
  color: '#3b82f6',
  status,
  tabs: [],
  created_ms: 1,
  title: 'Claude Code',
  title_set: false,
  archived: false,
  updated_ms: 1,
  queued: 0,
});

const permission = (id: string, sessionId: string, status: AiPermission['status'] = 'pending'): AiPermission => ({
  id,
  session: sessionId,
  tool: 'Bash',
  kind: 'tool',
  title: null,
  reason: null,
  input: {},
  status,
  always: false,
  answer: null,
  created_ms: 1,
});

describe('sendTarget', () => {
  it('continues the shown chat whatever its status; only no chat starts a new one', () => {
    for (const status of ['idle', 'running', 'stopped', 'failed'] as const) {
      expect(sendTarget(session('a', status))).toEqual({ kind: 'continue', id: 'a' });
    }
    expect(sendTarget(null)).toEqual({ kind: 'new' });
  });
});

describe('afterClose', () => {
  const order = ['a', 'b', 'c'];
  it('shows the next chat, or the previous one at the end, or none', () => {
    expect(afterClose(order, 'b', 'b')).toBe('c');
    expect(afterClose(order, 'c', 'c')).toBe('b');
    expect(afterClose(['a'], 'a', 'a')).toBeNull();
  });
  it('keeps another shown chat (or the new-chat page)', () => {
    expect(afterClose(order, 'b', 'a')).toBe('a');
    expect(afterClose(order, 'b', null)).toBeNull();
  });
});

describe('finished chats and confirmations', () => {
  it('finished = not running and no approval waiting', () => {
    const sessions = [session('a', 'idle'), session('b', 'running'), session('c', 'stopped'), session('d', 'failed')];
    expect(finishedIds(sessions, [permission('c.p1', 'c')])).toEqual(['a', 'd']);
  });
  it('asks before closing a working chat or one waiting for an approval', () => {
    expect(closeNeedsConfirm(session('a', 'running'), [])).toBe(true);
    expect(closeNeedsConfirm(session('a', 'idle'), [permission('a.p1', 'a')])).toBe(true);
    expect(closeNeedsConfirm(session('a', 'stopped'), [])).toBe(false);
  });
});

describe('applyPermission', () => {
  it('pending adds or updates; any other status removes', () => {
    let list = applyPermission([], permission('a.p1', 'a'));
    list = applyPermission(list, permission('a.p2', 'a'));
    expect(list.map((p) => p.id)).toEqual(['a.p1', 'a.p2']);
    list = applyPermission(list, permission('a.p1', 'a', 'denied'));
    expect(list.map((p) => p.id)).toEqual(['a.p2']);
    expect(applyPermission(list, permission('a.p2', 'a', 'cancelled'))).toEqual([]);
  });
});

describe('chat titles', () => {
  it('auto title: whitespace collapsed, cut to 48 chars with an ellipsis', () => {
    expect(autoTitle('  Find   the\ncheapest\tflight  ')).toBe('Find the cheapest flight');
    const long = autoTitle('Compare the prices of this laptop in three different online stores please');
    expect([...long]).toHaveLength(48);
    expect(long.endsWith('…')).toBe(true);
    expect(autoTitle('Привіт, знайди мені найдешевший квиток до Львова на наступну п’ятницю')).toHaveLength(48);
  });

  it('the provider name until there is a title; never the id', () => {
    expect(chatTitle({ title: 'Flights to Lviv' }, 'Claude Code')).toBe('Flights to Lviv');
    expect(chatTitle({ title: '' }, 'Claude Code')).toBe('Claude Code');
    expect(chatTitle({ title: '   ' }, 'Claude Code')).toBe('Claude Code');
    expect(chatTitle({}, 'Codex')).toBe('Codex');
  });
});

describe('history', () => {
  const names: Record<string, string> = { 'claude-code': 'Claude Code', codex: 'Codex' };
  const closed = (id: string, title: string, provider: string, updated_ms: number, model: string | null = null): AiSession => ({
    ...session(id, 'stopped'),
    provider,
    title,
    model,
    archived: true,
    updated_ms,
  });
  const list = [
    closed('a', 'Flights to Lviv', 'claude-code', 10, 'sonnet'),
    closed('b', 'Pricing table as CSV', 'codex', 30),
    closed('c', 'Lviv hotels', 'codex', 20, 'gpt-5-codex'),
  ];

  it('newest first, all of them without a query', () => {
    expect(filterHistory(list, '', (id) => names[id]).map((s) => s.id)).toEqual(['b', 'c', 'a']);
  });

  it('every word must match the title, provider or model, any case', () => {
    expect(filterHistory(list, 'lviv', (id) => names[id]).map((s) => s.id)).toEqual(['c', 'a']);
    expect(filterHistory(list, 'LVIV codex', (id) => names[id]).map((s) => s.id)).toEqual(['c']);
    expect(filterHistory(list, 'sonnet', (id) => names[id]).map((s) => s.id)).toEqual(['a']);
    expect(filterHistory(list, 'nothing', (id) => names[id])).toEqual([]);
  });

  it('falls back to created_ms when a chat has no updated_ms', () => {
    const old = { ...session('x', 'stopped'), created_ms: 25, updated_ms: undefined };
    expect(filterHistory([...list, old], '', (id) => names[id]).map((s) => s.id)).toEqual(['b', 'x', 'c', 'a']);
  });
});

describe('pickModel', () => {
  const models = [{ id: 'default' }, { id: 'opus' }, { id: 'sonnet' }];
  it('the remembered model while it is offered, else the provider default ("")', () => {
    expect(pickModel(models, 'opus')).toBe('opus');
    expect(pickModel(models, 'gone')).toBe('');
    expect(pickModel(models, undefined)).toBe('');
    // The list is not loaded yet: keep the remembered one.
    expect(pickModel(null, 'opus')).toBe('opus');
    expect(pickModel([], 'opus')).toBe('opus');
  });
});
