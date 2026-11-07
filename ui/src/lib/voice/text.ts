// Pure text rules of voice dictation: word counting for auto-submit, where dictated text goes
// in the prompt, and the failure backoff. No DOM, no API: unit-tested in voice.test.ts.

export type VoiceMode = 'hold' | 'tap';

/** Auto-submit needs at least this many words (like Claude Code's /voice). */
export const AUTO_SUBMIT_MIN_WORDS = 3;

/** Words with at least one letter or digit ("—" and "..." are not words). */
export function wordCount(text: string): number {
  return text
    .trim()
    .split(/\s+/)
    .filter((w) => /[\p{L}\p{N}]/u.test(w)).length;
}

/** Send the prompt after dictation? Only with the option on for that mode and ≥3 words. */
export function shouldAutoSubmit(
  text: string,
  mode: VoiceMode,
  settings: { auto_submit_hold: boolean; auto_submit_tap: boolean },
): boolean {
  const on = mode === 'hold' ? settings.auto_submit_hold : settings.auto_submit_tap;
  return on && wordCount(text) >= AUTO_SUBMIT_MIN_WORDS;
}

/**
 * The prompt with dictated `text` between `before` and `after`, with a space where words
 * would otherwise touch. `caret` = position right after the dictated text.
 */
export function joinDictation(before: string, text: string, after: string): { value: string; caret: number; start: number; length: number } {
  const t = text.trim();
  if (!t) return { value: before + after, caret: before.length, start: before.length, length: 0 };
  const lead = before && !/\s$/.test(before) ? ' ' : '';
  const trail = after && !/^[\s.,!?;:)\]}»"']/.test(after) ? ' ' : '';
  const start = before.length + lead.length;
  const value = before + lead + t + trail + after;
  return { value, caret: start + t.length, start, length: t.length };
}

/**
 * Three failures within 10 s pause dictation for 10 s (a broken key or a dead server should
 * not be hammered with every key press).
 */
export class FailureBackoff {
  readonly max: number;
  readonly windowMs: number;
  readonly pauseMs: number;
  #failures: number[] = [];
  #pausedUntil = 0;

  constructor(opts: { max?: number; windowMs?: number; pauseMs?: number } = {}) {
    this.max = opts.max ?? 3;
    this.windowMs = opts.windowMs ?? 10_000;
    this.pauseMs = opts.pauseMs ?? 10_000;
  }

  /** Record a failure at `now`; true when dictation is now paused. */
  fail(now: number): boolean {
    this.#failures = this.#failures.filter((t) => now - t < this.windowMs);
    this.#failures.push(now);
    if (this.#failures.length >= this.max) {
      this.#pausedUntil = now + this.pauseMs;
      this.#failures = [];
      return true;
    }
    return false;
  }

  /** A success clears the count. */
  succeed(): void {
    this.#failures = [];
  }

  /** Milliseconds left in the pause (0 = not paused). */
  remaining(now: number): number {
    return Math.max(0, this.#pausedUntil - now);
  }
}

/** Auto-stop: 15 s without speech, or 2 minutes in all. */
export const SILENCE_STOP_MS = 15_000;
export const MAX_RECORDING_MS = 120_000;
/**
 * Level (0..1, see wav.ts `levelOf`) above which the microphone counts as hearing speech: a
 * quiet room is about 0.05–0.15 (RMS 0.001–0.005), speech 0.4–0.9 (RMS 0.04–0.2).
 */
export const SPEECH_LEVEL = 0.22;

/** Why a recording should stop now, if it should. */
export function autoStopReason(now: number, startedAt: number, lastSpeechAt: number): 'silence' | 'limit' | null {
  if (now - startedAt >= MAX_RECORDING_MS) return 'limit';
  if (now - lastSpeechAt >= SILENCE_STOP_MS) return 'silence';
  return null;
}
