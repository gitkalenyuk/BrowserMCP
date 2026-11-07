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

// ---- names, history, models ------------------------------------------------------------

/** Longest automatic title (the core cuts the same way). */
export const TITLE_MAX = 48;

/** The automatic title of a chat from its first message: whitespace collapsed, cut with "…". */
export function autoTitle(text: string, max = TITLE_MAX): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  const chars = [...flat];
  return chars.length > max ? `${chars.slice(0, max - 1).join('').trimEnd()}…` : flat;
}

/**
 * What a chat is called in the panel: its title, or the provider's name before it has one
 * (and with older cores, which send none). Never the internal id ("claude-code-24").
 */
export function chatTitle(session: Pick<AiSession, 'title'>, providerName: string): string {
  return session.title?.trim() || providerName;
}

/** When a chat was last active, for sorting the history. */
export const lastActive = (s: Pick<AiSession, 'updated_ms' | 'created_ms'>): number => s.updated_ms ?? s.created_ms;

/**
 * Closed chats matching a search (every word must appear in the title, provider name or
 * model, case-insensitive), most recently active first.
 */
export function filterHistory(sessions: AiSession[], query: string, providerName: (id: string) => string): AiSession[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return sessions
    .filter((s) => {
      if (!words.length) return true;
      const name = providerName(s.provider);
      const hay = [chatTitle(s, name), name, s.model ?? ''].join(' ').toLowerCase();
      return words.every((w) => hay.includes(w));
    })
    .sort((a, b) => lastActive(b) - lastActive(a));
}

/**
 * The model preselected for a new chat: the one remembered for this provider while the
 * provider still offers it (or its list is not known yet); "" = the provider's default.
 */
export function pickModel(models: { id: string }[] | null | undefined, remembered: string | undefined): string {
  if (!remembered) return '';
  if (!models?.length) return remembered;
  return models.some((m) => m.id === remembered) ? remembered : '';
}
