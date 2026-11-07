// Pure chat-thread reducer for one AI session: folds ai.* events (or a saved transcript)
// into the rows the AI panel renders. No I/O (labels go through t()): unit-tested in
// thread.test.ts.
//
// Streaming contract of the core (crates/ai/src/session.rs): an assistant message arrives as
// `text_delta` chunks and is then repeated whole as `text` (one `text` per text block). So a
// `text` replaces the text streamed so far and closes it; deltas after that start a new row.
//
// Approval requests (`ai.permission`) are rows too: a `pending` one is the approval card, the
// same id answered later turns it into a one-line note in place.

import type { AiMessageEvent, AiPermission, AiTranscriptEntry, ApiEvents } from '../api/types';
import { t } from '../i18n/index.svelte';

export type ChatItem =
  /** `queued`: sent while the agent was busy and not taken by it yet (kept at the bottom). */
  | { kind: 'user'; text: string; queued?: boolean; ts?: number; index?: number }
  | { kind: 'assistant'; text: string; streaming: boolean; ts?: number }
  /** `code`: a note the UI knows, shown in the user's language (see `noticeText`). */
  | { kind: 'system'; text: string; code?: string; params?: Record<string, string> }
  | { kind: 'tool'; name: string; args: unknown; pending: boolean; ok: boolean | null; summary: string | null; ts?: number }
  | { kind: 'permission'; request: AiPermission }
  | { kind: 'done'; reason: string };

export type AiEvent =
  | { type: 'ai.message'; payload: AiMessageEvent }
  | { type: 'ai.tool_call'; payload: ApiEvents['ai.tool_call'] }
  | { type: 'ai.tool_result'; payload: ApiEvents['ai.tool_result'] }
  | { type: 'ai.permission'; payload: AiPermission }
  | { type: 'ai.done'; payload: ApiEvents['ai.done'] };

function closeStreaming(items: ChatItem[]): ChatItem[] {
  if (!items.some((i) => i.kind === 'assistant' && i.streaming)) return items;
  return items.map((i) => (i.kind === 'assistant' && i.streaming ? { ...i, streaming: false } : i));
}

const isQueued = (i: ChatItem | undefined): boolean => i?.kind === 'user' && i.queued === true;

/** Where the trailing run of queued user messages starts (`items.length` when there is none). */
function queuedStart(items: ChatItem[]): number {
  let at = items.length;
  while (at > 0 && isQueued(items[at - 1])) at--;
  return at;
}

const stamp = <T extends object>(row: T, ts: number | undefined): T => (ts === undefined ? row : { ...row, ts });

function unqueue(row: ChatItem): ChatItem {
  if (row.kind !== 'user') return row;
  const { queued: _queued, ...plain } = row;
  return plain;
}

/**
 * Fold one event. Returns a new array (the input is never mutated). `ts` (ms) is when it
 * happened, shown on hover.
 *
 * Messages sent while the agent works stay at the bottom, marked queued, while the agent's
 * answer goes on above them; when the agent takes one (`taken`, or the same text again with
 * `queued: false`) it moves to the end of the conversation as an ordinary message.
 */
export function applyAiEvent(items: ChatItem[], event: AiEvent, ts?: number): ChatItem[] {
  const cut = queuedStart(items);
  const head = items.slice(0, cut);
  const tail = items.slice(cut);
  if (event.type === 'ai.message' && event.payload.role === 'user') {
    const { text = '', queued, taken, index } = event.payload;
    if (taken || queued === false) {
      // The core names the message by its number (ai.message_taken); older forms by its text.
      const byIndex = index === undefined ? -1 : tail.findIndex((i) => i.kind === 'user' && i.index === index);
      const same = byIndex >= 0 ? byIndex : tail.findIndex((i) => i.kind === 'user' && i.text === text && (!taken || index === undefined));
      const pick = same >= 0 ? same : taken ? 0 : -1;
      if (pick >= 0 && pick < tail.length) {
        return [...closeStreaming(head), unqueue(tail[pick]), ...tail.filter((_, k) => k !== pick)];
      }
      if (taken) return items; // nothing of it is queued here (sent from another window)
    }
    const row = index === undefined ? { kind: 'user' as const, text } : { kind: 'user' as const, text, index };
    if (queued) return [...items, stamp({ ...row, queued: true }, ts)];
    return [...closeStreaming(head), stamp(row, ts), ...tail];
  }
  const next = applyToConversation(head, event, ts);
  return tail.length ? [...next, ...tail] : next;
}

