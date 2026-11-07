// Voice dictation for one prompt field (textarea / input), like Claude Code's /voice:
//
// * hold mode: hold the hotkey (Space on an empty prompt, after a short warm-up; or a combo)
//   to record, release to finish; tap mode: press to start, press again to stop;
// * the mic button: click = tap mode, press and hold = hold mode;
// * the transcript appears at the cursor while you speak (dimmed until final) and mixes with
//   typing; Esc (or Ctrl+C in the prompt / on the mic button, with nothing selected) cancels
//   and restores the prompt as it was;
// * Enter (send) while dictating finishes the dictation first, then sends the final text once;
// * auto-stop after 15 s of silence or 2 min; auto-submit with ≥3 words when enabled;
// * three failures within 10 s pause dictation for 10 s.
//
// Audio goes to the browser as 16 kHz PCM every 250 ms (voice.stream_*); see crates/voice.

import type { BrowserApi } from '../api';
import type { VoiceSettings } from '../api/types-voice';
import { locale } from '../i18n/index.svelte';
import { load, save } from '../storage';
import { MicCapture, MicCaptureError } from './capture';
import { HotkeyMachine, parseHotkey, type Hotkey } from './keys';
import { InterimOverlay } from './overlay';
import { FailureBackoff, SPEECH_LEVEL, autoStopReason, joinDictation, shouldAutoSubmit, type VoiceMode } from './text';
import { bytesToBase64, int16ToBytes } from './wav';

export type VoicePhase = 'idle' | 'warmup' | 'starting' | 'recording' | 'transcribing' | 'paused';
export type Field = HTMLTextAreaElement | HTMLInputElement;

/** Why the last recording ended or failed (shown under the button for a few seconds). */
export type VoiceNotice =
  | { kind: 'error'; text: string }
  | { kind: 'denied' | 'no-device' | 'unsupported' | 'silence' | 'limit' | 'nothing' | 'paused' | 'setup'; text?: string };

/** How many times the first-run hint is shown. */
export const HINT_TIMES = 5;
const PUSH_EVERY_MS = 250;
/** A mouse press longer than this is push-to-talk; shorter is a tap. */
const HOLD_PRESS_MS = 300;

interface Region {
  start: number;
  length: number;
  text: string;
}

interface Session {
  mode: VoiceMode;
  snapshot: { value: string; start: number; end: number };
  stream: string | null;
  interim: boolean;
  mic: MicCapture | null;
  queue: Int16Array[];
  pushing: Promise<void> | null;
  pushTimer: ReturnType<typeof setInterval> | undefined;
  startedAt: number;
  lastSpeechAt: number;
  stopRequested: boolean;
  /** Send the prompt once the final text is in (Enter / send pressed while dictating). */
  submitWhenDone: boolean;
  region: Region;
  partial: string;
}

export class VoiceController {
  phase = $state<VoicePhase>('idle');
  /** Microphone level 0..1. */
  level = $state(0);
  notice = $state<VoiceNotice | null>(null);
  settings = $state<VoiceSettings | null>(null);
  /** The mode of the recording in progress (or the configured one when idle). */
  mode = $state<VoiceMode>('hold');
  /** The engine shows text while you speak (else "transcribing…" at stop). */
  interim = $state(false);
  /** Show "hold Space to speak". */
  hint = $state(false);
  /** Seconds left of a pause after repeated failures. */
  pausedFor = $state(0);

  #api: BrowserApi;
  #target: Field | null = null;
  /** The mic button (Ctrl+C there cancels too). */
  #button: HTMLElement | null = null;
  #onsubmit: (() => void) | undefined;
  #context: (() => string | undefined) | undefined;
  #machine: HotkeyMachine | null = null;
  #hotkey: Hotkey | null = null;
  #session: Session | null = null;
  #warmupSnapshot: Session['snapshot'] | null = null;
  #backoff = new FailureBackoff();
  #overlay: InterimOverlay | null = null;
  #ownInput = false;
  #loadedAt = 0;
  #noticeTimer: ReturnType<typeof setTimeout> | undefined;
  #pauseTimer: ReturnType<typeof setInterval> | undefined;
  #pressAt = 0;
  #hintCounted = false;
  #now: () => number;

  constructor(api: BrowserApi, opts: { onsubmit?: () => void; context?: () => string | undefined; now?: () => number } = {}) {
    this.#api = api;
    this.#onsubmit = opts.onsubmit;
    this.#context = opts.context;
    this.#now = opts.now ?? (() => Date.now());
  }

  setCallbacks(opts: { onsubmit?: () => void; context?: () => string | undefined }): void {
    this.#onsubmit = opts.onsubmit;
    this.#context = opts.context;
  }

