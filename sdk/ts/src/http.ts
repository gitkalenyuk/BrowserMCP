// REST transport: `POST <base>/cmd/<name>` with the arguments as JSON body (see
// crates/api/README.md). 200 = the output; any other status = a `CmdError` body.
// No events over HTTP; use the WebSocket transport for `on()`.

import type { CallOptions, Transport } from "@browsermcp/protocol";
import { BrowserError, isCmdError } from "./errors.js";

export const DEFAULT_HTTP_URL = "http://127.0.0.1:47800/v1";

export interface HttpOptions {
  /** REST base URL, default `http://127.0.0.1:47800/v1` (the `rest` value of endpoint.json). */
  url?: string;
  /** Default per-call timeout in ms (default 60 000). */
  timeout?: number;
  /** `fetch` to use instead of the global one. */
  fetch?: typeof fetch;
}

export class HttpTransport implements Transport {
  readonly url: string;
  readonly #timeout: number;
  readonly #fetch: typeof fetch;

  constructor(options: HttpOptions = {}) {
    this.url = (options.url ?? DEFAULT_HTTP_URL).replace(/\/+$/, "");
    this.#timeout = options.timeout ?? 60_000;
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
  }

  async call(method: string, params: object, options: CallOptions = {}): Promise<unknown> {
    const timeout = options.timeout ?? this.#timeout;
    let response: Response;
    try {
      response = await this.#fetch(`${this.url}/cmd/${encodeURIComponent(method)}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(params),
        signal: AbortSignal.timeout(timeout),
      });
    } catch (err) {
      if (err instanceof Error && err.name === "TimeoutError") {
        throw new BrowserError({ code: "CLIENT_TIMEOUT", message: `${method}: no answer in ${timeout} ms`, retry: "after_wait" });
      }
      throw new BrowserError({ code: "CONNECTION_FAILED", message: `cannot reach ${this.url}: ${String(err)}`, retry: "after_wait" });
    }

    const text = await response.text();
    let body: unknown;
    try {
      body = text ? JSON.parse(text) : {};
    } catch {
      throw new BrowserError({ code: "BAD_RESPONSE", message: `${method}: HTTP ${response.status}, not JSON: ${text.slice(0, 200)}` }, response.status);
    }
    if (response.ok) return body;
    if (isCmdError(body)) throw new BrowserError(body, response.status);
    throw new BrowserError({ code: "BAD_RESPONSE", message: `${method}: HTTP ${response.status}` }, response.status);
  }
}