function applyToConversation(items: ChatItem[], event: AiEvent, ts?: number): ChatItem[] {
  switch (event.type) {
    case 'ai.message': {
      const { role, text, text_delta, code, params } = event.payload;
      if (role === 'system') {
        const note: ChatItem = { kind: 'system', text: text ?? text_delta ?? '' };
        if (code) Object.assign(note, { code, params: params ?? {} });
        return [...items, note];
      }
      const last = items[items.length - 1];
      const open = last?.kind === 'assistant' && last.streaming ? last : null;
      if (text_delta !== undefined) {
        if (open) return [...items.slice(0, -1), { ...open, text: open.text + text_delta }];
        return [...items, stamp({ kind: 'assistant' as const, text: text_delta, streaming: true }, ts)];
      }
      const whole = text ?? '';
      if (open) return [...items.slice(0, -1), stamp({ kind: 'assistant' as const, text: whole, streaming: false }, open.ts ?? ts)];
      return [...items, stamp({ kind: 'assistant' as const, text: whole, streaming: false }, ts)];
    }
    case 'ai.tool_call': {
      const { name, args } = event.payload;
      return [...closeStreaming(items), stamp({ kind: 'tool' as const, name, args, pending: true, ok: null, summary: null }, ts)];
    }
    case 'ai.tool_result': {
      const { name, ok, summary } = event.payload;
      // The newest pending call of that tool (calls of one step may finish out of order by name).
      for (let i = items.length - 1; i >= 0; i--) {
        const it = items[i];
        if (it.kind === 'tool' && it.pending && it.name === name) {
          const next = items.slice();
          next[i] = { ...it, pending: false, ok, summary };
          return next;
        }
      }
      return [...items, { kind: 'tool', name, args: null, pending: false, ok, summary }];
    }
    case 'ai.permission': {
      // An answer replaces its pending card in place; anything else is a new row.
      const request = event.payload;
      const at = items.findIndex((i) => i.kind === 'permission' && i.request.id === request.id);
      if (at < 0) return [...closeStreaming(items), { kind: 'permission', request }];
      const next = items.slice();
      next[at] = { kind: 'permission', request };
      return next;
    }
    case 'ai.done': {
      // Calls that never reported (stopped mid-way) stop spinning.
      const settled = closeStreaming(items).map((i) =>
        i.kind === 'tool' && i.pending ? { ...i, pending: false } : i,
      );
      return [...settled, { kind: 'done', reason: event.payload.reason }];
    }
  }
}

/** Rebuild a thread from `ai.session_transcript` entries. */
export function threadFromTranscript(entries: AiTranscriptEntry[]): ChatItem[] {
  let items: ChatItem[] = [];
  for (const e of entries) {
    switch (e.type) {
      case 'user':
        items = applyAiEvent(items, { type: 'ai.message', payload: { session: '', role: 'user', text: e.text } }, e.ts_ms);
        break;
      case 'assistant':
        items = applyAiEvent(items, { type: 'ai.message', payload: { session: '', role: 'assistant', text: e.text } }, e.ts_ms);
        break;
      case 'notice':
        items = applyAiEvent(items, {
          type: 'ai.message',
          payload: { session: '', role: 'system', text: e.text, code: e.code, params: e.params },
        });
        break;
      case 'tool_call':
        items = applyAiEvent(items, { type: 'ai.tool_call', payload: { session: '', name: e.name, args: e.args } }, e.ts_ms);
        break;
      case 'tool_result':
        items = applyAiEvent(items, {
          type: 'ai.tool_result',
          payload: { session: '', name: e.name, ok: e.ok, summary: e.summary },
        });
        break;
      case 'done':
        items = applyAiEvent(items, { type: 'ai.done', payload: { session: '', reason: e.reason } });
        break;
      case 'permission': {
        const { type: _type, ts_ms: _ts, ...request } = e;
        items = applyAiEvent(items, { type: 'ai.permission', payload: request });
        break;
      }
    }
  }
  return items;
}

