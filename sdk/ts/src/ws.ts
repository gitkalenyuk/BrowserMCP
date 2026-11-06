// JSON-RPC 2.0 over WebSocket, the protocol of /v1/ws (see crates/api/README.md):
//
//   -> {"jsonrpc":"2.0","id":1,"method":"tabs.list","params":{}}
//   <- {"jsonrpc":"2.0","id":1,"result":{"tabs":[…]}}
//   <- {"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"…","data":{CmdError}}}
//   <- {"jsonrpc":"2.0","method":"command.finished","params":{…,"seq":17}}   (event)
//
// Uses the global `WebSocket` (browsers, Node 22+), so the SDK has no runtime dependencies.

import { DEFAULT_TOPICS, topicMatches, type CallOptions, type Transport } from "@browsermcp/protocol";
import { BrowserError, isCmdError } from "./errors.js";

export const DEFAULT_WS_URL = "ws://127.0.0.1:47800/v1/ws";

export interface WsOptions {
  /** Default `ws://127.0.0.1:47800/v1/ws` (`--api-port` changes the port). */
  url?: string;
  /** Default per-call timeout in ms (default 60 000); override per call with `{ timeout }`. */
  timeout?: number;
  /** Reconnect after the connection drops (default true). The first connect never retries. */
  reconnect?: boolean;
  /** Reconnect delay: starts at `minBackoff` ms and doubles up to `maxBackoff` (250 / 5000). */
  minBackoff?: number;
  maxBackoff?: number;
  /**
   * Give up on a connection attempt that is not open after this many ms (default 10 000).
   * Some WebSocket implementations never report a refused or dropped handshake.
   */
  connectTimeout?: number;
  /** WebSocket class to use instead of the global one (e.g. the `ws` package on old Node). */
  WebSocket?: typeof WebSocket;
}

/** Receives the event payload and the concrete topic (useful with patterns like `tab.*`). */
export type EventHandler<T = unknown> = (data: T, topic: string) => void;

interface Pending {
  resolve: (value: unknown) => void;
  reject: (err: BrowserError) => void;
  timer: ReturnType<typeof setTimeout>;
  /** False while it waits for a (re)connect; only sent calls fail when the connection drops. */
  sent: boolean;
}

interface RpcMessage {
  id?: number | string | null;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
  method?: string;
  params?: unknown;
}

export class WsTransport implements Transport {
  readonly url: string;
  readonly #timeout: number;
  readonly #reconnect: boolean;
  readonly #minBackoff: number;
  readonly #maxBackoff: number;
  readonly #connectTimeout: number;
  readonly #WebSocket: typeof WebSocket;

  #ws: WebSocket | null = null;
  #open = false;
  /** Set after the first successful open: from then on a dropped connection is retried. */
  #everOpened = false;
  #closed = false;
  #backoff: number;
  #reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  #nextId = 1;
  #pending = new Map<number, Pending>();
  /** Calls made while (re)connecting wait here. */
  #openWaiters: Array<() => void> = [];
  /** Local event handlers by topic pattern. */
  #handlers = new Map<string, Set<EventHandler>>();
  /** Patterns this client asked the server for (sent again after a reconnect). */
  #subscribed = new Set<string>();

  constructor(options: WsOptions = {}) {
    this.url = options.url ?? DEFAULT_WS_URL;
    this.#timeout = options.timeout ?? 60_000;
    this.#reconnect = options.reconnect ?? true;
    this.#minBackoff = options.minBackoff ?? 250;
    this.#maxBackoff = options.maxBackoff ?? 5_000;
    this.#connectTimeout = options.connectTimeout ?? 10_000;
    this.#backoff = this.#minBackoff;
    const Ws = options.WebSocket ?? globalThis.WebSocket;
    if (!Ws) throw new Error("no global WebSocket: use Node 22+ or pass options.WebSocket");
    this.#WebSocket = Ws;
  }

