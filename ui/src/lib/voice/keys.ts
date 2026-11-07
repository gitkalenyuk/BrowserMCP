// The dictation hotkey: parsing `Space` / `Ctrl+Space` / `Alt+V` / `F8`, matching key events
// by physical key (event.code, so a Ukrainian layout still matches Alt+V), and the hold / tap
// state machine with the Space warm-up. Pure: unit-tested in voice.test.ts.

import type { VoiceMode } from './text';

export interface Hotkey {
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
  /** `Space`, a letter `A`–`Z`, a digit, `F1`–`F24` or `Backquote`. */
  key: string;
}

export interface KeyEventLike {
  code: string;
  key?: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
  repeat: boolean;
}

const MODS: Record<string, keyof Omit<Hotkey, 'key'>> = {
  ctrl: 'ctrl',
  control: 'ctrl',
  alt: 'alt',
  option: 'alt',
  shift: 'shift',
  meta: 'meta',
  cmd: 'meta',
  win: 'meta',
};

/** `ctrl + space` → {ctrl, key: 'Space'}; null when it is not a valid hotkey. */
export function parseHotkey(text: string): Hotkey | null {
  const parts = text
    .split('+')
    .map((p) => p.trim())
    .filter(Boolean);
  const last = parts.pop();
  if (!last) return null;
  const h: Hotkey = { ctrl: false, alt: false, shift: false, meta: false, key: '' };
  for (const m of parts) {
    const mod = MODS[m.toLowerCase()];
    if (!mod) return null;
    h[mod] = true;
  }
  const k = last.toLowerCase();
  if (k === 'space') h.key = 'Space';
  else if (k === 'backquote' || k === '`') h.key = 'Backquote';
  else if (/^[a-z0-9]$/.test(k)) h.key = k.toUpperCase();
  else if (/^f([1-9]|1\d|2[0-4])$/.test(k)) h.key = k.toUpperCase();
  else return null;
  return h;
}

export function formatHotkey(h: Hotkey): string {
  return [h.ctrl && 'Ctrl', h.alt && 'Alt', h.shift && 'Shift', h.meta && 'Meta', h.key].filter(Boolean).join('+');
}

/** No modifiers (plain Space or a function key). */
export function isPlain(h: Hotkey): boolean {
  return !h.ctrl && !h.alt && !h.shift && !h.meta;
}

/** The `KeyboardEvent.code` of the hotkey's main key. */
export function codeOf(key: string): string {
  if (key === 'Space' || key === 'Backquote' || /^F\d+$/.test(key)) return key;
  if (/^[A-Z]$/.test(key)) return `Key${key}`;
  if (/^\d$/.test(key)) return `Digit${key}`;
  return key;
}

/** Does the event press exactly this combination? */
export function matchesKey(e: KeyEventLike, h: Hotkey): boolean {
  return e.code === codeOf(h.key) && e.ctrlKey === h.ctrl && e.altKey === h.alt && e.shiftKey === h.shift && e.metaKey === h.meta;
}

/** A hotkey typed in Settings by pressing it; null while only modifiers are down. */
export function hotkeyFromEvent(e: KeyEventLike): string | null {
  let key: string | null = null;
  if (e.code === 'Space' || e.code === 'Backquote' || /^F\d+$/.test(e.code)) key = e.code;
  else if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  else if (/^Digit\d$/.test(e.code)) key = e.code.slice(5);
  if (!key) return null;
  const h: Hotkey = { ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey, meta: e.metaKey, key };
  // A plain letter would steal typing.
  if (isPlain(h) && key !== 'Space' && !/^F\d+$/.test(key)) return null;
  return formatHotkey(h);
}

/** What the controller should do with a key event. */
export interface Decision {
  /** Call preventDefault (no character typed). */
  prevent: boolean;
  action: 'none' | 'warmup' | 'start' | 'stop' | 'abortWarmup';
}

const NONE: Decision = { prevent: false, action: 'none' };

