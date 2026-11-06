// Prelude: runs once in every isolate before the user's script.
// Defines the globals scripts use: `browser` (incl. `browser.output` and `browser.on`), `inputs`,
// `console`, `sleep`.
// Types for editors live in crates/script/types/browser.d.ts.

import {
  op_bmcp_call,
  op_bmcp_inputs,
  op_bmcp_log,
  op_bmcp_next_event,
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

/**
 * Report a structured result to whoever started the script (script.run, a run.start row):
 * prints one stdout line `##bmcp:result <json>`. Several calls are merged (object keys
 * combine, later values win). See crates/jobs/src/host.rs.
 */
function output(value) {
  op_bmcp_log("##bmcp:result " + JSON.stringify(value === undefined ? null : value), false);
}

// ---- Events: `browser.on(pattern, handler)` --------------------------------------------------
// Same meaning as in the TS SDK (sdk/ts/src/ws.ts): `pattern` is an exact topic
// ("tab.updated"), a prefix ("tab.*") or "*"; `handler(data, topic)` runs for every matching
// event; the return value is a function that removes the handler. The first `on()` starts a
// dispatch loop that awaits events from the sink. While that loop waits the script stays alive
// (this is how a plugin keeps running); it ends when the browser connection closes, or after
// the next event once every handler was removed.
const handlers = new Map(); // pattern -> Set of handlers
let dispatching = false;

function topicMatches(pattern, topic) {
  if (pattern === "*") return true;
  if (pattern.endsWith(".*")) return topic.startsWith(pattern.slice(0, -1)); // keeps the dot
  return pattern === topic;
}

function reportHandlerError(pattern, err) {
  op_bmcp_log(`browser.on("${pattern}") handler failed: ${format(err)}`, true);
}

async function dispatchLoop() {
  while (handlers.size > 0) {
    const event = await op_bmcp_next_event();
    if (event === null) break; // connection closed (or a sink without events)
    for (const [pattern, set] of handlers) {
      if (!topicMatches(pattern, event.topic)) continue;
      for (const handler of [...set]) {
        try {
          const result = handler(event.data, event.topic);
          // An async handler's rejection is reported, never fatal for the script.
          if (result && typeof result.then === "function") {
            result.then(undefined, (err) => reportHandlerError(pattern, err));
          }
        } catch (err) {
          reportHandlerError(pattern, err);
        }
      }
    }
  }
  dispatching = false;
}

function on(pattern, handler) {
  if (typeof pattern !== "string" || typeof handler !== "function") {
    throw new TypeError("usage: browser.on(pattern: string, handler: (data, topic) => void)");
  }
  let set = handlers.get(pattern);
  const isNewPattern = !set;
  if (!set) handlers.set(pattern, (set = new Set()));
  set.add(handler);
  // Start listening BEFORE subscribing, so no event of the new subscription is dropped.
  if (!dispatching) {
    dispatching = true;
    dispatchLoop();
  }
  if (isNewPattern) {
    call("events.subscribe", { topics: [pattern] }).catch((err) => reportHandlerError(pattern, err));
  }
  return () => {
    set.delete(handler);
    if (set.size > 0 || handlers.get(pattern) !== set) return;
    handlers.delete(pattern);
    call("events.unsubscribe", { topics: [pattern] }).catch(() => {});
  };
}

const browser = new Proxy(Object.freeze({ call, output, on }), {
  get(target, ns) {
    if (ns === "call" || ns === "output" || ns === "on") return target[ns];
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
