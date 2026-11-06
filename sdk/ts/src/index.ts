// @browsermcp/sdk: drive BrowserMCP from Node or a web page with the same typed `browser.*`
// the script host offers.
//
//   import { connect } from "@browsermcp/sdk";
//   const browser = await connect();                    // ws://127.0.0.1:47800/v1/ws
//   const { tab } = await browser.tabs.open({ url: "https://example.com" });
//   console.log((await browser.page.snapshot({ tab })).text);
//   browser.close();

import {
  createBrowser,
  type Browser,
  type EventTopic,
  type Events,
  type EventsSubscribeArgs,
  type EventsSubscribeOutput,
  type Transport,
} from "@browsermcp/protocol";
import { HttpTransport, type HttpOptions } from "./http.js";
import { WsTransport, type EventHandler, type WsOptions } from "./ws.js";

export * from "@browsermcp/protocol";
export { BrowserError, type ClientErrorCode } from "./errors.js";
export { DEFAULT_HTTP_URL, HttpTransport, type HttpOptions } from "./http.js";
export { DEFAULT_WS_URL, WsTransport, type EventHandler, type WsOptions } from "./ws.js";

export interface ConnectOptions extends WsOptions, Omit<HttpOptions, "url"> {
  /**
   * `ws://…/v1/ws` (default `ws://127.0.0.1:47800/v1/ws`): WebSocket, with events.
   * `http://…/v1`: REST, without events.
   */
  url?: string;
}

/** The `browser` object returned by {@link connect}: every command, plus events. */
export interface BrowserClient extends Browser {
  /**
   * Listen to events: an exact topic (`"command.finished"`, typed payload), `"prefix.*"` or
   * `"*"`. Returns a function that stops listening. WebSocket only.
   */
  on<T extends EventTopic>(topic: T, handler: EventHandler<Events[T]>): () => void;
  on(pattern: string, handler: EventHandler): () => void;
  /** Raw server-side subscription management (`on()` does this for you). WebSocket only. */
  events: {
    subscribe(args: EventsSubscribeArgs): Promise<EventsSubscribeOutput>;
    unsubscribe(args?: Partial<EventsSubscribeArgs>): Promise<EventsSubscribeOutput>;
  };
  /** The transport, for untyped calls: `browser.transport.call("x.y", {})`. */
  readonly transport: Transport;
  /** Close the connection (no reconnect). */
  close(): void;
}

/**
 * Connect to a running BrowserMCP and return the typed `browser` object.
 * WebSocket: waits until connected, rejects with `CONNECTION_FAILED` if it cannot be reached,
 * reconnects automatically after a drop.
 */
export async function connect(options: ConnectOptions = {}): Promise<BrowserClient> {
  if (options.url && /^https?:/i.test(options.url)) {
    return withHttp(new HttpTransport({ ...options, url: options.url }));
  }
  const transport = new WsTransport(options);
  await transport.connect();
  return withWs(transport);
}

/** `browser` over an existing WebSocket transport (not connected yet is fine). */
export function withWs(transport: WsTransport): BrowserClient {
  return Object.assign(createBrowser(transport), {
    on: (pattern: string, handler: EventHandler) => transport.on(pattern, handler),
    events: {
      subscribe: (args: EventsSubscribeArgs) => transport.call("events.subscribe", args) as Promise<EventsSubscribeOutput>,
      unsubscribe: (args: Partial<EventsSubscribeArgs> = {}) =>
        transport.call("events.unsubscribe", args) as Promise<EventsSubscribeOutput>,
    },
    transport,
    close: () => transport.close(),
  });
}

/** `browser` over REST. `on()` / `events` throw: HTTP has no events. */
export function withHttp(transport: HttpTransport): BrowserClient {
  const noEvents = (): never => {
    throw new Error("events need the WebSocket transport: connect({ url: 'ws://…/v1/ws' })");
  };
  return Object.assign(createBrowser(transport), {
    on: noEvents,
    events: { subscribe: noEvents, unsubscribe: noEvents },
    transport,
    close: () => {},
  });
}
