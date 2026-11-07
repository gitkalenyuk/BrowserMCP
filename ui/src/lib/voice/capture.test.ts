// MicCapture against fake Web Audio objects (the tests run in Node): stop() while start() is
// still setting up must leave nothing running.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MicCapture } from './capture';

let loadModule: () => void;
let resumeDone: () => void;
let resumeHangs = false;

class FakeNode {
  port = { onmessage: null as unknown };
  gain = { value: 1 };
  fftSize = 0;
  connect(n: unknown) {
    return n;
  }
  disconnect() {}
  getFloatTimeDomainData() {}
}

class FakeAudioContext {
  sampleRate = 16_000;
  state = 'suspended';
  destination = new FakeNode();
  closed = false;
  audioWorklet = {
    addModule: () => new Promise<void>((r) => (loadModule = r)),
  };
  createMediaStreamSource = () => new FakeNode();
  createAnalyser = () => new FakeNode();
  createGain = () => new FakeNode();
  createScriptProcessor = () => new FakeNode();
  resume() {
    return resumeHangs ? new Promise<void>((r) => (resumeDone = r)) : Promise.resolve();
  }
  close() {
    this.closed = true;
    return Promise.resolve();
  }
}

const track = { stop: vi.fn() };

beforeEach(() => {
  vi.useFakeTimers();
  resumeHangs = false;
  vi.stubGlobal('AudioContext', FakeAudioContext);
  vi.stubGlobal('AudioWorkletNode', FakeNode);
  vi.stubGlobal('cancelAnimationFrame', () => {});
  vi.stubGlobal('navigator', {
    ...navigator,
    mediaDevices: { getUserMedia: () => Promise.resolve({ getTracks: () => [track] }) },
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('MicCapture.stop() during start()', () => {
  it('while the worklet module loads: no level timer is left behind', async () => {
    const mic = new MicCapture();
    const onLevel = vi.fn();
    const starting = mic.start({ onPcm: () => {}, onLevel });
    await vi.advanceTimersByTimeAsync(0);
    mic.stop();
    loadModule();
    await starting;
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(200);
    expect(onLevel).not.toHaveBeenCalled();
  });

  it('while the context resumes: the level timer is cleared', async () => {
    resumeHangs = true;
    const mic = new MicCapture();
    const starting = mic.start({ onPcm: () => {}, onLevel: () => {} });
    await vi.advanceTimersByTimeAsync(0);
    loadModule();
    await vi.advanceTimersByTimeAsync(0);
    mic.stop();
    resumeDone();
    await starting;
    expect(vi.getTimerCount()).toBe(0);
  });

  it('a normal start runs the level timer until stop()', async () => {
    const mic = new MicCapture();
    const onLevel = vi.fn();
    const starting = mic.start({ onPcm: () => {}, onLevel });
    await vi.advanceTimersByTimeAsync(0);
    loadModule();
    await starting;
    expect(vi.getTimerCount()).toBe(1);
    await vi.advanceTimersByTimeAsync(100);
    expect(onLevel).toHaveBeenCalled();
    mic.stop();
    expect(vi.getTimerCount()).toBe(0);
    expect(track.stop).toHaveBeenCalled();
  });
});
