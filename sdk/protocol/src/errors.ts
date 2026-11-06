// The error every command can return (Rust: `bmcp_core::CmdError`, crates/core/src/command/error.rs).
// Over REST it is the response body; over WebSocket it is `error.data` of the JSON-RPC answer.

/** Machine-readable error code. */
export type ErrorCode =
  /** An element ref (`@7:e3`) is from an old snapshot. */
  | "STALE_REF"
  /** A target description matched more than one element; see `candidates`. */
  | "AMBIGUOUS_TARGET"
  /** Waiting for something took too long. */
  | "TIMEOUT"
  /** The element exists but cannot be clicked/typed into (hidden, disabled, covered). */
  | "NOT_INTERACTABLE"
  /** Something (command, element, file, …) does not exist. */
  | "NOT_FOUND"
  /** Another caller (agent or human) holds the lock on this tab. */
  | "TAB_LOCKED"
  /** No tab with that id. */
  | "TAB_NOT_FOUND"
  /** The command needs a tab but none is open. */
  | "NO_TABS"
  /** A navigation started while the command was running. */
  | "NAVIGATION_INTERRUPTED"
  /** The renderer process of the tab crashed. */
  | "PAGE_CRASHED"
  /** The tab was closed while the command was running. */
  | "TAB_CLOSED"
  /** The user must approve this action first. */
  | "NEEDS_APPROVAL"
  /** Not allowed while the profile is in stealth mode. */
  | "BLOCKED_IN_STEALTH"
  /** Arguments did not match the command's schema. */
  | "INVALID_PARAMS"
  /** The browser is shutting down. */
  | "SHUTTING_DOWN"
  /** The command exists but this option is not built yet (see `hint`). */
  | "NOT_IMPLEMENTED"
  /** A JavaScript dialog blocks the page; `candidates[0]` = `{tab, type, message, default_prompt, url}`. Answer it with `page.dialog`. */
  | "DIALOG_OPEN"
  /** A bug or an unexpected failure. */
  | "INTERNAL";

/** What the caller should do about an error. */
export type Retry =
  /** Retrying the same call right away will probably work. */
  | "now"
  /** Retry after waiting (page still loading, tab locked by someone else, …). */
  | "after_wait"
  /** Take a fresh snapshot first; the old refs/targets are no longer valid. */
  | "after_snapshot"
  /** Retrying the same call will fail again; change the arguments or give up. */
  | "no";

/** `{"code":"TAB_NOT_FOUND","message":"tab 7 not found","retry":"no","hint":"Call tabs_list…"}` */
export interface CmdError {
  code: ErrorCode;
  message: string;
  retry: Retry;
  /** What to do next, e.g. "Call tabs_list to see the open tabs." */
  hint?: string;
  /** Possible intended targets (for `AMBIGUOUS_TARGET`, unknown command names, …). */
  candidates?: unknown[];
}
