// Ambient types for BrowserMCP scripts, for editors (VS Code / Monaco).
// The host only strips types; it does not type-check.
//
// Use from a script:  /// <reference path="../../crates/script/types/browser.d.ts" />
// Spike S5 version: every command returns `unknown`. Phase 8 generates precise
// types from the command registry.

/** One browser command namespace, e.g. `browser.tabs`. Any method name is allowed. */
type BrowserNamespace = {
  [method: string]: (args?: unknown) => Promise<unknown>;
};

/** `browser.<ns>.<method>(args)` sends command "<ns>.<method>" to the browser. */
declare const browser: {
  /** Low level: `browser.call("tabs.list", {})`. */
  call(name: string, args?: unknown): Promise<unknown>;
  /** Structured result for script.run / run.start rows (prints `##bmcp:result <json>`;
   *  several calls merge object keys). */
  output(value: unknown): void;
  tabs: BrowserNamespace;
  nav: BrowserNamespace;
  page: BrowserNamespace;
} & { [ns: string]: BrowserNamespace };

/** Values from `--input key=value` (always strings). */
declare const inputs: Readonly<Record<string, string | undefined>>;

/** Wait `ms` milliseconds. */
declare function sleep(ms: number): Promise<void>;
