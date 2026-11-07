import { describe, expect, it } from 'vitest';
import { createVoiceMocks } from '../api/mock-voice';
import { HotkeyMachine, formatHotkey, hotkeyFromEvent, matchesKey, parseHotkey, type KeyEventLike } from './keys';
import { VOICE_LANGUAGES, languageName } from './languages';
import { FailureBackoff, MAX_RECORDING_MS, SILENCE_STOP_MS, SPEECH_LEVEL, autoStopReason, joinDictation, shouldAutoSubmit, wordCount } from './text';
import { bytesToBase64, downsample, encodeWav, floatTo16, int16ToBytes, levelOf } from './wav';

const ev = (code: string, o: Partial<KeyEventLike> = {}): KeyEventLike => ({
  code,
  ctrlKey: false,
  altKey: false,
  shiftKey: false,
  metaKey: false,
  repeat: false,
  ...o,
});

describe('word count and auto-submit', () => {
  it('counts words with letters or digits, in any script', () => {
    expect(wordCount('')).toBe(0);
    expect(wordCount('  — ... ')).toBe(0);
    expect(wordCount('Відкрий пошту')).toBe(2);
    expect(wordCount('open 3 tabs, please!')).toBe(4);
  });

  it('submits only with the option on for that mode and three words or more', () => {
    const s = { auto_submit_hold: false, auto_submit_tap: true };
    expect(shouldAutoSubmit('one two three', 'tap', s)).toBe(true);
    expect(shouldAutoSubmit('one two', 'tap', s)).toBe(false);
    expect(shouldAutoSubmit('one two three', 'hold', s)).toBe(false);
    expect(shouldAutoSubmit('один два три', 'hold', { auto_submit_hold: true, auto_submit_tap: false })).toBe(true);
  });
});

describe('inserting dictation at the cursor', () => {
  it('adds spaces only where words would touch', () => {
    expect(joinDictation('', 'hello world', '')).toEqual({ value: 'hello world', caret: 11, start: 0, length: 11 });
    expect(joinDictation('Find', 'cheap flights', '')).toMatchObject({ value: 'Find cheap flights', caret: 18, start: 5 });
    expect(joinDictation('Find ', 'cheap', 'to Lviv')).toMatchObject({ value: 'Find cheap to Lviv', caret: 10 });
    expect(joinDictation('a', 'b', '.')).toMatchObject({ value: 'a b.' });
    expect(joinDictation('x', '   ', 'y')).toEqual({ value: 'xy', caret: 1, start: 1, length: 0 });
  });
});

describe('failure backoff', () => {
  it('pauses for 10 s after three failures within 10 s', () => {
    const b = new FailureBackoff();
    expect(b.fail(0)).toBe(false);
    expect(b.fail(4000)).toBe(false);
    expect(b.remaining(4000)).toBe(0);
    expect(b.fail(9000)).toBe(true);
    expect(b.remaining(9000)).toBe(10_000);
    expect(b.remaining(15_000)).toBe(4000);
    expect(b.remaining(19_000)).toBe(0);
  });

  it('forgets old failures and resets on success', () => {
    const b = new FailureBackoff();
    b.fail(0);
    b.fail(1000);
    expect(b.fail(12_000)).toBe(false); // the first two are older than 10 s
    b.succeed();
    expect(b.fail(12_500)).toBe(false);
    expect(b.fail(13_000)).toBe(false);
    expect(b.fail(13_500)).toBe(true);
  });

  it('auto-stops after 15 s of silence or 2 minutes', () => {
    expect(autoStopReason(1000, 0, 0)).toBeNull();
    expect(autoStopReason(SILENCE_STOP_MS, 0, 0)).toBe('silence');
    expect(autoStopReason(SILENCE_STOP_MS + 5, 0, 10)).toBeNull();
    expect(autoStopReason(MAX_RECORDING_MS, 0, MAX_RECORDING_MS - 1)).toBe('limit');
  });
});

