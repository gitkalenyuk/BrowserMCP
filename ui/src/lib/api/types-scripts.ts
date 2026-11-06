// Wire types of the Scripts panel's step debugger (crates/jobs/src/debug.rs). Params and
// results come from the generated protocol types (`cargo xtask gen-sdk`); the event payloads are
// built with `json!` in Rust and are written out here. The library and Run methods
// (script.list/get/save/delete/run) are declared in types-record-runs.ts.
//
// types.ts re-exports this file; the `declare module` below adds the methods and events to
// RpcMethods / ApiEvents (declaration merging).

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';

/** One saved script (data/scripts/<name>.ts). */
export type ScriptInfo = Commands['script.list']['output']['scripts'][number];
/** Result of script.run. */
export type ScriptRunOutcome = Commands['script.run']['output'];
/** A parsed script failure: `{code, command?, message, line?}`. */
export type ScriptFailure = NonNullable<ScriptRunOutcome['error']>;
export type DebugStartArgs = Commands['script.debug_start']['args'];
export type DebugStartResult = Commands['script.debug_start']['output'];
export type DebugCmdArgs = Commands['script.debug_cmd']['args'];
export type DebugAction = DebugCmdArgs['action'];

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

/** `script.debug_paused`: all lines are 1-based TypeScript lines. */
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
    'script.debug_start': { params: DebugStartArgs; result: DebugStartResult };
    'script.debug_cmd': { params: DebugCmdArgs; result: Commands['script.debug_cmd']['output'] };
  }
  interface ApiEvents {
    'script.debug_paused': DebugPaused;
    'script.debug_resumed': { id: string };
    'script.debug_output': DebugOutput;
    'script.debug_ended': DebugEnded;
  }
}
