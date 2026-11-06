// Prelude: runs once in every isolate before the user's script.
// Defines the globals scripts use: `browser`, `inputs`, `console`, `sleep`.
// Types for editors live in crates/script/types/browser.d.ts.

import {
  op_bmcp_call,
  op_bmcp_inputs,
  op_bmcp_log,
  op_bmcp_sleep,
} from "ext:core/ops";

/** Low-level escape hatch: `await browser.call("tabs.list", {})`. */
function call(name, args) {
  return op_bmcp_call(String(name), args === undefined ? null : args);
}

// `browser.<ns>.<method>(args)` -> call("<ns>.<method>", args).
// Proxies let ANY name work without listing commands here, so new browser
// commands need no prelude change. "then" is excluded so that
// `await browser` / `await browser.tabs` don't treat the proxy as a Promise.
const namespaces = new Map();
function namespace(ns) {
  let proxy = namespaces.get(ns);
  if (!proxy) {
    proxy = new Proxy(Object.create(null), {
      get(_target, method) {
        if (typeof method !== "string" || method === "then") return undefined;
        return (args) => call(`${ns}.${method}`, args);
      },
    });
    namespaces.set(ns, proxy);
  }
  return proxy;
}

const browser = new Proxy(Object.freeze({ call }), {
  get(target, ns) {
    if (ns === "call") return target.call;
    if (typeof ns !== "string" || ns === "then") return undefined;
    return namespace(ns);
  },
});

// console.* -> Rust (stdout / stderr), so the host decides where logs go.
function format(value) {
  if (typeof value === "string") return value;
  if (value instanceof Error) return value.stack ?? String(value);
  if (value === undefined || typeof value === "function" || typeof value === "symbol") {
    return String(value);
  }
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value); // e.g. circular objects
  }
}
const toStdout = (...args) => op_bmcp_log(args.map(format).join(" "), false);
const toStderr = (...args) => op_bmcp_log(args.map(format).join(" "), true);
const console = { log: toStdout, info: toStdout, debug: toStdout, warn: toStderr, error: toStderr };

/** Wait `ms` milliseconds. */
function sleep(ms) {
  return op_bmcp_sleep(Number(ms) || 0);
}

const readOnly = (value) => ({ value, writable: false, enumerable: false, configurable: false });
Object.defineProperties(globalThis, {
  browser: readOnly(browser),
  inputs: readOnly(Object.freeze(op_bmcp_inputs())),
  console: readOnly(Object.freeze(console)),
  sleep: readOnly(sleep),
});