describe('hotkeys', () => {
  it('parses, formats and matches by physical key', () => {
    expect(parseHotkey('ctrl + space')).toEqual({ ctrl: true, alt: false, shift: false, meta: false, key: 'Space' });
    expect(formatHotkey(parseHotkey('shift+alt+v')!)).toBe('Alt+Shift+V');
    expect(parseHotkey('f8')!.key).toBe('F8');
    expect(parseHotkey('Hyper+V')).toBeNull();
    expect(parseHotkey('')).toBeNull();
    const altV = parseHotkey('Alt+V')!;
    // A Ukrainian layout types "м" on that key: event.code still says KeyV.
    expect(matchesKey(ev('KeyV', { altKey: true, key: 'м' }), altV)).toBe(true);
    expect(matchesKey(ev('KeyV', { altKey: true, ctrlKey: true }), altV)).toBe(false);
    expect(matchesKey(ev('KeyV'), altV)).toBe(false);
  });

  it('reads a combination pressed in Settings', () => {
    expect(hotkeyFromEvent(ev('Space', { ctrlKey: true }))).toBe('Ctrl+Space');
    expect(hotkeyFromEvent(ev('KeyV', { altKey: true }))).toBe('Alt+V');
    expect(hotkeyFromEvent(ev('Space'))).toBe('Space');
    expect(hotkeyFromEvent(ev('F8'))).toBe('F8');
    // A plain letter would steal typing; a lone modifier is not a key yet.
    expect(hotkeyFromEvent(ev('KeyV'))).toBeNull();
    expect(hotkeyFromEvent(ev('ControlLeft', { ctrlKey: true }))).toBeNull();
  });
});

describe('hold / tap state machine', () => {
  const space = parseHotkey('Space')!;

  it('hold Space: a single tap types a space', () => {
    const m = new HotkeyMachine(space, 'hold');
    expect(m.keydown(ev('Space'), { empty: true, recording: false })).toEqual({ prevent: false, action: 'warmup' });
    expect(m.keyup(ev('Space'))).toEqual({ prevent: false, action: 'abortWarmup' });
    expect(m.state).toBe('idle');
  });

  it('hold Space: key repeats start recording, release stops it', () => {
    const m = new HotkeyMachine(space, 'hold', 2);
    m.keydown(ev('Space'), { empty: true, recording: false });
    expect(m.keydown(ev('Space', { repeat: true }), { empty: false, recording: false })).toEqual({ prevent: true, action: 'none' });
    expect(m.keydown(ev('Space', { repeat: true }), { empty: false, recording: false })).toEqual({ prevent: true, action: 'start' });
    expect(m.keydown(ev('Space', { repeat: true }), { empty: false, recording: true })).toEqual({ prevent: true, action: 'none' });
    expect(m.keyup(ev('Space'))).toEqual({ prevent: true, action: 'stop' });
  });

  it('a reset while the key is held (auto-stop, Esc) swallows its repeats until release', () => {
    const m = new HotkeyMachine(space, 'hold', 1);
    m.keydown(ev('Space'), { empty: true, recording: false });
    expect(m.keydown(ev('Space', { repeat: true }), { empty: true, recording: false }).action).toBe('start');
    m.reset();
    expect(m.keydown(ev('Space', { repeat: true }), { empty: false, recording: false })).toEqual({ prevent: true, action: 'none' });
    expect(m.keyup(ev('Space')).action).toBe('none');
    // Released: Space types again.
    expect(m.keydown(ev('Space'), { empty: false, recording: false })).toEqual({ prevent: false, action: 'none' });
    // Tap mode: the key is not held after the press, so an auto-stop swallows nothing.
    const tap = new HotkeyMachine(space, 'tap');
    tap.keydown(ev('Space'), { empty: true, recording: false });
    tap.keyup(ev('Space'));
    tap.reset();
    expect(tap.keydown(ev('Space', { repeat: true }), { empty: false, recording: false }).prevent).toBe(false);
  });

  it('hold Space only works on an empty prompt; typing aborts the warm-up', () => {
    const m = new HotkeyMachine(space, 'hold');
    expect(m.keydown(ev('Space'), { empty: false, recording: false }).action).toBe('none');
    m.keydown(ev('Space'), { empty: true, recording: false });
    expect(m.keydown(ev('KeyA'), { empty: false, recording: false }).action).toBe('abortWarmup');
    expect(m.state).toBe('idle');
  });

  it('hold combo: starts at once, stops when the key or its modifier is released', () => {
    const m = new HotkeyMachine(parseHotkey('Ctrl+Space')!, 'hold');
    expect(m.keydown(ev('Space', { ctrlKey: true }), { empty: false, recording: false })).toEqual({ prevent: true, action: 'start' });
    expect(m.keyup(ev('ControlLeft'))).toEqual({ prevent: true, action: 'stop' });
    m.keydown(ev('Space', { ctrlKey: true }), { empty: false, recording: false });
    expect(m.keyup(ev('KeyA')).action).toBe('none');
    expect(m.keyup(ev('Space'))).toEqual({ prevent: true, action: 'stop' });
  });

  it('tap: press to start (Space: empty prompt), press again to stop', () => {
    const m = new HotkeyMachine(space, 'tap');
    expect(m.keydown(ev('Space'), { empty: false, recording: false }).action).toBe('none');
    expect(m.keydown(ev('Space'), { empty: true, recording: false })).toEqual({ prevent: true, action: 'start' });
    expect(m.keyup(ev('Space')).action).toBe('none');
    // The prompt now holds the interim text; Space still stops.
    expect(m.keydown(ev('Space'), { empty: false, recording: true })).toEqual({ prevent: true, action: 'stop' });
    const alt = new HotkeyMachine(parseHotkey('Alt+V')!, 'tap');
    expect(alt.keydown(ev('KeyV', { altKey: true }), { empty: false, recording: false }).action).toBe('start');
    expect(alt.keydown(ev('KeyV', { altKey: true, repeat: true }), { empty: false, recording: true })).toEqual({ prevent: true, action: 'none' });
    expect(alt.keydown(ev('KeyV', { altKey: true }), { empty: false, recording: true }).action).toBe('stop');
  });
});