/** Add the still-pending approval requests (`ai.permissions_list`) that the thread lacks. */
export function withPending(items: ChatItem[], pending: AiPermission[]): ChatItem[] {
  return pending.reduce(
    (acc, request) =>
      acc.some((i) => i.kind === 'permission' && i.request.id === request.id)
        ? acc
        : applyAiEvent(acc, { type: 'ai.permission', payload: request }),
    items,
  );
}

/** `done` rows worth showing: anything but a normal end of turn. */
export function doneLabel(reason: string): string | null {
  if (reason === 'end_turn') return null;
  if (reason === 'stopped') return t('aiChat.done.stopped');
  if (reason === 'max_steps') return t('aiChat.done.maxSteps');
  if (reason === 'interrupted') return t('aiChat.done.interrupted');
  if (reason.startsWith('error')) return t('aiChat.done.error', { message: reason.replace(/^error:\s*/, '') });
  return reason;
}

/** Notes the core sends with a code (crates/ai/src/session.rs `AgentEvent::KnownNotice`). */
const NOTICE_CODES = new Set(['resume_lost', 'mcp_not_connected']);

/** A system note in the user's language when the core sent a known code, else as sent. */
export function noticeText(item: Extract<ChatItem, { kind: 'system' }>): string {
  if (item.code && NOTICE_CODES.has(item.code)) return t(`aiChat.notice.${item.code}`, item.params ?? {});
  return item.text;
}

/**
 * Tool results the core words itself (crates/ai/src/cli/codex.rs) in the user's language;
 * anything else (output, the agent's own errors) as it came.
 */
export function summaryText(summary: string): string {
  const text = summary.trim();
  const exit = /^Exited with code (-?\d+)$/.exec(text);
  if (exit) return t('aiChat.summary.exitCode', { code: exit[1] });
  switch (text) {
    case 'Declined: the command was not approved':
      return t('aiChat.summary.commandDeclined');
    case 'Declined: the change was not approved':
      return t('aiChat.summary.changeDeclined');
    case 'Files changed':
      return t('aiChat.summary.filesChanged');
    case 'The command failed':
      return t('aiChat.summary.commandFailed');
    case 'The user denied this action.':
      return t('aiChat.summary.userDenied');
    case 'The request was cancelled before the user answered.':
      return t('aiChat.summary.requestCancelled');
  }
  const denied = /^The user denied this action: ([\s\S]+)$/.exec(text);
  if (denied) return t('aiChat.summary.userDeniedWhy', { reason: denied[1] });
  return summary;
}

/**
 * Why a tool call failed, for the line under its row: the error text, or a note when there is
 * none. `null` for calls that did not fail.
 */
export function failureReason(item: Extract<ChatItem, { kind: 'tool' }>): string | null {
  if (item.ok !== false) return null;
  const text = item.summary?.trim();
  return text ? summaryText(text) : t('aiChat.tool.noReason');
}