/**
 * Hold mode: hold the hotkey to record, release to finish. Plain Space needs a warm-up: the
 * first press types a space as usual ("keep holding…"); only key repeats (the key is still
 * down) start recording, so a single tap of Space still types a space. Space works only while
 * the prompt is empty; combos (Ctrl+Space, Alt+V) and function keys start at once anywhere.
 *
 * Tap mode: press to start (Space: on an empty prompt), press again to stop.
 *
 * A reset while the hotkey is still physically down (an auto-stop, Esc) swallows the key's
 * repeats until it is released, so a held Space does not start typing spaces.
 */
export class HotkeyMachine {
  state: 'idle' | 'warmup' | 'active' = 'idle';
  #repeats = 0;
  /** The hotkey's main key is down (pressed and not yet released). */
  #down = false;
  /** Swallow the repeats of the current press (it ended a recording). */
  #swallow = false;

  constructor(
    public hotkey: Hotkey,
    public mode: VoiceMode,
    /** Key repeats needed before plain Space starts recording. */
    public warmupRepeats = 2,
  ) {}

  get spaceWarmup(): boolean {
    return this.hotkey.key === 'Space' && isPlain(this.hotkey);
  }

  reset(): void {
    this.#swallow = this.#down && this.state !== 'idle';
    this.state = 'idle';
    this.#repeats = 0;
  }

  keydown(e: KeyEventLike, ctx: { empty: boolean; recording: boolean }): Decision {
    if (e.code === codeOf(this.hotkey.key)) {
      if (!e.repeat) this.#swallow = false;
      else if (this.#swallow) return { prevent: true, action: 'none' };
    }
    if (matchesKey(e, this.hotkey)) this.#down = true;
    if (!matchesKey(e, this.hotkey)) {
      // Typing something else during the warm-up: it was just a space.
      if (this.state === 'warmup') {
        this.reset();
        return { prevent: false, action: 'abortWarmup' };
      }
      return NONE;
    }
    if (this.mode === 'tap') {
      if (e.repeat) return { prevent: this.state === 'active' || ctx.recording, action: 'none' };
      if (ctx.recording) {
        this.reset();
        return { prevent: true, action: 'stop' };
      }
      if (this.spaceWarmup && !ctx.empty) return NONE;
      this.state = 'active';
      return { prevent: true, action: 'start' };
    }
    // Hold mode.
    if (this.spaceWarmup) {
      if (this.state === 'idle') {
        if (e.repeat || !ctx.empty || ctx.recording) return NONE;
        this.state = 'warmup';
        this.#repeats = 0;
        return { prevent: false, action: 'warmup' };
      }
      if (this.state === 'warmup') {
        if (!e.repeat) return NONE;
        this.#repeats += 1;
        if (this.#repeats >= this.warmupRepeats) {
          this.state = 'active';
          return { prevent: true, action: 'start' };
        }
        return { prevent: true, action: 'none' };
      }
      return { prevent: true, action: 'none' };
    }
    if (e.repeat) return { prevent: true, action: 'none' };
    if (ctx.recording && this.state === 'idle') return { prevent: true, action: 'stop' };
    this.state = 'active';
    return { prevent: true, action: 'start' };
  }

  keyup(e: KeyEventLike): Decision {
    if (e.code === codeOf(this.hotkey.key)) {
      this.#down = false;
      this.#swallow = false;
    }
    if (this.mode === 'tap' || this.state === 'idle') return NONE;
    const h = this.hotkey;
    const released =
      e.code === codeOf(h.key) ||
      (h.ctrl && /^Control/.test(e.code)) ||
      (h.alt && /^Alt/.test(e.code)) ||
      (h.shift && /^Shift/.test(e.code)) ||
      (h.meta && /^(Meta|OS)/.test(e.code));
    if (!released) return NONE;
    const was = this.state;
    this.reset();
    return was === 'warmup' ? { prevent: false, action: 'abortWarmup' } : { prevent: true, action: 'stop' };
  }
}
