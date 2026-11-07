// AudioWorklet for voice dictation: collects the microphone's mono samples into ~100 ms
// blocks and posts them to the page (capture.ts turns them into 16 kHz PCM).
/* global sampleRate, registerProcessor, AudioWorkletProcessor */
class BmcpPcm extends AudioWorkletProcessor {
  constructor() {
    super();
    this.size = Math.max(128, Math.round(sampleRate / 10));
    this.buf = new Float32Array(this.size);
    this.fill = 0;
  }

  process(inputs) {
    const ch = inputs[0] && inputs[0][0];
    if (ch) {
      let i = 0;
      while (i < ch.length) {
        const n = Math.min(ch.length - i, this.size - this.fill);
        this.buf.set(ch.subarray(i, i + n), this.fill);
        this.fill += n;
        i += n;
        if (this.fill === this.size) {
          this.port.postMessage(this.buf.slice(0));
          this.fill = 0;
        }
      }
    }
    return true;
  }
}

registerProcessor('bmcp-pcm', BmcpPcm);
