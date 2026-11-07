// VoiceController against the mock voice.* commands, a fake microphone and a fake prompt
// field (the tests run in Node: no DOM). Checks the flows that touch the browser's streams
// and the prompt: a failed microphone, Enter while dictating, Ctrl+C.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { BrowserApi } from '../api';
import { createVoiceMocks } from '../api/mock-voice';
import type { VoiceSettingsPatch } from '../api/types-voice';
import { VoiceController } from './controller.svelte';

const mic = vi.hoisted(() => ({
  /** What the next MicCapture.start does. */
  next: 'ok' as 'ok' | 'deny' | 'hang',
  instances: [] as { handlers: { onPcm: (pcm: Int16Array) => void } | null; stopped: boolean; release?: () => void }[],
}));

vi.mock('./capture', async (importOriginal) => {
  const real = await importOriginal<typeof import('./capture')>();
  class FakeMic {
    handlers: { onPcm: (pcm: Int16Array) => void } | null = null;
    stopped = false;
    release?: () => void;
    constructor() {
      mic.instances.push(this);
    }
    start(h: { onPcm: (pcm: Int16Array) => void }): Promise<void> {
      this.handlers = h;
      if (mic.next === 'deny') return Promise.reject(new real.MicCaptureError('denied', 'Microphone access was denied.'));
      if (mic.next === 'hang') return new Promise((r) => (this.release = r));
      return Promise.resolve();
    }
    stop(): void {
      this.stopped = true;
    }
  }
  return { ...real, MicCapture: FakeMic };
});

/** A textarea as far as the controller cares. */
class FakeField extends EventTarget {
  value = '';
  selectionStart = 0;
  selectionEnd = 0;
  style: Record<string, string> = {};
  scrollTop = 0;
  scrollLeft = 0;
  setSelectionRange(a: number, b: number) {
    this.selectionStart = a;
    this.selectionEnd = b;
  }
}

function key(type: string, code: string, o: Record<string, unknown> = {}, target?: unknown): Event {
  const e = Object.assign(new Event(type, { cancelable: true, bubbles: true }), {
    code,
    key: code,
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    repeat: false,
    isComposing: false,
    ...o,
  });
  if (target) Object.defineProperty(e, 'target', { value: target });
  return e;
}

function setup(patch: VoiceSettingsPatch = {}) {
  const mocks = createVoiceMocks();
  if (Object.keys(patch).length) mocks['voice.settings_set'](patch);
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const handlers = mocks as unknown as Record<string, (p: unknown) => unknown>;
  const api = {
    call: (method: string, params: Record<string, unknown> = {}) => {
      calls.push({ method, params });
      try {
        return Promise.resolve(handlers[method](params));
      } catch (err) {
        return Promise.reject(err);
      }
    },
  } as unknown as BrowserApi;
  const onsubmit = vi.fn();
  const field = new FakeField();
  const voice = new VoiceController(api, { onsubmit });
  return { mocks, calls, voice, field, onsubmit };
}

/** One second of audio from the fake microphone, then let the 250 ms push run. */
async function speak(ms = 1000) {
  mic.instances.at(-1)!.handlers!.onPcm(new Int16Array((16_000 * ms) / 1000));
  await vi.advanceTimersByTimeAsync(300);
}

let selection = '';

