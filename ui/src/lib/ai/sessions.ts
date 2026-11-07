// Pure rules of the AI panel's chat list: which chat a message goes to, which chat is shown
// after one is closed, which chats count as finished, and the list of approval requests
// waiting for an answer. No Svelte, no I/O: unit-tested in sessions.test.ts.

import type { AiPermission, AiSession } from '../api/types';

/**
 * Where a message typed in the panel goes: the shown chat (whatever its status: a stopped
 * or failed chat is continued by the core), or a new chat when none is shown. Only "+ New"
 * (no chat shown) starts a new one.
 */
export function sendTarget(active: AiSession | null): { kind: 'continue'; id: string } | { kind: 'new' } {
  return active ? { kind: 'continue', id: active.id } : { kind: 'new' };
}

/**
 * The chat to show once `closing` is closed: unchanged if another one is shown, else the
 * neighbour in the tab strip (the next one, or the previous one at the end), else none
 * (the new-chat page).
 */
export function afterClose(order: string[], closing: string, active: string | null): string | null {
  if (active !== closing) return active;
  const at = order.indexOf(closing);
  const rest = order.filter((id) => id !== closing);
  if (rest.length === 0 || at < 0) return rest[0] ?? null;
  return rest[Math.min(at, rest.length - 1)];
}

/** Chats that are not working and wait for no approval: what "Close finished chats" closes. */
export function finishedIds(sessions: AiSession[], pending: AiPermission[]): string[] {
  const waiting = new Set(pending.map((p) => p.session));
  return sessions.filter((s) => s.status !== 'running' && !waiting.has(s.id)).map((s) => s.id);
}

/** Fold an `ai.permission` event into the list of pending requests. */
export function applyPermission(pending: AiPermission[], event: AiPermission): AiPermission[] {
  const rest = pending.filter((p) => p.id !== event.id);
  return event.status === 'pending' ? [...rest, event] : rest;
}

/** Ask before closing a chat that is still working (closing stops it). */
export function closeNeedsConfirm(session: AiSession, pending: AiPermission[]): boolean {
  return session.status === 'running' || pending.some((p) => p.session === session.id);
}
