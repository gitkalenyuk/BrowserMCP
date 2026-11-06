// Fakes for the step debugger (script.debug_start / script.debug_cmd + script.debug_* events),
// used by MockApi so `pnpm dev` demos it without a backend. The script library and Run are
// faked in mock-record-runs.ts; saved scripts are read through `getCode`. The "execution" is a
// toy: console.log lines with a string literal print it, `throw new Error("…")` fails,
// `browser.output(…)` reports a result, and every non-empty line is a step.

import { RpcError } from './client';
import type { ApiEvents, EventName, Params, Result } from './types';
import type { DebugPaused, DebugVariable, ScriptFailure } from './types-scripts';

type ScriptMethod = 'script.debug_start' | 'script.debug_cmd';
export type ScriptHandlers = { [M in ScriptMethod]: (params: Params<M>) => Result<M> | Promise<Result<M>> };
type Emit = <E extends EventName>(event: E, payload: ApiEvents[E]) => void;

/** Lines the toy debugger can stop on (1-based). */
function executableLines(code: string): number[] {
  const out: number[] = [];
  code.split('\n').forEach((raw, i) => {
    const t = raw.trim();
    if (!t || t.startsWith('//') || /^[})\]]+;?$/.test(t) || /^(interface|type|import type)\b/.test(t)) return;
    out.push(i + 1);
  });
  return out;
}