  get enabled(): boolean {
    return this.settings?.enabled ?? false;
  }

  get active(): boolean {
    return this.phase === 'starting' || this.phase === 'recording' || this.phase === 'transcribing';
  }

  /** The mic button of this prompt (set by VoiceInput). */
  set button(el: HTMLElement | null) {
    this.#button = el;
  }

  get hotkeyLabel(): string {
    return this.settings?.hotkey ?? 'Space';
  }

  async loadSettings(): Promise<void> {
    try {
      const s = await this.#api.call('voice.settings_get');
      this.settings = s;
      this.#loadedAt = this.#now();
      this.#hotkey = parseHotkey(s.hotkey) ?? parseHotkey('Space');
      this.#machine = new HotkeyMachine(this.#hotkey!, s.mode);
      if (!this.active) this.mode = s.mode;
      this.#updateHint();
    } catch {
      // No voice commands (an older browser): the button stays hidden.
      this.settings = null;
    }
  }

  /** Listen to `target`'s keys. Returns the detach function. */
  attach(target: Field): () => void {
    this.#target = target;
    this.#overlay = new InterimOverlay(target);
    const onKeyDown = (e: KeyboardEvent) => this.#keydown(e);
    const onKeyUp = (e: KeyboardEvent) => this.#keyup(e);
    const onFocus = () => {
      if (this.#now() - this.#loadedAt > 3000) void this.loadSettings();
      this.#hintCounted = false;
      this.#updateHint();
    };
    const onBlur = () => {
      // Releasing the key outside the field must still stop a hold recording.
      if (this.#machine?.state === 'warmup') this.#machine.reset();
      if (this.phase === 'warmup') this.phase = 'idle';
      this.#updateHint();
    };
    const onInput = () => {
      if (!this.#ownInput) this.#userTyped();
      this.#updateHint();
    };
    // The union type of `target` hides the typed overloads of addEventListener.
    const el: HTMLElement = target;
    el.addEventListener('keydown', onKeyDown);
    el.addEventListener('keyup', onKeyUp);
    el.addEventListener('focus', onFocus);
    el.addEventListener('blur', onBlur);
    el.addEventListener('input', onInput);
    window.addEventListener('keyup', onKeyUp);
    window.addEventListener('keydown', this.#windowKey, true);
    return () => {
      el.removeEventListener('keydown', onKeyDown);
      el.removeEventListener('keyup', onKeyUp);
      el.removeEventListener('focus', onFocus);
      el.removeEventListener('blur', onBlur);
      el.removeEventListener('input', onInput);
      window.removeEventListener('keyup', onKeyUp);
      window.removeEventListener('keydown', this.#windowKey, true);
      this.destroy();
    };
  }

  destroy(): void {
    if (this.#session) this.cancel();
    this.#overlay?.hide();
    clearTimeout(this.#noticeTimer);
    clearInterval(this.#pauseTimer);
  }

  // ------------------------------------------------------------------ keys

  /**
   * Esc anywhere on the page cancels a recording (focus may be on the mic button). Ctrl+C
   * cancels only in the prompt or on the mic button and only when nothing is selected:
   * elsewhere it is the usual copy.
   */
  #windowKey = (e: KeyboardEvent) => {
    if (this.phase === 'idle' || this.phase === 'paused') return;
    const ctrlC = (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.code === 'KeyC' && this.#ownsKey(e) && !this.#hasSelection();
    if (e.key === 'Escape' || ctrlC) {
      e.preventDefault();
      e.stopPropagation();
      this.cancel();
    }
  };

  /** The key went to this prompt or its mic button. */
  #ownsKey(e: Event): boolean {
    const at = e.target;
    return !!at && (at === this.#target || at === this.#button);
  }

  /** Text selected in the prompt or anywhere on the page (Ctrl+C then copies it). */
  #hasSelection(): boolean {
    const t = this.#target;
    if (t && t.selectionStart !== t.selectionEnd) return true;
    try {
      const sel = typeof window !== 'undefined' ? window.getSelection?.() : null;
      return !!sel && !sel.isCollapsed && sel.toString() !== '';
    } catch {
      return false;
    }
  }

  #keydown(e: KeyboardEvent): void {
    if (!this.enabled || !this.#machine || e.isComposing) return;
    const t = this.#target!;
    const recording = this.phase === 'starting' || this.phase === 'recording';
    const d = this.#machine.keydown(e, { empty: t.value === '', recording });
    if (d.prevent) e.preventDefault();
    switch (d.action) {
      case 'warmup':
        // The space is typed as usual; if the key stays down, it is taken back at start.
        this.#warmupSnapshot = { value: t.value, start: t.selectionStart ?? 0, end: t.selectionEnd ?? 0 };
        this.phase = 'warmup';
        this.hint = false;
        break;
      case 'start':
        void this.start(this.#machine.mode);
        break;
      case 'stop':
        void this.stop();
        break;
      case 'abortWarmup':
        this.#warmupSnapshot = null;
        if (this.phase === 'warmup') this.phase = 'idle';
        break;
    }
  }

  #keyup(e: KeyboardEvent): void {
    if (!this.#machine) return;
    const d = this.#machine.keyup(e);
    if (d.action === 'stop' && this.#session?.mode === 'hold') {
      e.preventDefault();
      void this.stop();
    } else if (d.action === 'abortWarmup') {
      this.#warmupSnapshot = null;
      if (this.phase === 'warmup') this.phase = 'idle';
    }
  }

  // ------------------------------------------------------------------ mouse

  /** Mic button pressed: start at once; the release decides hold or tap. */
  pointerDown(): void {
    if (!this.enabled) return;
    if (this.phase === 'recording' || this.phase === 'starting') {
      // A tap recording: this press stops it (on release).
      this.#pressAt = -1;
      return;
    }
    if (this.phase !== 'idle') return;
    this.#pressAt = this.#now();
    void this.start('hold');
  }

  pointerUp(): void {
    if (this.#pressAt === -1) {
      this.#pressAt = 0;
      void this.stop();
      return;
    }
    if (!this.#pressAt) return;
    const held = this.#now() - this.#pressAt;
    this.#pressAt = 0;
    const s = this.#session;
    if (!s) return;
    if (held < HOLD_PRESS_MS) s.mode = 'tap';
    this.mode = s.mode;
    if (s.mode === 'hold') void this.stop();
  }

  /**
   * Enter / the send button while dictating: finish the dictation and send the prompt once
   * the final text is in (never the interim text, never twice). True when it took over the
   * send: the caller must not send now.
   */
  finishAndSubmit(): boolean {
    const s = this.#session;
    if (!s || !this.active) return false;
    s.submitWhenDone = true;
    if (this.phase === 'starting' || this.phase === 'recording') void this.stop();
    return true;
  }

  /** Keyboard activation of the mic button: toggle (tap mode). */
  toggle(): void {
    if (this.phase === 'recording' || this.phase === 'starting') void this.stop();
    else if (this.phase === 'idle') void this.start('tap');
  }

  // ------------------------------------------------------------------ recording

  #language(): string | undefined {
    const l = this.settings?.language ?? '';
    if (l === 'auto') return 'auto';
    return l || locale();
  }

  async start(mode: VoiceMode): Promise<void> {
    const t = this.#target;
    if (!t || this.#session || !this.settings?.enabled) return;
    const paused = this.#backoff.remaining(this.#now());
    if (paused > 0) {
      this.#showPause(paused);
      return;
    }
    // Hold Space: give back the space(s) typed during the warm-up.
    const snap = this.#warmupSnapshot ?? { value: t.value, start: t.selectionStart ?? t.value.length, end: t.selectionEnd ?? t.value.length };
    this.#warmupSnapshot = null;
    if (t.value !== snap.value) this.#setValue(snap.value, snap.start);
    const now = this.#now();
    const session: Session = {
      mode,
      snapshot: snap,
      stream: null,
      interim: false,
      mic: new MicCapture(),
      queue: [],
      pushing: null,
      pushTimer: undefined,
      startedAt: now,
      lastSpeechAt: now,
      stopRequested: false,
      submitWhenDone: false,
      region: { start: snap.start, length: 0, text: '' },
      partial: '',
    };
    // Replace any selection with the dictation.
    if (snap.end > snap.start) this.#setValue(snap.value.slice(0, snap.start) + snap.value.slice(snap.end), snap.start);
    this.#session = session;
    this.mode = mode;
    this.phase = 'starting';
    this.notice = null;
    this.hint = false;
    this.#countHint();
    const context = this.settings.page_hint ? this.#context?.()?.trim() || undefined : undefined;
    const opening = this.#api.call('voice.stream_start', { language: this.#language(), prompt: context });
    try {
      const [started] = await Promise.all([
        opening,
        session.mic!.start({
          onPcm: (pcm) => session.queue.push(pcm),
          onLevel: (level) => this.#onLevel(session, level),
        }),
      ]);
      if (this.#session !== session) {
        // Cancelled while starting.
        void this.#api.call('voice.stream_stop', { stream: started.stream, cancel: true }).catch(() => {});
        return;
      }
      session.stream = started.stream;
      session.interim = started.interim;
      this.interim = started.interim;
      session.startedAt = session.lastSpeechAt = this.#now();
      this.phase = 'recording';
      session.pushTimer = setInterval(() => void this.#flush(session), PUSH_EVERY_MS);
      if (session.stopRequested) await this.stop();
    } catch (err) {
      session.mic?.stop();
      // The microphone failed but the stream may open (or have opened) anyway: close it,
      // or it holds one of the browser's few stream slots (and a Deepgram socket).
      if (!session.stream) {
        void opening.then((st) => this.#api.call('voice.stream_stop', { stream: st.stream, cancel: true })).catch(() => {});
      }
      if (this.#session === session) {
        this.#session = null;
        this.phase = 'idle';
        this.level = 0;
      }
      this.#fail(err);
    }
  }

  #onLevel(session: Session, level: number): void {
    if (this.#session !== session) return;
    this.level = level;
    const now = this.#now();
    if (level >= SPEECH_LEVEL) session.lastSpeechAt = now;
    if (this.phase !== 'recording') return;
    const why = autoStopReason(now, session.startedAt, session.lastSpeechAt);
    if (why) {
      this.#flash({ kind: why });
      void this.stop();
    }
  }

  /** Send the audio collected since the last push; show the interim text. */
  #flush(session: Session): Promise<void> {
    if (session.pushing) return session.pushing;
    if (!session.stream || session.queue.length === 0) return Promise.resolve();
    const blocks = session.queue.splice(0);
    const total = blocks.reduce((n, b) => n + b.length, 0);
    const pcm = new Int16Array(total);
    let at = 0;
    for (const b of blocks) {
      pcm.set(b, at);
      at += b.length;
    }
    session.pushing = this.#api
      .call('voice.stream_push', { stream: session.stream, pcm_base64: bytesToBase64(int16ToBytes(pcm)) })
      .then((r) => {
        if (this.#session === session && session.interim && r.partial !== session.partial) {
          session.partial = r.partial;
          this.#showText(session, r.partial, false);
        }
      })
      .catch((err) => {
        if (this.#session === session) {
          this.#fail(err);
          this.cancel(false);
        }
      })
      .finally(() => {
        session.pushing = null;
      });
    return session.pushing;
  }

  /** Finish: the final text goes into the prompt; auto-submit when enabled. */
  async stop(): Promise<void> {
    const session = this.#session;
    if (this.phase === 'warmup') {
      this.phase = 'idle';
      return;
    }
    if (!session) return;
    if (this.phase === 'starting') {
      session.stopRequested = true;
      return;
    }
    if (this.phase !== 'recording') return;
    this.phase = 'transcribing';
    this.#machine?.reset();
    session.mic?.stop();
    session.mic = null;
    clearInterval(session.pushTimer);
    this.level = 0;
    try {
      await this.#flush(session);
      if (this.#session !== session) return;
      await this.#flush(session);
      const r = await this.#api.call('voice.stream_stop', { stream: session.stream!, cancel: false });
      if (this.#session !== session) return;
      this.#session = null;
      this.#showText(session, r.text, true);
      this.phase = 'idle';
      this.#backoff.succeed();
      if (!r.text.trim() && !session.submitWhenDone) this.#flash({ kind: 'nothing' });
      else if (session.submitWhenDone || (this.settings && shouldAutoSubmit(r.text, session.mode, this.settings))) {
        // After the input event reached the page's own state (bind:value). Once: the session
        // is over, so a later Enter is an ordinary send of whatever the prompt holds then.
        queueMicrotask(() => this.#onsubmit?.());
      }
    } catch (err) {
      if (this.#session === session) {
        this.#session = null;
        // Keep what was heard so far.
        if (session.partial) this.#showText(session, session.partial, true);
        this.phase = 'idle';
      }
      this.#fail(err);
    } finally {
      this.#overlay?.hide();
      this.#updateHint();
    }
  }

  /** Esc: stop the microphone, drop the transcript, restore the prompt as it was. */
  cancel(restore = true): void {
    const session = this.#session;
    this.#machine?.reset();
    this.#warmupSnapshot = null;
    if (!session) {
      if (this.phase === 'warmup') this.phase = 'idle';
      return;
    }
    this.#session = null;
    session.mic?.stop();
    clearInterval(session.pushTimer);
    if (session.stream) void this.#api.call('voice.stream_stop', { stream: session.stream, cancel: true }).catch(() => {});
    this.#overlay?.hide();
    if (restore) this.#setValue(session.snapshot.value, session.snapshot.start, session.snapshot.end);
    this.level = 0;
    this.phase = 'idle';
    this.#updateHint();
  }

  // ------------------------------------------------------------------ the prompt text

  #setValue(value: string, caret: number, caretEnd = caret): void {
    const t = this.#target;
    if (!t) return;
    t.value = value;
    try {
      t.setSelectionRange(caret, caretEnd);
    } catch {
      // input types without a selection
    }
    // bind:value and autosize listen to `input`.
    this.#ownInput = true;
    try {
      t.dispatchEvent(new Event('input', { bubbles: true }));
    } finally {
      this.#ownInput = false;
    }
  }

  /** Put `text` in the dictation region (interim: dimmed). */
  #showText(session: Session, text: string, final: boolean): void {
    const t = this.#target;
    if (!t) return;
    const r = session.region;
    const value = t.value;
    // The region moved or was edited by typing: find it again, else continue at the caret.
    if (value.slice(r.start, r.start + r.length) !== r.text) {
      const found = r.length ? value.indexOf(r.text, Math.max(0, r.start - 200)) : -1;
      if (found >= 0) r.start = found;
      else {
        r.start = t.selectionStart ?? value.length;
        r.length = 0;
        r.text = '';
      }
    }
    const before = value.slice(0, r.start);
    const after = value.slice(r.start + r.length);
    const caretInRegion = (t.selectionStart ?? 0) >= r.start && (t.selectionStart ?? 0) <= r.start + r.length;
    const j = joinDictation(before, text, after);
    const caret = caretInRegion || final ? j.caret : (t.selectionStart ?? j.caret);
    this.#setValue(j.value, caret);
    r.start = j.start;
    r.length = j.length;
    r.text = j.value.slice(j.start, j.start + j.length);
    if (final) this.#overlay?.hide();
    else this.#overlay?.show(r.start, r.length);
  }

  #userTyped(): void {
    const s = this.#session;
    if (!s) return;
    // Typing during dictation: the interim text becomes plain text where it is; dictation
    // continues at the cursor.
    const t = this.#target!;
    const r = s.region;
    if (t.value.slice(r.start, r.start + r.length) !== r.text) {
      r.start = t.selectionStart ?? t.value.length;
      r.length = 0;
      r.text = '';
      s.partial = '';
      this.#overlay?.hide();
    }
  }

  // ------------------------------------------------------------------ notices

  #fail(err: unknown): void {
    let notice: VoiceNotice;
    if (err instanceof MicCaptureError) notice = { kind: err.reason === 'failed' ? 'error' : err.reason, text: err.message } as VoiceNotice;
    else {
      const e = err as { code?: string; message?: string };
      notice = e?.code === 'NOT_FOUND' || /no API key|not downloaded|no Whisper model|no API address/.test(e?.message ?? '')
        ? { kind: 'setup', text: e.message }
        : { kind: 'error', text: e?.message ?? String(err) };
    }
    const now = this.#now();
    if (this.#backoff.fail(now)) this.#showPause(this.#backoff.remaining(now));
    else this.#flash(notice, 6000);
  }

  #showPause(ms: number): void {
    this.phase = 'paused';
    this.pausedFor = Math.ceil(ms / 1000);
    this.notice = { kind: 'paused' };
    clearInterval(this.#pauseTimer);
    this.#pauseTimer = setInterval(() => {
      const left = this.#backoff.remaining(this.#now());
      this.pausedFor = Math.ceil(left / 1000);
      if (left <= 0) {
        clearInterval(this.#pauseTimer);
        if (this.phase === 'paused') this.phase = 'idle';
        this.notice = null;
      }
    }, 250);
  }

  #flash(notice: VoiceNotice, ms = 4000): void {
    this.notice = notice;
    clearTimeout(this.#noticeTimer);
    this.#noticeTimer = setTimeout(() => {
      if (this.notice === notice) this.notice = null;
    }, ms);
  }

  dismissNotice(): void {
    this.notice = null;
  }

  // ------------------------------------------------------------------ first-run hint

  #updateHint(): void {
    const t = this.#target;
    const focused = !!t && typeof document !== 'undefined' && document.activeElement === t;
    const shown = load<number>('voice.hintShown', 0);
    this.hint = this.enabled && focused && this.phase === 'idle' && !!t && t.value === '' && shown < HINT_TIMES;
    if (this.hint && !this.#hintCounted) {
      this.#hintCounted = true;
      save('voice.hintShown', shown + 1);
    }
  }

  /** Using dictation once is enough to stop the hint. */
  #countHint(): void {
    save('voice.hintShown', HINT_TIMES);
  }
}
