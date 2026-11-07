// Microphone capture for voice dictation: getUserMedia → AudioContext (16 kHz when the
// browser resamples for us) → AudioWorklet (ScriptProcessor as a fallback) → 16-bit PCM
// blocks, plus an AnalyserNode for the level meter.
//
// The browser grants the microphone to our own bmcp://ui pages without a prompt
// (crates/cef scheme.rs `allow_ui_media`); websites keep the normal permission bar.

import { SAMPLE_RATE, downsample, floatTo16, levelOf } from './wav';
// A file of its own (never a data: URL): the CSP of our pages runs scripts from 'self' only.
import workletUrl from './pcm-worklet.js?url&no-inline';

export interface CaptureHandlers {
  /** ~100 ms of 16 kHz mono 16-bit samples. */
  onPcm: (pcm: Int16Array) => void;
  /** Microphone level 0..1, about 30 times a second. */
  onLevel?: (level: number) => void;
}

export class MicCaptureError extends Error {
  constructor(
    readonly reason: 'unsupported' | 'denied' | 'no-device' | 'failed',
    message: string,
  ) {
    super(message);
    this.name = 'MicCaptureError';
  }
}

export class MicCapture {
  #stream: MediaStream | null = null;
  #ctx: AudioContext | null = null;
  #nodes: AudioNode[] = [];
  #raf = 0;
  #timer: ReturnType<typeof setInterval> | undefined;
  #stopped = false;

  static supported(): boolean {
    return typeof navigator !== 'undefined' && !!navigator.mediaDevices?.getUserMedia && typeof AudioContext !== 'undefined';
  }

  async start(h: CaptureHandlers): Promise<void> {
    if (!MicCapture.supported()) throw new MicCaptureError('unsupported', 'This page cannot use the microphone.');
    try {
      this.#stream = await navigator.mediaDevices.getUserMedia({
        audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true },
      });
    } catch (err) {
      const name = (err as DOMException)?.name;
      if (name === 'NotAllowedError' || name === 'SecurityError') throw new MicCaptureError('denied', 'Microphone access was denied.');
      if (name === 'NotFoundError' || name === 'OverconstrainedError') throw new MicCaptureError('no-device', 'No microphone was found.');
      throw new MicCaptureError('failed', (err as Error)?.message ?? String(err));
    }
    if (this.#stopped) return this.stop();
    let ctx: AudioContext;
    try {
      ctx = new AudioContext({ sampleRate: SAMPLE_RATE });
    } catch {
      ctx = new AudioContext();
    }
    this.#ctx = ctx;
    const rate = ctx.sampleRate;
    const source = ctx.createMediaStreamSource(this.#stream);
    const analyser = ctx.createAnalyser();
    analyser.fftSize = 1024;
    source.connect(analyser);
    this.#nodes.push(source, analyser);
    const emit = (block: Float32Array) => {
      if (this.#stopped) return;
      h.onPcm(floatTo16(rate === SAMPLE_RATE ? block : downsample(block, rate, SAMPLE_RATE)));
    };
    try {
      await ctx.audioWorklet.addModule(workletUrl);
      // stop() while the module loaded: it has closed the context already.
      if (this.#stopped) return this.stop();
      const node = new AudioWorkletNode(ctx, 'bmcp-pcm');
      node.port.onmessage = (e: MessageEvent<Float32Array>) => emit(e.data);
      source.connect(node);
      // The worklet must be connected to run; a muted gain keeps it silent.
      const mute = ctx.createGain();
      mute.gain.value = 0;
      node.connect(mute).connect(ctx.destination);
      this.#nodes.push(node, mute);
    } catch {
      if (this.#stopped) return this.stop();
      const proc = ctx.createScriptProcessor(4096, 1, 1);
      proc.onaudioprocess = (e) => emit(new Float32Array(e.inputBuffer.getChannelData(0)));
      source.connect(proc);
      proc.connect(ctx.destination);
      this.#nodes.push(proc);
    }
    if (h.onLevel && !this.#stopped) {
      const buf = new Float32Array(analyser.fftSize);
      const tick = () => {
        if (this.#stopped) return;
        analyser.getFloatTimeDomainData(buf);
        h.onLevel?.(levelOf(buf));
      };
      // An interval, not requestAnimationFrame: background tabs and hidden panels still need
      // the silence auto-stop.
      this.#timer = setInterval(tick, 33);
    }
    if (ctx.state === 'suspended') await ctx.resume().catch(() => {});
    // stop() during resume(): clear what was set up after it ran (the level timer).
    if (this.#stopped) this.stop();
  }

  stop(): void {
    this.#stopped = true;
    clearInterval(this.#timer);
    cancelAnimationFrame(this.#raf);
    for (const n of this.#nodes) {
      try {
        n.disconnect();
      } catch {
        // already disconnected
      }
    }
    this.#nodes = [];
    this.#stream?.getTracks().forEach((t) => t.stop());
    this.#stream = null;
    void this.#ctx?.close().catch(() => {});
    this.#ctx = null;
  }
}

/** Record `ms` milliseconds and return all samples (Settings → Voice → Test). */
export async function recordFor(ms: number, onLevel?: (level: number) => void): Promise<Int16Array> {
  const mic = new MicCapture();
  const blocks: Int16Array[] = [];
  await mic.start({ onPcm: (pcm) => blocks.push(pcm), onLevel });
  await new Promise((r) => setTimeout(r, ms));
  mic.stop();
  const total = blocks.reduce((n, b) => n + b.length, 0);
  const out = new Int16Array(total);
  let at = 0;
  for (const b of blocks) {
    out.set(b, at);
    at += b.length;
  }
  return out;
}