/** `console.log("text")` / `console.log(\`text\`)` -> text. */
function logText(line: string): string | null {
  const m = line.match(/console\.(?:log|info|warn|error)\((["'`])(.*)\1\)/);
  return m ? m[2] : null;
}

function throwText(line: string): string | null {
  const m = line.match(/throw new Error\((["'`])(.*)\1\)/);
  return m ? m[2] : null;
}

/** Fake variables: every `const|let|var x = …` above `line`. */
function variables(lines: string[], line: number): DebugVariable[] {
  const vars = new Map<string, DebugVariable>();
  for (const raw of lines.slice(0, line - 1)) {
    const m = raw.match(/\b(?:const|let|var)\s+(?:\{\s*([^}]+)\}|([A-Za-z_$][\w$]*))(?:\s*:[^=]+)?\s*=\s*(.+?);?\s*$/);
    if (!m) continue;
    const names = m[1] ? m[1].split(',').map((n) => n.trim().split(/[:\s]/)[0]) : [m[2]];
    for (const name of names) {
      const value = m[1] ? 'Object' : m[3].length > 60 ? m[3].slice(0, 59) + '…' : m[3];
      const type = /^\d/.test(value) ? 'number' : /^["'`]/.test(value) ? 'string' : 'object';
      vars.set(name, { name, type, value });
    }
  }
  return [...vars.values()];
}

interface Session {
  id: string;
  name: string;
  lines: string[];
  exec: number[];
  /** Index into exec of the paused line, -1 before the start. */
  pos: number;
  bps: number[];
  result: unknown;
  started: number;
  paused: boolean;
}

export function createScriptMocks(
  emit: Emit,
  opts: { delay?: number; getCode?: (name: string) => string | Promise<string> } = {},
): ScriptHandlers {
  const delay = opts.delay ?? 140;
  const sessions = new Map<string, Session>();
  let nextSession = 1;

  const codeOf = async (p: { name?: string | null; code?: string | null }): Promise<{ name: string; code: string }> => {
    if (p.name != null && p.code == null) {
      if (!opts.getCode) throw new RpcError('NOT_FOUND', `script '${p.name}' not found`);
      return { name: p.name, code: await opts.getCode(p.name) };
    }
    if (p.code != null && p.name == null) return { name: '.adhoc', code: p.code };
    throw new RpcError('INVALID_PARAMS', 'give exactly one of `name` or `code`');
  };
  const later = (fn: () => void) => setTimeout(fn, delay);

  /** Resolve breakpoint lines to lines with code (like V8 moving them). */
  const resolve = (s: Session, lines: number[]) =>
    [...new Set(lines.map((l) => s.exec.find((e) => e >= l)).filter((l): l is number => l !== undefined))].sort((a, b) => a - b);

  /** "Execute" line `exec[i]`; returns the failure when it throws. */
  const execute = (s: Session, i: number): ScriptFailure | null => {
    const line = s.exec[i];
    const text = s.lines[line - 1];
    const err = throwText(text);
    if (err && !/if\s*\(/.test(text)) return { code: 'SCRIPT_ERROR', message: `Uncaught Error: ${err}`, line };
    const log = logText(text);
    if (log !== null) emit('script.debug_output', { id: s.id, stream: 'stdout', text: log });
    if (text.includes('browser.output(')) {
      s.result = { mock: true };
      emit('script.debug_output', { id: s.id, stream: 'result', text: '{"mock":true}' });
    }
    return null;
  };

  /** Run the paused line, then on until a breakpoint (or the very next line when `step`). */
  const advance = (s: Session, step: boolean) => {
    if (s.paused) {
      s.paused = false;
      emit('script.debug_resumed', { id: s.id });
    }
    later(() => {
      if (!sessions.has(s.id)) return;
      if (s.pos >= 0) {
        const failure = execute(s, s.pos);
        if (failure) return end(s, failure);
      }
      for (let i = s.pos + 1; i < s.exec.length; i++) {
        const line = s.exec[i];
        s.pos = i;
        if (step || s.bps.includes(line)) return pause(s, line, step ? 'step' : 'breakpoint');
        const failure = execute(s, i);
        if (failure) return end(s, failure);
      }
      end(s, null);
    });
  };

  const pause = (s: Session, line: number, reason: string) => {
    s.paused = true;
    const indented = /^\s+/.test(s.lines[line - 1]);
    const url = `file:///data/scripts/${s.name}.ts`;
    const paused: DebugPaused = {
      id: s.id,
      reason,
      line,
      column: (s.lines[line - 1].match(/^\s*/)?.[0].length ?? 0) + 1,
      url,
      hit_breakpoints: reason === 'breakpoint' ? [line] : [],
      call_frames: [{ function: '(top level)', line, url }],
      scopes: [
        ...(indented ? [{ type: 'block', variables: [{ name: 'tab', type: 'object', value: '{id: 1, title: "Example Domain"}' }] }] : []),
        { type: 'module', variables: variables(s.lines, line) },
      ],
    };
    emit('script.debug_paused', paused);
  };

  const end = (s: Session, error: ScriptFailure | null, stopped = false) => {
    sessions.delete(s.id);
    if (error) emit('script.debug_output', { id: s.id, stream: 'stderr', text: `error: ${error.message}` });
    emit('script.debug_ended', {
      id: s.id,
      ok: !error && !stopped,
      exit_code: stopped ? null : error ? 1 : 0,
      duration_ms: Date.now() - s.started,
      result: s.result,
      stopped,
      error,
    });
  };

  return {
    'script.debug_start': async (params) => {
      const { name, code } = await codeOf(params);
      const s: Session = {
        id: `dbg${nextSession++}`,
        name,
        lines: code.split('\n'),
        exec: executableLines(code),
        pos: -1,
        bps: [],
        result: null,
        started: Date.now(),
        paused: false,
      };
      s.bps = resolve(s, params.breakpoints ?? []);
      sessions.set(s.id, s);
      advance(s, false);
      return { id: s.id, breakpoints: s.bps };
    },
    'script.debug_cmd': ({ id, action, lines }) => {
      const s = sessions.get(id);
      if (!s) throw new RpcError('NOT_FOUND', `debug session '${id}' not found (it may have ended)`);
      switch (action) {
        case 'set_breakpoints':
          s.bps = resolve(s, lines ?? []);
          return { breakpoints: s.bps };
        case 'stop':
          end(s, null, true);
          return {};
        case 'pause':
          return {};
        case 'continue':
          if (s.paused) advance(s, false);
          return {};
        default:
          // step_over / step_into / step_out: the toy has no calls, every step is a line.
          if (s.paused) advance(s, true);
          return {};
      }
    },
  };
}