beforeEach(() => {
  vi.useFakeTimers();
  mic.next = 'ok';
  mic.instances.length = 0;
  selection = '';
  vi.stubGlobal('window', Object.assign(new EventTarget(), { getSelection: () => ({ isCollapsed: !selection, toString: () => selection }) }));
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('a microphone that fails to start', () => {
  it('closes the stream that opened anyway', async () => {
    const { voice, field, calls, mocks } = setup();
    await voice.loadSettings();
    voice.attach(field as unknown as HTMLTextAreaElement);
    mic.next = 'deny';
    await voice.start('tap');
    await vi.advanceTimersByTimeAsync(0);
    expect(voice.phase).toBe('idle');
    expect(voice.notice?.kind).toBe('denied');
    const started = calls.find((c) => c.method === 'voice.stream_start');
    expect(started).toBeTruthy();
    const stop = calls.find((c) => c.method === 'voice.stream_stop');
    expect(stop?.params).toMatchObject({ stream: 'v1', cancel: true });
    // The stream is gone on the "browser" side.
    expect(() => mocks['voice.stream_push']({ stream: 'v1', pcm_base64: '' })).toThrow(/no voice stream/);
  });
});

describe('Enter while dictating', () => {
  it('finishes the dictation and sends the final text once (hold mode, auto-submit off)', async () => {
    const { voice, field, onsubmit, calls } = setup();
    await voice.loadSettings();
    voice.attach(field as unknown as HTMLTextAreaElement);
    let sentWith: string | null = null;
    onsubmit.mockImplementation(() => (sentWith = field.value));
    await voice.start('hold');
    expect(voice.phase).toBe('recording');
    await speak(1000);
    // The interim words are in the prompt already.
    expect(field.value).not.toBe('');
    const interim = field.value;
    expect(voice.finishAndSubmit()).toBe(true);
    // A second Enter while the final text is on its way changes nothing.
    expect(voice.finishAndSubmit()).toBe(true);
    await vi.advanceTimersByTimeAsync(10);
    expect(voice.phase).toBe('idle');
    expect(onsubmit).toHaveBeenCalledTimes(1);
    expect(sentWith).toBe('Open the weather forecast for Kyiv and sum it up briefly');
    expect(sentWith).not.toBe(interim);
    expect(calls.filter((c) => c.method === 'voice.stream_stop')).toHaveLength(1);
    // Done: the next Enter is an ordinary send.
    expect(voice.finishAndSubmit()).toBe(false);
  });

  it('sends once in tap mode too (auto-submit on does not send a second time)', async () => {
    const { voice, field, onsubmit } = setup({ mode: 'tap' });
    await voice.loadSettings();
    voice.attach(field as unknown as HTMLTextAreaElement);
    await voice.start('tap');
    await speak(1000);
    expect(voice.finishAndSubmit()).toBe(true);
    await vi.advanceTimersByTimeAsync(10);
    expect(onsubmit).toHaveBeenCalledTimes(1);
  });

  it('pressed while the microphone is still starting: sends once it has finished', async () => {
    const { voice, field, onsubmit } = setup();
    await voice.loadSettings();
    voice.attach(field as unknown as HTMLTextAreaElement);
    mic.next = 'hang';
    const starting = voice.start('hold');
    await vi.advanceTimersByTimeAsync(0);
    expect(voice.phase).toBe('starting');
    expect(voice.finishAndSubmit()).toBe(true);
    mic.instances.at(-1)!.release!();
    await starting;
    await vi.advanceTimersByTimeAsync(10);
    expect(voice.phase).toBe('idle');
    expect(onsubmit).toHaveBeenCalledTimes(1);
  });

  it('is an ordinary send when not dictating', () => {
    const { voice } = setup();
    expect(voice.finishAndSubmit()).toBe(false);
  });
});

describe('Ctrl+C during a recording', () => {
  async function recording() {
    const s = setup();
    await s.voice.loadSettings();
    s.voice.attach(s.field as unknown as HTMLTextAreaElement);
    await s.voice.start('tap');
    expect(s.voice.phase).toBe('recording');
    return s;
  }

  it('elsewhere on the page is the usual copy', async () => {
    const { voice } = await recording();
    const other = new EventTarget();
    const e = key('keydown', 'KeyC', { ctrlKey: true }, other);
    window.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
    expect(voice.phase).toBe('recording');
  });

  it('in the prompt with text selected on the page is the usual copy', async () => {
    const { voice, field } = await recording();
    selection = 'some text';
    const e = key('keydown', 'KeyC', { ctrlKey: true }, field);
    window.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(false);
    expect(voice.phase).toBe('recording');
  });

  it('in the prompt or on the mic button with nothing selected cancels', async () => {
    const { voice, field } = await recording();
    const e = key('keydown', 'KeyC', { ctrlKey: true }, field);
    window.dispatchEvent(e);
    expect(e.defaultPrevented).toBe(true);
    expect(voice.phase).toBe('idle');
    const button = new EventTarget();
    voice.button = button as unknown as HTMLElement;
    await voice.start('tap');
    window.dispatchEvent(key('keydown', 'KeyC', { ctrlKey: true }, button));
    expect(voice.phase).toBe('idle');
  });

  it('Esc cancels from anywhere', async () => {
    const { voice } = await recording();
    window.dispatchEvent(key('keydown', 'Escape', { key: 'Escape' }, new EventTarget()));
    expect(voice.phase).toBe('idle');
  });
});

describe('auto-stop while Space is still held', () => {
  it('swallows the key repeats until Space is released', async () => {
    const { voice, field } = setup();
    await voice.loadSettings();
    voice.attach(field as unknown as HTMLTextAreaElement);
    field.dispatchEvent(key('keydown', 'Space', { key: ' ' }));
    field.dispatchEvent(key('keydown', 'Space', { key: ' ', repeat: true }));
    field.dispatchEvent(key('keydown', 'Space', { key: ' ', repeat: true }));
    await vi.advanceTimersByTimeAsync(0);
    expect(voice.phase).toBe('recording');
    // The silence / 2-minute auto-stop ends the recording while the key is still down.
    await voice.stop();
    expect(voice.phase).toBe('idle');
    const repeat = key('keydown', 'Space', { key: ' ', repeat: true });
    field.dispatchEvent(repeat);
    expect(repeat.defaultPrevented).toBe(true);
    expect(voice.phase).toBe('idle');
    field.dispatchEvent(key('keyup', 'Space', { key: ' ' }));
    // A new press behaves as usual again (types a space, warms up).
    const press = key('keydown', 'Space', { key: ' ' });
    field.dispatchEvent(press);
    expect(press.defaultPrevented).toBe(false);
    expect(voice.phase).toBe('warmup');
  });
});
