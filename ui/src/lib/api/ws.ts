import { Emitter, RpcError, type BrowserApi } from './client';
import type { ApiEvents, CallArgs, ConnectionStatus, EventName, Method, Result } from './types';

export const DEFAULT_WS_URL = 'ws://127.0.0.1:47800/v1/ws';

/**
 * Topics asked for on every connection, on top of the server's defaults (tab.*, tabs.*, …):
 * the side panels render AI sessions, terminals and traffic; ui.* carries notifications.
 * Subscribing twice is harmless.
 */
export const EXTRA_TOPICS = ['ai.*', 'term.*', 'net.*', 'record.*', 'run.*', 'trigger.*', 'ui.*', 'script.*', 'permission.*'];

interface Pending {
  resolve: (value: unknown) => void;
  reject: (err: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

interface RpcResponse {
  jsonrpc: '2.0';
  id?: number | string | null;
  result?: unknown;
  error?: { code: number | string; message: string; data?: unknown };
  /** Present on notifications (events): the event name, e.g. "tab.updated". */
  method?: string;
  params?: unknown;
}

export interface WsApiOptions {
  url?: string;
  /** Per-call timeout in ms. */
  timeout?: number;
  /** Reconnect backoff bounds in ms. */
  minBackoff?: number;
  maxBackoff?: number;
  /** Extra event topics to subscribe to after each (re)connect (default EXTRA_TOPICS). */
  topics?: string[];
}

/**
 * JSON-RPC 2.0 over WebSocket.
 * - Requests: {jsonrpc, id, method, params}
 * - Responses: {jsonrpc, id, result} | {jsonrpc, id, error:{code,message,data}}
 * - Events: notifications without id, {jsonrpc, method: "tab.updated", params: {...}}
 * Calls made while disconnected wait for the next successful connection (up to `timeout`).
 * Reconnects forever with exponential backoff; in-flight calls are rejected on disconnect.
 */
export class WsApi implements BrowserApi {
  readonly kind = 'ws' as const;
  #url: string;
  #timeout: number;
  #minBackoff: number;
  #maxBackoff: number;
  #ws: WebSocket | null = null;
  #nextId = 1;
  #pending = new Map<number, Pending>();
  #events = new Emitter();
  #statusHandlers = new Set<(s: ConnectionStatus) => void>();
  #status: ConnectionStatus = 'closed';
  #backoff: number;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #closedByUser = false;
  #openWaiters: Array<() => void> = [];
  #topics: string[];

  constructor(opts: WsApiOptions = {}) {
    this.#url = opts.url ?? DEFAULT_WS_URL;
    this.#timeout = opts.timeout ?? 15_000;
    this.#minBackoff = opts.minBackoff ?? 250;
    this.#maxBackoff = opts.maxBackoff ?? 5_000;
    this.#backoff = this.#minBackoff;
    this.#topics = opts.topics ?? EXTRA_TOPICS;
  }

  get status(): ConnectionStatus {
    return this.#status;
  }

  connect(): Promise<void> {
    this.#closedByUser = false;
    if (this.#status === 'open') return Promise.resolve();
    const opened = this.#waitOpen();
    if (!this.#ws) this.#open();
    return opened;
  }

  close(): void {
    this.#closedByUser = true;
    if (this.#reconnectTimer) clearTimeout(this.#reconnectTimer);
    this.#reconnectTimer = null;
    this.#ws?.close(1000, 'client closed');
    this.#ws = null;
    this.#setStatus('closed');
    this.#failPending(new RpcError('DISCONNECTED', 'Connection closed'));
  }

  async call<M extends Method>(method: M, ...args: CallArgs<M>): Promise<Result<M>> {
    if (this.#status !== 'open') {
      await Promise.race([
        this.connect(),
        new Promise<never>((_, reject) =>
          setTimeout(() => reject(new RpcError('TIMEOUT', `Not connected: ${method}`)), this.#timeout),
        ),
      ]);
    }
    const ws = this.#ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) throw new RpcError('DISCONNECTED', 'Not connected');
    const id = this.#nextId++;
    const msg: Record<string, unknown> = { jsonrpc: '2.0', id, method };
    if (args.length > 0) msg.params = args[0];
    return new Promise<Result<M>>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new RpcError('TIMEOUT', `${method} timed out after ${this.#timeout} ms`));
      }, this.#timeout);
      this.#pending.set(id, { resolve: resolve as (v: unknown) => void, reject, timer });
      ws.send(JSON.stringify(msg));
    });
  }

  on<E extends EventName>(event: E, handler: (payload: ApiEvents[E]) => void): () => void {
    return this.#events.on(event, handler);
  }

  onStatus(handler: (status: ConnectionStatus) => void): () => void {
    this.#statusHandlers.add(handler);
    return () => this.#statusHandlers.delete(handler);
  }

  #open(): void {
    this.#setStatus('connecting');
    let ws: WebSocket;
    try {
      ws = new WebSocket(this.#url);
    } catch (err) {
      console.warn('[api] WebSocket failed to start', err);
      this.#scheduleReconnect();
      return;
    }
    this.#ws = ws;
    ws.onopen = () => {
      this.#backoff = this.#minBackoff;
      if (this.#topics.length > 0) {
        // A notification (no id): the server applies it before any later request, no answer.
        ws.send(JSON.stringify({ jsonrpc: '2.0', method: 'events.subscribe', params: { topics: this.#topics } }));
      }
      this.#setStatus('open');
      const waiters = this.#openWaiters;
      this.#openWaiters = [];
      waiters.forEach((w) => w());
    };
    ws.onmessage = (ev) => this.#onMessage(ev.data);
    ws.onclose = () => {
      if (this.#ws === ws) this.#ws = null;
      this.#failPending(new RpcError('DISCONNECTED', 'Connection lost'));
      if (this.#closedByUser) return;
      this.#setStatus('connecting');
      this.#scheduleReconnect();
    };
    ws.onerror = () => {
      // onclose follows and handles reconnection.
    };
  }

  #onMessage(data: unknown): void {
    if (typeof data !== 'string') return;
    let msg: RpcResponse;
    try {
      msg = JSON.parse(data) as RpcResponse;
    } catch {
      console.warn('[api] dropping non-JSON frame');
      return;
    }
    if (msg.id !== undefined && msg.id !== null) {
      const pending = this.#pending.get(Number(msg.id));
      if (!pending) return;
      this.#pending.delete(Number(msg.id));
      clearTimeout(pending.timer);
      if (msg.error) {
        // The core sends a numeric JSON-RPC code (-32000…) and the real error code
        // ("TAB_NOT_FOUND", …, same as the mock's) in error.data.code.
        const data = msg.error.data as { code?: unknown } | undefined;
        const code = typeof data?.code === 'string' ? data.code : String(msg.error.code);
        pending.reject(new RpcError(code, msg.error.message, msg.error.data));
      } else {
        pending.resolve(msg.result);
      }
      return;
    }
    if (msg.method) this.#events.emitRaw(msg.method, msg.params);
  }

  #scheduleReconnect(): void {
    if (this.#reconnectTimer || this.#closedByUser) return;
    const delay = this.#backoff;
    this.#backoff = Math.min(this.#backoff * 2, this.#maxBackoff);
    this.#reconnectTimer = setTimeout(() => {
      this.#reconnectTimer = null;
      this.#open();
    }, delay);
  }

  #waitOpen(): Promise<void> {
    return new Promise((resolve) => this.#openWaiters.push(resolve));
  }

  #failPending(err: Error): void {
    for (const p of this.#pending.values()) {
      clearTimeout(p.timer);
      p.reject(err);
    }
    this.#pending.clear();
  }

  #setStatus(status: ConnectionStatus): void {
    if (this.#status === status) return;
    this.#status = status;
    this.#statusHandlers.forEach((h) => h(status));
  }
}
