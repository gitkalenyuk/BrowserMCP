// Wire types of the Recording and Runs panels: record.* (crates/record) and script.* / run.* /
// trigger.* (crates/jobs). Params and results come from the generated protocol types
// (sdk/protocol/src/generated/commands.ts, `cargo xtask gen-sdk`); the event payloads are built
// with `json!` in Rust and are written out here (crates/record/src/{commands,session}.rs,
// crates/jobs/src/{lib,runner,triggers}.rs).
//
// types.ts re-exports this file; the `declare module` below adds the methods and events to
// RpcMethods / ApiEvents, so `api.call('run.start', …)` is typed like every other call.

import type { Commands } from '../../../../sdk/protocol/src/generated/commands';

/** `{}` arguments (`Record<string, never>`: record.list, script.list, …) become `void`. */
type NoArgs<A> = string extends keyof A ? (A[string & keyof A] extends never ? void : A) : A;

type Cmd<K extends keyof Commands> = {
  params: NoArgs<Commands[K]['args']>;
  result: Commands[K]['output'];
};

export interface RecordRunsMethods {
  'record.start': Cmd<'record.start'>;
  'record.stop': Cmd<'record.stop'>;
  'record.list': Cmd<'record.list'>;
  'record.get': Cmd<'record.get'>;
  'record.codegen': Cmd<'record.codegen'>;
  'record.video_start': Cmd<'record.video_start'>;
  'record.video_stop': Cmd<'record.video_stop'>;
  'script.list': Cmd<'script.list'>;
  'script.get': Cmd<'script.get'>;
  'script.save': Cmd<'script.save'>;
  'script.delete': Cmd<'script.delete'>;
  'script.run': Cmd<'script.run'>;
  'run.start': Cmd<'run.start'>;
  'run.status': Cmd<'run.status'>;
  'run.list': Cmd<'run.list'>;
  'run.results': Cmd<'run.results'>;
  'run.pause': Cmd<'run.pause'>;
  'run.resume': Cmd<'run.resume'>;
  'run.cancel': Cmd<'run.cancel'>;
  'run.export': Cmd<'run.export'>;
  'run.heal_apply': Cmd<'run.heal_apply'>;
  'trigger.set': Cmd<'trigger.set'>;
  'trigger.list': Cmd<'trigger.list'>;
  'trigger.remove': Cmd<'trigger.remove'>;
}

// ---- shapes the panels pass around ----------------------------------------------------------

export type RecordingEntry = Commands['record.list']['output']['recordings'][number];
export type RecordingDetail = Commands['record.get']['output'];
export type CodegenOutput = Commands['record.codegen']['output'];
export type CodegenInput = CodegenOutput['inputs'][number];
export type ScriptEntry = Commands['script.list']['output']['scripts'][number];
export type ScriptRunOutput = Commands['script.run']['output'];
export type ScriptError = NonNullable<ScriptRunOutput['error']>;
export type RunInfo = Commands['run.status']['output'];
export type RunStatus = RunInfo['status'];
export type RunCounts = RunInfo['counts'];
export type RunStartArgs = Commands['run.start']['args'];
export type RunResultsPage = Commands['run.results']['output'];
export type RunRow = RunResultsPage['rows'][number];
export type RowStatus = RunRow['status'];
export type ExportFormat = Commands['run.export']['args']['to'];
export type TriggerInfo = Commands['trigger.list']['output']['triggers'][number];

/** One line of a recording's actions.jsonl (`Step` in crates/record/src/model.rs). */
export interface RecordStep {
  seq: number;
  ts_ms: number;
  source: 'api' | 'human' | 'system';
  /** `start`, `goto`, `navigated`, `wait`, `command`, `click`, `fill`, `press`, … */
  action: string;
  target?: {
    ref?: string;
    role?: string;
    name?: string;
    text?: string;
    nth?: number;
    css?: string;
    tag?: string;
    input_type?: string;
    from?: string;
  };
  value?: string;
  /** The value is a password: stored as `***`. */
  secret?: boolean;
  url?: string;
  wait_for?: string;
  command?: { name: string; call_id: number; caller: unknown; args: unknown; ok?: boolean; error?: string; duration_ms?: number };
  /** Human copy of the API step with this seq (codegen skips it). */
  dup_of?: number;
  extra?: unknown;
}

/** `run.heal_needed` payload, also kept in `RunInfo.heal` while the run waits. */
export interface HealRequest {
  id: string;
  row: number;
  script: string;
  error: ScriptError | null;
  /** The browser command that failed (`page.act`). */
  step: string | null;
  line: number | null;
}

export interface RunProgress {
  id: string;
  done: number;
  ok: number;
  failed: number;
  needs_heal: number;
  cancelled: number;
  running: number;
  pending: number;
  total: number;
}

export interface RecordRunsEvents {
  'record.started': { id: string; tab: number; name: string; start_url: string };
  /** A step was added, or an API step got its result / locator (same seq again). */
  'record.step': { id: string; step: RecordStep };
  'record.stopped': { id: string; steps: number; video: string | null };
  'run.started': { id: string; script: string; total: number; parallel: number };
  'run.progress': RunProgress;
  'run.row_failed': { id: string; row: number; error: ScriptError | null; attempts: number };
  'run.heal_needed': HealRequest;
  'run.finished': {
    id: string;
    status: RunStatus;
    total: number;
    ok: number;
    failed: number;
    cancelled: number;
    needs_heal: number;
    duration_ms: number;
  };
  'trigger.fired': {
    id: string;
    script: string;
    ok: boolean;
    exit_code: number | null;
    duration_ms: number;
    result?: unknown;
    error?: { code: string; message: string } | null;
  };
}

declare module './types' {
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface RpcMethods extends RecordRunsMethods {}
  // eslint-disable-next-line @typescript-eslint/no-empty-object-type
  interface ApiEvents extends RecordRunsEvents {}
}
