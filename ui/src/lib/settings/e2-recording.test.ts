import { describe, expect, it } from 'vitest';
import type { RecordingSettings, ScriptSettings } from '../api/types-e2-settings';
import { recordErrors, recordForm, recordPatch, sameSettings, scriptErrors, scriptForm, scriptPatch } from './e2-recording';

const video: RecordingSettings = { video_fps: 10, video_format: 'webm', video_quality: 80 };
const scripts: ScriptSettings = { script_timeout_secs: 300, run_timeout_secs: 300, run_parallel: 4, run_retries: 1 };

describe('video form', () => {
  it('round-trips and patches only what changed', () => {
    const f = recordForm(video);
    expect(f).toEqual({ fps: '10', format: 'webm', quality: '80' });
    expect(recordPatch(video, f)).toEqual({});
    expect(recordPatch(video, { ...f, fps: '24', format: 'mp4' })).toEqual({ video_fps: 24, video_format: 'mp4' });
  });

  it('checks the ranges the core checks', () => {
    expect(recordErrors(recordForm(video))).toEqual({});
    expect(recordErrors({ fps: '0', format: 'webm', quality: '101' })).toEqual({
      fps: 'settings.recording.err.fps',
      quality: 'settings.recording.err.quality',
    });
    expect(recordErrors({ fps: '2.5', format: 'webm', quality: '' }).fps).toBe('settings.recording.err.fps');
  });
});

describe('script form', () => {
  it('round-trips and patches only what changed', () => {
    const f = scriptForm(scripts);
    expect(scriptPatch(scripts, f)).toEqual({});
    expect(scriptPatch(scripts, { ...f, parallel: '8', retries: '0' })).toEqual({ run_parallel: 8, run_retries: 0 });
  });

  it('checks the ranges, with the hard parallel limit from the core', () => {
    expect(scriptErrors(scriptForm(scripts), 32)).toEqual({});
    expect(scriptErrors({ scriptTimeout: '0', runTimeout: '86401', parallel: '33', retries: '11' }, 32)).toEqual({
      scriptTimeout: 'settings.recording.err.timeout',
      runTimeout: 'settings.recording.err.timeout',
      parallel: 'settings.recording.err.parallel',
      retries: 'settings.recording.err.retries',
    });
    expect(scriptErrors({ ...scriptForm(scripts), parallel: '8' }, 4).parallel).toBe('settings.recording.err.parallel');
  });

  it('compares settings with the defaults', () => {
    expect(sameSettings(scripts, { ...scripts })).toBe(true);
    expect(sameSettings(scripts, { ...scripts, run_retries: 2 })).toBe(false);
  });
});
