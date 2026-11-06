import { RpcError, type BrowserApi } from '../api/client';
import type { NetPaused, NetResume, TabId } from '../api/types';
import { parsePausedLines } from './parse';

/** A held request: the full `net.paused` event when we saw it, else what net.paused_list tells. */
export interface PausedItem {
  info: NetPaused;
  /** ms since epoch when it auto-continues. */
  deadline: number;
  /** Only the list line was seen (opened before this chrome started): no headers/body. */
  partial: boolean;
}

/** The core continues a paused request after 60 s. */
const PAUSE_TIMEOUT_MS = 60_000;

/**
 * Traffic state that must outlive the panel: which tabs capture, and the requests held by
 * breakpoints (net.paused arrives whenever a page sends something).
 */
export class NetStore {
  readonly api: BrowserApi;
  /** tab -> capturing, as last reported by net.capture / net.list. */
  capturing = $state<Record<number, boolean>>({});
  paused = $state<PausedItem[]>([]);
  /** Last warning from the core (pause timeouts, lagged capture, intercept errors). */
  warning = $state<string | null>(null);
  #unsubs: Array<() => void> = [];

  constructor(api: BrowserApi) {
    this.api = api;
  }

  start(): void {
    this.#unsubs.push(
      this.api.on('net.paused', (p) => {
        this.paused = [
          ...this.paused.filter((x) => x.info.pause_id !== p.pause_id),
          { info: p, deadline: Date.now() + PAUSE_TIMEOUT_MS, partial: false },
        ];
      }),
      this.api.on('net.pause_timeout', (p) => {
        this.#remove(p.pause_id);
        this.warning = p.warning;
      }),
      this.api.on('net.lagged', (p) => (this.warning = p.warning)),
      this.api.on('net.intercept_error', (p) => (this.warning = p.warning)),
    );
  }

  stop(): void {
    this.#unsubs.forEach((u) => u());
    this.#unsubs = [];
  }

  async setCapture(tab: TabId, on: boolean): Promise<void> {
    const r = await this.api.call('net.capture', { tab, on });
    this.capturing[r.tab] = r.capturing;
  }

  /** Re-read held requests (the panel opens, or a pause may have been missed). */
  async refreshPaused(): Promise<void> {
    const { text } = await this.api.call('net.paused_list', {});
    const rows = parsePausedLines(text);
    const known = new Map(this.paused.map((p) => [p.info.pause_id, p]));
    this.paused = rows.map(
      (r) =>
        known.get(r.pause_id) ?? {
          info: {
            pause_id: r.pause_id,
            tab: r.tab,
            rule: r.rule,
            stage: r.stage,
            method: r.method,
            url: r.url,
            resource_type: 'other',
            headers: {},
            ...(r.status !== null ? { status: r.status } : {}),
          },
          deadline: Date.now() + r.secondsLeft * 1000,
          partial: true,
        },
    );
  }

  async resume(params: NetResume): Promise<string> {
    try {
      const r = await this.api.call('net.resume', params);
      this.#remove(params.pause_id);
      return r.text;
    } catch (err) {
      // Refused edits (INVALID_PARAMS) are checked before the core takes the pause: it is
      // still held and can be resumed again with corrected edits.
      if (!(err instanceof RpcError && err.code === 'INVALID_PARAMS')) this.#remove(params.pause_id);
      throw err;
    }
  }

  #remove(id: string): void {
    this.paused = this.paused.filter((p) => p.info.pause_id !== id);
  }
}
