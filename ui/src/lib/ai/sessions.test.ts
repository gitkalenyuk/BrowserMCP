import { describe, expect, it } from 'vitest';
import type { AiPermission, AiSession } from '../api/types';
import { afterClose, applyPermission, closeNeedsConfirm, finishedIds, sendTarget } from './sessions';

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
