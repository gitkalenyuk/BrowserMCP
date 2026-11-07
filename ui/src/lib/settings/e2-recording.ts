// Settings → Recording & scripts: forms for record.settings and script.settings, the patch of
// what changed and the checks the core also does. Pure functions (e2-recording.test.ts).

import type { RecordSettingsArgs, RecordingSettings, ScriptSettings, ScriptSettingsArgs } from '../api/types-e2-settings';

export type VideoFormat = RecordingSettings['video_format'];

/** Form values as typed (numbers stay text until saved, so half-typed input is not lost). */
export interface RecordForm {
  fps: string;
  format: VideoFormat;
  quality: string;
}

export interface ScriptForm {
  scriptTimeout: string;
  runTimeout: string;
  parallel: string;
  retries: string;
}

export const recordForm = (s: RecordingSettings): RecordForm => ({
  fps: String(s.video_fps),
  format: s.video_format,
  quality: String(s.video_quality),
});

export const scriptForm = (s: ScriptSettings): ScriptForm => ({
  scriptTimeout: String(s.script_timeout_secs),
  runTimeout: String(s.run_timeout_secs),
  parallel: String(s.run_parallel),
  retries: String(s.run_retries),
});

/** Integer in [min, max], else null. */
function int(text: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(text.trim())) return null;
  const n = Number(text);
  return n >= min && n <= max ? n : null;
}

export function recordErrors(f: RecordForm): Partial<Record<keyof RecordForm, string>> {
  const e: Partial<Record<keyof RecordForm, string>> = {};
  if (int(f.fps, 1, 60) === null) e.fps = 'settings.recording.err.fps';
  if (int(f.quality, 1, 100) === null) e.quality = 'settings.recording.err.quality';
  return e;
}

export const MAX_TIMEOUT_SECS = 86_400;

export function scriptErrors(f: ScriptForm, maxParallel: number): Partial<Record<keyof ScriptForm, string>> {
  const e: Partial<Record<keyof ScriptForm, string>> = {};
  if (int(f.scriptTimeout, 1, MAX_TIMEOUT_SECS) === null) e.scriptTimeout = 'settings.recording.err.timeout';
  if (int(f.runTimeout, 1, MAX_TIMEOUT_SECS) === null) e.runTimeout = 'settings.recording.err.timeout';
  if (int(f.parallel, 1, maxParallel) === null) e.parallel = 'settings.recording.err.parallel';
  if (int(f.retries, 0, 10) === null) e.retries = 'settings.recording.err.retries';
  return e;
}

/** record.settings arguments for the fields that differ from `s` (valid forms only). */
export function recordPatch(s: RecordingSettings, f: RecordForm): RecordSettingsArgs {
  const p: RecordSettingsArgs = {};
  if (Number(f.fps) !== s.video_fps) p.video_fps = Number(f.fps);
  if (f.format !== s.video_format) p.video_format = f.format;
  if (Number(f.quality) !== s.video_quality) p.video_quality = Number(f.quality);
  return p;
}

export function scriptPatch(s: ScriptSettings, f: ScriptForm): ScriptSettingsArgs {
  const p: ScriptSettingsArgs = {};
  if (Number(f.scriptTimeout) !== s.script_timeout_secs) p.script_timeout_secs = Number(f.scriptTimeout);
  if (Number(f.runTimeout) !== s.run_timeout_secs) p.run_timeout_secs = Number(f.runTimeout);
  if (Number(f.parallel) !== s.run_parallel) p.run_parallel = Number(f.parallel);
  if (Number(f.retries) !== s.run_retries) p.run_retries = Number(f.retries);
  return p;
}

/** Same settings as the defaults (the reset button is then pointless). */
export function sameSettings<T extends object>(a: T, b: T): boolean {
  return (Object.keys(a) as (keyof T)[]).every((k) => a[k] === b[k]);
}

