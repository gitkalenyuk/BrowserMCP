import type { BrowserApi } from '../api/client';
import type { RecordStep, RecordingEntry } from '../api/types-record-runs';
import { parseStepsJsonl, upsertStep } from './logic';

export interface ActiveRecording {
  id: string;
  tab: number;
  name: string;
  startUrl: string;
  startedMs: number;
  steps: RecordStep[];
  video: string | null;
}

/**
 * Recording state that outlives the panel: the running recording (filled from record.* events,
 * also for recordings started over MCP or REST) and the list of saved ones. One per API,
 * created the first time the panel opens (so the shared ChromeController stays untouched).
 */
export class RecordStore {
  readonly api: BrowserApi;
  recordings = $state<RecordingEntry[]>([]);
  active = $state<ActiveRecording | null>(null);
  error = $state<string | null>(null);
  #unsubs: Array<() => void> = [];

  constructor(api: BrowserApi) {
    this.api = api;
    this.#unsubs.push(
      api.on('record.started', (e) => {
        if (this.active?.id !== e.id) {
          this.active = { id: e.id, tab: e.tab, name: e.name, startUrl: e.start_url, startedMs: Date.now(), steps: [], video: null };
        }
        // Steps written before this event (the start step) come from the log.
        void this.#mergeLog(e.id);
        void this.refresh();
      }),
      api.on('record.step', ({ id, step }) => {
        if (this.active?.id !== id) {
          // Started before we listened (or by another client): pick it up from the list.
          void this.refresh();
          return;
        }
        this.active.steps = upsertStep(this.active.steps, step);
      }),
      api.on('record.stopped', ({ id }) => {
        if (this.active?.id === id) this.active = null;
        void this.refresh();
      }),
      api.onStatus((s) => {
        if (s === 'open') void this.refresh();
      }),
    );
  }

  stop(): void {
    this.#unsubs.forEach((u) => u());
    this.#unsubs = [];
  }

  #refreshing: Promise<void> | null = null;

  /** record.list (+ the running recording's log when it is new to us). Coalesces calls. */
  refresh(): Promise<void> {
    this.#refreshing ??= this.#refresh().finally(() => (this.#refreshing = null));
    return this.#refreshing;
  }

  async #mergeLog(id: string): Promise<void> {
    try {
      const detail = await this.api.call('record.get', { id, format: 'jsonl' });
      if (this.active?.id !== id) return;
      let steps = this.active.steps;
      for (const s of parseStepsJsonl(detail.text)) if (!steps.some((x) => x.seq === s.seq)) steps = upsertStep(steps, s);
      this.active.steps = steps;
      this.active.video ??= detail.meta.video ?? null;
    } catch {
      // the next event or refresh fills it in
    }
  }

  async #refresh(): Promise<void> {
    try {
      const { recordings } = await this.api.call('record.list');
      this.recordings = recordings;
      const running = recordings.find((r) => r.active);
      if (!running) {
        this.active = null;
      } else if (this.active?.id !== running.id) {
        const detail = await this.api.call('record.get', { id: running.id, format: 'jsonl' });
        this.active = {
          id: running.id,
          tab: detail.meta.tab,
          name: running.name,
          startUrl: running.start_url,
          startedMs: running.started_ms,
          steps: parseStepsJsonl(detail.text),
          video: detail.meta.video ?? null,
        };
      }
      this.error = null;
    } catch (err) {
      this.error = (err as Error).message;
    }
  }

  async start(opts: { tab: number; name: string; video: boolean; traffic: boolean }): Promise<void> {
    const r = await this.api.call('record.start', { tab: opts.tab, name: opts.name || undefined, video: opts.video, traffic: opts.traffic });
    // record.started usually arrives first; make sure we show it either way.
    if (this.active?.id !== r.id) {
      this.active = { id: r.id, tab: r.tab, name: opts.name || 'recording', startUrl: '', startedMs: Date.now(), steps: [], video: r.video ?? null };
    } else {
      this.active.video = r.video ?? null;
    }
    void this.refresh();
  }

  async stopRecording(): Promise<string | null> {
    const id = this.active?.id;
    const r = await this.api.call('record.stop', id ? { id } : {});
    this.active = null;
    await this.refresh();
    return r.id;
  }
}

const stores = new WeakMap<BrowserApi, RecordStore>();

export function recordStore(api: BrowserApi): RecordStore {
  let s = stores.get(api);
  if (!s) stores.set(api, (s = new RecordStore(api)));
  return s;
}
