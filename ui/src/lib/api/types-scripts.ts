// Wire types of the Scripts panel: the script library, script.run and the step debugger
// (crates/jobs/src/{commands,debug,host}.rs). They are merged into RpcMethods / ApiEvents of
// ./types (declaration merging), so `api.call('script.list')` is typed like every other call.

/** One saved script (data/scripts/<name>.ts). */
export interface ScriptInfo {
  /** Name without .ts, e.g. "login" or "login.v2". */
  name: string;
  size: number;
  modified_ms: number;
}

/** A parsed script failure (crates/jobs/src/host.rs `ScriptFailure`). */
export interface ScriptFailure {
  /** NOT_FOUND, TIMEOUT, … for a failed browser call; else SCRIPT_ERROR, SCRIPT_TIMEOUT, HOST_ERROR, CANCELLED. */
  code: string;
  /** The browser command that failed, e.g. "page.find". */
  command?: string;
  message: string;
  /** TypeScript line of the top stack frame. */
  line?: number;
}

/** Result of script.run. */
export interface ScriptRunOutcome {
  ok: boolean;
  exit_code: number | null;
  duration_ms: number;
  /** Merged browser.output(...) values; null when none. */
  result: unknown;
  /** Last few KB of ordinary stdout / stderr lines. */
  stdout: string;
  stderr: string;
  error?: ScriptFailure;
}

export type ScriptSourceArgs = { name: string; code?: never } | { code: string; name?: never };

export type ScriptRunArgs = ScriptSourceArgs & {
  inputs?: Record<string, string>;
  timeout_secs?: number;
};

export type DebugStartArgs = ScriptSourceArgs & {
  inputs?: Record<string, string>;
  /** TypeScript lines (1-based). */
  breakpoints?: number[];
  timeout_secs?: number;
};

export interface DebugStartResult {
  id: string;
  /** Where the breakpoints really landed. */
  breakpoints: number[];
  /** The script never started (e.g. a syntax error). */
  error?: ScriptFailure;
}

export type DebugAction = 'continue' | 'step_over' | 'step_into' | 'step_out' | 'pause' | 'set_breakpoints' | 'stop';

export interface DebugVariable {
  name: string;
  /** number, string, object, array, function, null, undefined, … */
  type: string;
  /** Short, truncated text. */
  value: string;
}

export interface DebugScope {
  /** local, block, closure, module, … (global is left out). */
  type: string;
  variables: DebugVariable[];
}

export interface DebugFrame {
  function: string;
  line: number;
  url: string;
}

export interface DebugPaused {
  id: string;
  /** breakpoint, step, … */
  reason: string;
  line: number;
  column: number;
  url: string;
  hit_breakpoints: number[];
  call_frames: DebugFrame[];
  scopes: DebugScope[];
}

export interface DebugOutput {
  id: string;
  stream: 'stdout' | 'stderr' | 'result';
  text: string;
}

export interface DebugEnded {
  id: string;
  ok: boolean;
  exit_code: number | null;
  duration_ms: number;
  result: unknown;
  /** Ended by script.debug_cmd stop, the session timeout or shutdown. */
  stopped: boolean;
  error: ScriptFailure | null;
}

declare module './types' {
  interface RpcMethods {
    'script.list': { params: void; result: { scripts: ScriptInfo[] } };
    'script.get': { params: { name: string }; result: { name: string; code: string } };
    'script.save': { params: { name: string; code: string }; result: ScriptInfo };
    'script.delete': { params: { name: string }; result: { deleted: boolean } };
    'script.run': { params: ScriptRunArgs; result: ScriptRunOutcome };
    'script.debug_start': { params: DebugStartArgs; result: DebugStartResult };
    'script.debug_cmd': {
      params: { id: string; action: DebugAction; lines?: number[] };
      result: { breakpoints?: number[] };
    };
  }
  interface ApiEvents {
    'script.debug_paused': DebugPaused;
    'script.debug_resumed': { id: string };
    'script.debug_output': DebugOutput;
    'script.debug_ended': DebugEnded;
  }
}