describe('WAV encoder', () => {
  it('writes a 16 kHz mono PCM header and little-endian samples', () => {
    const wav = encodeWav(new Int16Array([1, -2, 32767]));
    const v = new DataView(wav.buffer);
    expect(wav.length).toBe(44 + 6);
    expect(String.fromCharCode(...wav.subarray(0, 4))).toBe('RIFF');
    expect(v.getUint32(4, true)).toBe(36 + 6);
    expect(String.fromCharCode(...wav.subarray(8, 16))).toBe('WAVEfmt ');
    expect(v.getUint16(22, true)).toBe(1);
    expect(v.getUint32(24, true)).toBe(16_000);
    expect(v.getUint32(28, true)).toBe(32_000);
    expect(v.getUint16(34, true)).toBe(16);
    expect(String.fromCharCode(...wav.subarray(36, 40))).toBe('data');
    expect(v.getUint32(40, true)).toBe(6);
    expect([v.getInt16(44, true), v.getInt16(46, true), v.getInt16(48, true)]).toEqual([1, -2, 32767]);
  });

  it('converts, resamples, measures and encodes', () => {
    expect([...floatTo16(new Float32Array([0, 1, -1, 2, -0.5]))]).toEqual([0, 32767, -32768, 32767, -16384]);
    const down = downsample(new Float32Array([1, 1, 1, 0, 0, 0]), 48_000, 16_000);
    expect([...down]).toEqual([1, 0]);
    expect(() => downsample(new Float32Array(4), 8000, 16_000)).toThrow();
    expect([...int16ToBytes(new Int16Array([0x0102]))]).toEqual([0x02, 0x01]);
    expect(bytesToBase64(new Uint8Array([104, 105]))).toBe('aGk=');
    expect(bytesToBase64(new Uint8Array(100_000)).length).toBe(133_336);
    expect(levelOf(new Float32Array(10))).toBe(0);
    expect(levelOf(new Float32Array(10).fill(0.25))).toBe(1);
    expect(levelOf(new Float32Array(10).fill(0.01))).toBeGreaterThan(0.15);
  });

  it('tells room noise from speech for the silence auto-stop', () => {
    expect(levelOf(new Float32Array(100).fill(0.003))).toBeLessThan(SPEECH_LEVEL);
    expect(levelOf(new Float32Array(100).fill(0.05))).toBeGreaterThan(SPEECH_LEVEL);
  });
});

