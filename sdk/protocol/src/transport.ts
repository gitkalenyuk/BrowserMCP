// How the typed `browser` object talks to BrowserMCP. `@browsermcp/sdk` has a WebSocket and an
// HTTP implementation; tests and the UI can plug in their own (e.g. a mock).

/** Per-call options, accepted as the last argument of every command method. */
export interface CallOptions {
  /** Give up after this many milliseconds (default: the transport's timeout). */
  timeout?: number;
}

/** Sends one command and resolves with its JSON result, or rejects with an error. */
export interface Transport {
  /**
   * @param method dotted command name, e.g. `"tabs.list"` (wire names like `tabs_list` work too)
   * @param params the arguments object (`{}` when there are none)
   */
  call(method: string, params: object, options?: CallOptions): Promise<unknown>;
}