  /** Open the connection. Rejects with `CONNECTION_FAILED` if the browser cannot be reached. */
  connect(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.#openSocket(
        () => resolve(),
        () => reject(new BrowserError({ code: "CONNECTION_FAILED", message: `cannot connect to ${this.url}`, retry: "after_wait", hint: "Is BrowserMCP running? Check the port (--api-port) and <data>/runtime/endpoint.json." })),
      );
    });
  }

  /** Close for good: pending calls fail with `CONNECTION_CLOSED`, no reconnect. */
  close(): void {
    this.#closed = true;
    if (this.#reconnectTimer) clearTimeout(this.#reconnectTimer);
    this.#ws?.close(1000, "client closed");
    this.#failPending("CONNECTION_CLOSED", "the connection was closed by the client");
  }

  call(method: string, params: object, options: CallOptions = {}): Promise<unknown> {
    if (this.#closed) {
      return Promise.reject(new BrowserError({ code: "CONNECTION_CLOSED", message: "the connection is closed" }));
    }
    const id = this.#nextId++;
    const timeout = options.timeout ?? this.#timeout;
    return new Promise((resolve, reject) => {
      // One timer for the whole call, including waiting for a reconnect.
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new BrowserError({ code: "CLIENT_TIMEOUT", message: `${method}: no answer in ${timeout} ms`, retry: "after_wait" }));
      }, timeout);
      const pending: Pending = { resolve, reject, timer, sent: false };
      this.#pending.set(id, pending);
      const message = JSON.stringify({ jsonrpc: "2.0", id, method, params });
      this.#whenOpen(() => {
        if (!this.#pending.has(id)) return; // timed out while waiting
        pending.sent = true;
        this.#ws?.send(message);
      });
    });
  }

  /**
   * Call `handler` for every event whose topic matches `pattern` (exact, `prefix.*` or `*`).
   * Subscribes on the server first (`events.subscribe`), so events caused by calls made after
   * `on()` are delivered. Returns a function that removes the handler.
   */
  on(pattern: string, handler: EventHandler): () => void {
    let set = this.#handlers.get(pattern);
    if (!set) this.#handlers.set(pattern, (set = new Set()));
    set.add(handler);
    const isDefault = (DEFAULT_TOPICS as readonly string[]).includes(pattern);
    if (!isDefault && !this.#subscribed.has(pattern)) {
      this.#subscribed.add(pattern);
      this.#notify("events.subscribe", { topics: [pattern] });
    }
    return () => {
      set.delete(handler);
      if (set.size > 0 || this.#handlers.get(pattern) !== set) return;
      this.#handlers.delete(pattern);
      if (this.#subscribed.delete(pattern)) this.#notify("events.unsubscribe", { topics: [pattern] });
    };
  }

  // ------------------------------------------------------------------------------------------

  /**
   * Open a socket. `onOpen` / `onFail` report the first outcome of this attempt. A failure is
   * whichever comes first of `close`, `error` (some implementations, e.g. Node 22's, may report a
   * refused connection with `error` only) or the connect timeout, and is handled once.
   */
  #openSocket(onOpen?: () => void, onFail?: () => void): void {
    const ws = new this.#WebSocket(this.url);
    this.#ws = ws;
    let settled = false;
    let lost = false;
    const connectTimer = setTimeout(() => {
      if (settled) return;
      try {
        ws.close();
      } catch {
        // a socket still connecting may refuse close(); it is abandoned either way
      }
      onLost();
    }, this.#connectTimeout);
    const onLost = () => {
      clearTimeout(connectTimer);
      if (lost) return;
      lost = true;
      if (!settled) {
        settled = true;
        onFail?.();
      }
      if (this.#ws !== ws) return;
      this.#open = false;
      this.#ws = null;
      this.#failPending("CONNECTION_LOST", "the connection to the browser dropped", true);
      // Only reconnect a connection that worked once; a failing first connect is reported.
      if (this.#everOpened && this.#reconnect && !this.#closed) this.#scheduleReconnect();
    };
    ws.addEventListener("open", () => {
      clearTimeout(connectTimer);
      if (lost || this.#ws !== ws) return;
      settled = true;
      onOpen?.();
      this.#open = true;
      this.#everOpened = true;
      this.#backoff = this.#minBackoff;
      // Restore subscriptions before any queued call goes out (the server handles frames in order).
      if (this.#subscribed.size > 0) this.#send("events.subscribe", { topics: [...this.#subscribed] });
      const waiters = this.#openWaiters;
      this.#openWaiters = [];
      waiters.forEach((run) => run());
    });
    ws.addEventListener("message", (event) => {
      if (this.#ws === ws) this.#onMessage(event.data);
    });
    ws.addEventListener("close", onLost);
    ws.addEventListener("error", onLost);
  }

  #scheduleReconnect(): void {
    this.#reconnectTimer = setTimeout(() => {
      this.#reconnectTimer = null;
      this.#openSocket(); // if it fails, its "close" handler schedules the next try
    }, this.#backoff);
    this.#backoff = Math.min(this.#backoff * 2, this.#maxBackoff);
  }

  #whenOpen(run: () => void): void {
    if (this.#open) run();
    else this.#openWaiters.push(run);
  }

  /** Fire-and-forget request (its answer, if any, is ignored). */
  #notify(method: string, params: object): void {
    this.#whenOpen(() => this.#send(method, params));
  }

  #send(method: string, params: object): void {
    this.#ws?.send(JSON.stringify({ jsonrpc: "2.0", id: `sdk-${this.#nextId++}`, method, params }));
  }

  /** Reject pending calls (`onlySent`: keep the ones still waiting for a reconnect). */
  #failPending(code: "CONNECTION_LOST" | "CONNECTION_CLOSED", message: string, onlySent = false): void {
    for (const [id, pending] of this.#pending) {
      if (onlySent && !pending.sent) continue;
      clearTimeout(pending.timer);
      pending.reject(new BrowserError({ code, message, retry: "after_wait" }));
      this.#pending.delete(id);
    }
  }

  #onMessage(raw: unknown): void {
    let msg: RpcMessage;
    try {
      msg = JSON.parse(String(raw)) as RpcMessage;
    } catch {
      return; // not JSON: nothing we can match it to
    }
    // Event notification: a method and no id.
    if (msg.method !== undefined && (msg.id === undefined || msg.id === null)) {
      for (const [pattern, handlers] of this.#handlers) {
        if (!topicMatches(pattern, msg.method)) continue;
        for (const handler of handlers) {
          try {
            handler(msg.params, msg.method);
          } catch (err) {
            console.error(`BrowserMCP event handler for ${pattern} threw:`, err);
          }
        }
      }
      return;
    }
    if (typeof msg.id !== "number") return; // an answer to #send (string id) or junk
    const pending = this.#pending.get(msg.id);
    if (!pending) return; // timed out already
    this.#pending.delete(msg.id);
    clearTimeout(pending.timer);
    if (msg.error) {
      const data = isCmdError(msg.error.data)
        ? msg.error.data
        : { code: "BAD_RESPONSE" as const, message: msg.error.message ?? "unknown error" };
      pending.reject(new BrowserError(data, msg.error.code));
    } else {
      pending.resolve(msg.result);
    }
  }
}