describe('language names', () => {
  it('names Whisper languages in the UI language', () => {
    expect(languageName('uk', 'en-US')).toBe('Ukrainian');
    expect(languageName('uk', 'uk-UA')).toBe('Українська');
    expect(languageName('qq', 'en-US')).toBe('qq');
    expect(new Set(VOICE_LANGUAGES).size).toBe(VOICE_LANGUAGES.length);
  });
});

describe('voice mock (MockApi)', () => {
  it('keeps keys secret, validates and reports readiness', () => {
    const m = createVoiceMocks({ aiKeys: { groq: true } });
    let s = m['voice.settings_set']({ engine: 'openai', openai: { api_key: 'sk-test-12345678' }, hotkey: 'alt+v' });
    expect(s.openai.api_key_set).toBe(true);
    expect(s.openai.api_key_hint).toBe('…5678');
    expect(JSON.stringify(s)).not.toContain('sk-test');
    expect(s.hotkey).toBe('Alt+V');
    expect(() => m['voice.settings_set']({ hotkey: 'q' })).toThrow(/hotkey/);
    expect(() => m['voice.settings_set']({ local: { model: 'huge' } })).toThrow(/model/);
    const engines = m['voice.engines']();
    expect(engines.engines.find((e) => e.id === 'groq')).toMatchObject({ configured: true, key_source: 'ai_provider' });
    expect(engines.engines.find((e) => e.id === 'deepgram')).toMatchObject({ configured: false, interim: true });
    s = m['voice.settings_set']({ openai: { api_key: '' } });
    expect(s.openai.api_key_set).toBe(false);
  });

  it('streams interim text for local Whisper and the final sentence at stop', () => {
    const m = createVoiceMocks();
    const st = m['voice.stream_start']({ language: 'uk' });
    expect(st.interim).toBe(true);
    // 1 s of 16 kHz 16-bit audio = 32 000 bytes ≈ 42 668 base64 characters.
    const second = 'A'.repeat(42_668);
    const p1 = m['voice.stream_push']({ stream: st.stream, pcm_base64: second });
    expect(p1.partial.split(' ').length).toBe(2);
    const done = m['voice.stream_stop']({ stream: st.stream });
    expect(done.text).toMatch(/^Відкрий прогноз погоди/);
    expect(() => m['voice.stream_push']({ stream: st.stream, pcm_base64: '' })).toThrow(/no voice stream/);
    const c = m['voice.stream_start']({});
    m['voice.stream_push']({ stream: c.stream, pcm_base64: second });
    expect(m['voice.stream_stop']({ stream: c.stream, cancel: true }).text).toBe('');
  });

  it('downloads with progress, then the model is installed', async () => {
    const m = createVoiceMocks();
    m['voice.local_download']({ what: 'small' });
    expect(() => m['voice.local_download']({ what: 'tiny' })).toThrow(/still downloading/);
    await new Promise((r) => setTimeout(r, 3400));
    const s = m['voice.local_status']();
    expect(s.download?.phase).toBe('done');
    expect(s.models.find((x) => x.id === 'small')?.installed).toBe(true);
    expect(s.active_model).toBe('small');
    m['voice.local_delete']({ what: 'small' });
    expect(m['voice.local_status']().active_model).toBe('base');
  }, 10_000);
});
