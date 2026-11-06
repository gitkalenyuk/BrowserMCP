// Pure chat-thread reducer for one AI session: folds ai.* events (or a saved transcript)
// into the rows the AI panel renders. No Svelte, no I/O: unit-tested in thread.test.ts.
//
// Streaming contract of the core (crates/ai/src/session.rs): an assistant message arrives as
// `text_delta` chunks and is then repeated whole as `text` (one `text` per text block). So a
// `text` replaces the text streamed so far and closes it; deltas after that start a new row.

import type { AiMessageEvent, AiTranscriptEntry, ApiEvents } from '../api/types';

export type ChatItem =
  | { kind: 'user'; text: string }
  | { kind: 'assistant'; text: string; streaming: boolean }
  | { kind: 'system'; text: string }
  | { kind: 'tool'; name: string; args: unknown; pending: boolean; ok: boolean | null; summary: string | null }
  | { kind: 'done'; reason: string };

export type AiEvent =
  | { type: 'ai.message'; payload: AiMessageEvent }
  | { type: 'ai.tool_call'; payload: ApiEvents['ai.tool_call'] }
  | { type: 'ai.tool_result'; payload: ApiEvents['ai.tool_result'] }
  | { type: 'ai.done'; payload: ApiEvents['ai.done'] };

function closeStreaming(items: ChatItem[]): ChatItem[] {
  if (!items.some((i) => i.kind === 'assistant' && i.streaming)) return items;
  return items.map((i) => (i.kind === 'assistant' && i.streaming ? { ...i, streaming: false } : i));
}

/** Returns a new array (the input is never mutated). */
export function applyAiEvent(items: ChatItem[], event: AiEvent): ChatItem[] {
  switch (event.type) {
    case 'ai.message': {
      const { role, text, text_delta } = event.payload;
      if (role === 'user') return [...closeStreaming(items), { kind: 'user', text: text ?? '' }];
      if (role === 'system') return [...items, { kind: 'system', text: text ?? text_delta ?? '' }];
      const last = items[items.length - 1];
      const open = last?.kind === 'assistant' && last.streaming ? last : null;
      if (text_delta !== undefined) {
        if (open) return [...items.slice(0, -1), { ...open, text: open.text + text_delta }];
        return [...items, { kind: 'assistant', text: text_delta, streaming: true }];
      }
      const whole = text ?? '';
      if (open) return [...items.slice(0, -1), { kind: 'assistant', text: whole, streaming: false }];
      return [...items, { kind: 'assistant', text: whole, streaming: false }];
    }
    case 'ai.tool_call': {
      const { name, args } = event.payload;
      return [...closeStreaming(items), { kind: 'tool', name, args, pending: true, ok: null, summary: null }];
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
        items = applyAiEvent(items, { type: 'ai.message', payload: { session: '', role: 'user', text: e.text } });
        break;
      case 'assistant':
        items = applyAiEvent(items, { type: 'ai.message', payload: { session: '', role: 'assistant', text: e.text } });
        break;
      case 'notice':
        items = applyAiEvent(items, { type: 'ai.message', payload: { session: '', role: 'system', text: e.text } });
        break;
      case 'tool_call':
        items = applyAiEvent(items, { type: 'ai.tool_call', payload: { session: '', name: e.name, args: e.args } });
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
    }
  }
  return items;
}

/** `done` rows worth showing: anything but a normal end of turn. */
export function doneLabel(reason: string): string | null {
  if (reason === 'end_turn') return null;
  if (reason === 'stopped') return 'Stopped';
  if (reason === 'max_steps') return 'Stopped after the step limit';
  if (reason.startsWith('error')) return reason.replace(/^error:\s*/, 'Error: ');
  return reason;
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