/** Short one-line preview of tool arguments: `{url: "https://…", tab: 3}`. */
export function argsPreview(args: unknown, max = 80): string {
  if (args === null || args === undefined) return '';
  let s: string;
  try {
    s = typeof args === 'string' ? args : JSON.stringify(args);
  } catch {
    s = String(args);
  }
  if (s === '{}') return '';
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

/** Strip MCP prefixes: `mcp__browsermcp__nav_goto` -> `nav_goto`. */
export function toolLabel(name: string): string {
  return name.split('__').pop() || name;
}

// ---- queue, edit, grouping, copy -------------------------------------------------------

/**
 * Reconcile the queued marks with the chat's `queued` count from the core: only the newest
 * `count` queued messages still wait; older ones were taken (their follow-up event may have
 * been missed, e.g. by a window opened later).
 */
export function settleQueued(items: ChatItem[], count: number): ChatItem[] {
  const cut = queuedStart(items);
  const tail = items.slice(cut);
  const taken = tail.length - Math.max(0, count);
  if (taken <= 0) return items;
  return [...closeStreaming(items.slice(0, cut)), ...tail.slice(0, taken).map(unqueue), ...tail.slice(taken)];
}

/** Messages waiting for the agent to take them. */
export const queuedCount = (items: ChatItem[]): number => items.length - queuedStart(items);

/**
 * Which user message a row is: its 0-based ordinal among the conversation's user messages
 * (what `ai.session_edit` takes). -1 for other rows and for queued messages.
 */
export function userOrdinal(items: ChatItem[], index: number): number {
  if (items[index]?.kind !== 'user' || isQueued(items[index])) return -1;
  let n = 0;
  for (let i = 0; i < index; i++) if (items[i].kind === 'user' && !isQueued(items[i])) n++;
  return n;
}

/** Row index of the last user message of the conversation (not a queued one), or -1. */
export function lastUserIndex(items: ChatItem[]): number {
  for (let i = items.length - 1; i >= 0; i--) if (items[i].kind === 'user' && !isQueued(items[i])) return i;
  return -1;
}

/**
 * The thread as `ai.session_edit` leaves it: everything before user message number
 * `ordinal` (queued messages go too); the new text then arrives as a new user message.
 * Unchanged when there is no such message.
 */
export function truncateAtUser(items: ChatItem[], ordinal: number): ChatItem[] {
  let n = 0;
  for (let i = 0; i < items.length; i++) {
    if (items[i].kind !== 'user' || isQueued(items[i])) continue;
    if (n === ordinal) return items.slice(0, i);
    n++;
  }
  return items;
}

type ToolItem = Extract<ChatItem, { kind: 'tool' }>;

/** A row of the rendered thread: one item, or a run of tool calls folded into one group. */
export type ThreadRow =
  | { kind: 'item'; item: ChatItem; index: number }
  | { kind: 'tools'; start: number; items: { item: ToolItem; index: number }[] };

/** Runs of at least `min` consecutive tool calls become one collapsible group. */
export function groupTools(items: ChatItem[], min = 4): ThreadRow[] {
  const rows: ThreadRow[] = [];
  let i = 0;
  while (i < items.length) {
    const it = items[i];
    if (it.kind !== 'tool') {
      rows.push({ kind: 'item', item: it, index: i++ });
      continue;
    }
    const run: { item: ToolItem; index: number }[] = [];
    while (i < items.length && items[i].kind === 'tool') run.push({ item: items[i] as ToolItem, index: i++ });
    if (run.length >= min) rows.push({ kind: 'tools', start: run[0].index, items: run });
    else for (const r of run) rows.push({ kind: 'item', item: r.item, index: r.index });
  }
  return rows;
}

/** Pretty JSON of tool arguments (a string as it is). */
export function pretty(v: unknown): string {
  if (typeof v === 'string') return v;
  try {
    return JSON.stringify(v, null, 2) ?? String(v);
  } catch {
    return String(v);
  }
}

/** The whole chat as Markdown, for "Copy chat as Markdown". */
export function threadMarkdown(items: ChatItem[], title: string): string {
  const out: string[] = [`# ${title}`];
  let prevTool = false;
  for (const it of items) {
    const tool = prevTool;
    prevTool = it.kind === 'tool';
    switch (it.kind) {
      case 'user':
        out.push(`**${t('aiChat.md.you')}${it.queued ? ` (${t('aiChat.queued')})` : ''}:**\n\n${it.text}`);
        break;
      case 'assistant':
        out.push(`**${t('aiChat.md.agent')}:**\n\n${it.text}`);
        break;
      case 'system':
        out.push(`> ${noticeText(it).replace(/\n/g, '\n> ')}`);
        break;
      case 'tool': {
        const mark = it.pending ? '…' : it.ok === false ? '✗' : it.ok ? '✓' : '–';
        const args = argsPreview(it.args, 200);
        const line = `- ${mark} \`${toolLabel(it.name)}\`${args ? ` ${args}` : ''}`;
        // Consecutive tool calls read as one list.
        if (tool) out[out.length - 1] += `\n${line}`;
        else out.push(line);
        break;
      }
      case 'permission':
        out.push(`> ${t('aiChat.md.approval', { tool: it.request.tool, status: t(`aiChat.md.status.${it.request.status}`) })}`);
        break;
      case 'done': {
        const label = doneLabel(it.reason);
        if (label) out.push(`_${label}_`);
        break;
      }
    }
  }
  return `${out.join('\n\n')}\n`;
}
