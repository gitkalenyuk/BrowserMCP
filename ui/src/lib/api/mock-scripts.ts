// Fakes for the Scripts panel's commands (script.*), used by MockApi so `pnpm dev` demos the
// library, Run and the step debugger without a backend. Shapes follow crates/jobs; the
// "execution" is a toy: console.log lines with a string literal print it, `throw new Error("…")`
// fails, `browser.output(…)` reports a result, and the debugger walks the non-empty lines.

import { RpcError } from './client';
import type { ApiEvents, EventName, Method, Params, Result } from './types';
import type { DebugPaused, DebugVariable, ScriptFailure, ScriptInfo, ScriptRunOutcome } from './types-scripts';

type ScriptMethod = Extract<Method, `script.${string}`>;
export type ScriptHandlers = { [M in ScriptMethod]: (params: Params<M>) => Result<M> | Promise<Result<M>> };
type Emit = <E extends EventName>(event: E, payload: ApiEvents[E]) => void;

const SEED: Record<string, string> = {
  hello: `// hello.ts: list the tabs and greet someone.
const name = inputs.name ?? "world";
console.log("hello from the mock script host");

const { tabs } = await browser.tabs.list();
let count = 0;
for (const tab of tabs) {
  count += 1;
  console.log(\`- \${tab.title}\`);
}

browser.output({ name, tabs: count });
`,
  'login-form': `// login-form.ts: fill a login form with the Run form values.
const email = inputs.email;
const password = inputs.password;
if (!email || !password) throw new Error("set the email and password inputs");

await browser.nav.goto({ url: "https://example.com/login" });
const field = await browser.page.find({ name: "Email" });
console.log("found the email field");
browser.output({ email, ok: true });
`,
};

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

export function createScriptMocks(emit: Emit, opts: { delay?: number } = {}): ScriptHandlers {
  const delay = opts.delay ?? 140;
  const files = new Map<string, { code: string; modified: number }>(
    Object.entries(SEED).map(([name, code], i) => [name, { code, modified: Date.now() - (i + 1) * 3_600_000 }]),
  );
  const sessions = new Map<string, Session>();
  let nextSession = 1;

  const info = (name: string): ScriptInfo => {
    const f = files.get(name)!;
    return { name, size: new TextEncoder().encode(f.code).length, modified_ms: f.modified };
  };
  const require = (name: string) => {
    const f = files.get(name);
    if (!f) throw new RpcError('NOT_FOUND', `script '${name}' not found`);
    return f;
  };
  const codeOf = (p: { name?: string; code?: string }): { name: string; code: string } => {
    if (p.name !== undefined && p.code === undefined) return { name: p.name, code: require(p.name).code };
    if (p.code !== undefined && p.name === undefined) return { name: '.adhoc', code: p.code };
    throw new RpcError('INVALID_PARAMS', 'give exactly one of `name` or `code`');
  };
  const later = (fn: () => void) => setTimeout(fn, delay);

  // ---- the toy debugger --------------------------------------------------------------------

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
    const paused: DebugPaused = {
      id: s.id,
      reason,
      line,
      column: (s.lines[line - 1].match(/^\s*/)?.[0].length ?? 0) + 1,
      url: `file:///data/scripts/${s.name}.ts`,
      hit_breakpoints: reason === 'breakpoint' ? [line] : [],
      call_frames: [{ function: '(top level)', line, url: `file:///data/scripts/${s.name}.ts` }],
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
    'script.list': () => ({ scripts: [...files.keys()].sort().map(info) }),
    'script.get': ({ name }) => ({ name, code: require(name).code }),
    'script.save': ({ name, code }) => {
      if (!/^[A-Za-z0-9][A-Za-z0-9_-]{0,63}(\.v\d+)?$/.test(name)) throw new RpcError('INVALID_PARAMS', `invalid script name "${name}"`);
      files.set(name, { code, modified: Date.now() });
      return info(name);
    },
    'script.delete': ({ name }) => ({ deleted: files.delete(name) }),
    'script.run': async (params): Promise<ScriptRunOutcome> => {
      const { code } = codeOf(params);
      const started = Date.now();
      await new Promise((r) => setTimeout(r, delay * 3));
      const stdout: string[] = [];
      let result: unknown = null;
      const lines = code.split('\n');
      for (let i = 0; i < lines.length; i++) {
        const err = throwText(lines[i]);
        if (err && !/if\s*\(/.test(lines[i])) {
          return {
            ok: false,
            exit_code: 1,
            duration_ms: Date.now() - started,
            result,
            stdout: stdout.join('\n'),
            stderr: `error: Uncaught Error: ${err}\n    at file:///data/scripts/x.ts:${i + 1}:7\n`,
            error: { code: 'SCRIPT_ERROR', message: `Uncaught Error: ${err}`, line: i + 1 },
          };
        }
        const log = logText(lines[i]);
        if (log !== null) stdout.push(log);
        if (lines[i].includes('browser.output(')) result = { mock: true, inputs: params.inputs ?? {} };
      }
      return { ok: true, exit_code: 0, duration_ms: Date.now() - started, result, stdout: stdout.join('\n'), stderr: '' };
    },
    'script.debug_start': (params) => {
      const { name, code } = codeOf(params);
      const lines = code.split('\n');
      const s: Session = {
        id: `dbg${nextSession++}`,
        name,
        lines,
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
