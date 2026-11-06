import type { CmdError, ErrorCode, Retry } from "@browsermcp/protocol";

/** Errors made by the SDK itself (the browser never sends these). */
export type ClientErrorCode =
  /** The first connection to the browser failed (is it running? right port?). */
  | "CONNECTION_FAILED"
  /** The connection dropped while the call was in flight; it may or may not have run. */
  | "CONNECTION_LOST"
  /** `close()` was called. */
  | "CONNECTION_CLOSED"
  /** No answer within the call's timeout. */
  | "CLIENT_TIMEOUT"
  /** The server sent something that is not a valid answer. */
  | "BAD_RESPONSE";

/** A `CmdError`, or the same fields with a client-side code. */
export type BrowserErrorInit = Omit<Partial<CmdError>, "code" | "message"> & {
  code: ErrorCode | ClientErrorCode;
  message: string;
};

/**
 * What every failed call rejects with: the browser's `CmdError` (`code`, `message`, `retry`,
 * `hint`, `candidates`) or a {@link ClientErrorCode} for transport problems.
 *
 * ```ts
 * try { await browser.page.act({ ref: "@3:e9", action: "click" }); }
 * catch (err) {
 *   if (err instanceof BrowserError && err.retry === "after_snapshot") { … }
 * }
 * ```
 */
export class BrowserError extends Error {
  override readonly name = "BrowserError";
  readonly code: ErrorCode | ClientErrorCode;
  /** What to do about it: `now`, `after_wait`, `after_snapshot` or `no`. */
  readonly retry: Retry;
  /** What to do next, e.g. "Call tabs_list to see the open tabs." */
  readonly hint: string | undefined;
  /** Possible intended targets (ambiguous element, unknown command name, …). */
  readonly candidates: unknown[];
  /** JSON-RPC error code (WebSocket) or HTTP status (REST) when the server answered. */
  readonly status: number | undefined;

  constructor(error: BrowserErrorInit, status?: number) {
    super(error.message);
    this.code = error.code;
    this.retry = error.retry ?? "no";
    this.hint = error.hint;
    this.candidates = error.candidates ?? [];
    this.status = status;
  }

  /** The same shape as the browser's `CmdError`, e.g. for logging. */
  toJSON(): CmdError | Record<string, unknown> {
    return {
      code: this.code,
      message: this.message,
      retry: this.retry,
      ...(this.hint === undefined ? {} : { hint: this.hint }),
      ...(this.candidates.length ? { candidates: this.candidates } : {}),
    };
  }
}

/** Is `value` a `CmdError` object (`{code, message, …}`)? */
export function isCmdError(value: unknown): value is CmdError {
  const v = value as CmdError | null;
  return typeof v === "object" && v !== null && typeof v.code === "string" && typeof v.message === "string";
}
