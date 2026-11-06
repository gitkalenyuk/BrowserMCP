# @browsermcp/sdk

Drive a running BrowserMCP from Node (22+) or a web page with a typed `browser` object: the
same `browser.<group>.<action>(args)` that scripts get inside the script host. Every command,
argument and result type is generated from the Rust command registry
(`@browsermcp/protocol`, `cargo xtask gen-sdk`), so a typo in a command name or argument is a
compile error.

```ts
import { BrowserError, connect } from "@browsermcp/sdk";

const browser = await connect(); // ws://127.0.0.1:47800/v1/ws

const { tab } = await browser.tabs.open({ url: "https://example.com" });
await browser.nav.wait({ tab, for: "load" });

const snap = await browser.page.snapshot({ tab });
console.log(snap.text); // - link "More information..." @1:e1 ...

const found = await browser.page.find({ tab, role: "link" });
console.log(found.total, found.text);

browser.close();
```

## Connecting

| Option | Default | |
|---|---|---|
| `url` | `ws://127.0.0.1:47800/v1/ws` | `ws://…/v1/ws` = WebSocket (events, reconnect); `http://…/v1` = REST (no events). The running browser writes both URLs to `<data>/runtime/endpoint.json`. |
| `timeout` | `60000` | Per-call timeout in ms. Override per call: `browser.nav.goto({ url }, { timeout: 90_000 })`. |
| `reconnect` | `true` | After a drop, reconnect with backoff (`minBackoff` 250 ms … `maxBackoff` 5 s) and restore event subscriptions. The *first* connect does not retry: `connect()` rejects with `CONNECTION_FAILED`. |
| `WebSocket` | global | WebSocket class to use (e.g. the `ws` package) where there is no global one. |
| `fetch` | global | For the HTTP transport. |

Calls made while reconnecting wait (within their timeout). Calls in flight when the connection
drops fail with `CONNECTION_LOST` — they may or may not have run.

## Errors

Every failure rejects with a `BrowserError`:

```ts
try {
  await browser.page.act({ ref: "@3:e9", action: "click" });
} catch (err) {
  if (!(err instanceof BrowserError)) throw err;
  err.code;       // "STALE_REF", "NOT_INTERACTABLE", "TAB_LOCKED", … (see ErrorCode)
  err.retry;      // "now" | "after_wait" | "after_snapshot" | "no"
  err.hint;       // "Take a new page_snapshot …"
  err.candidates; // possible intended targets
  err.status;     // JSON-RPC error code (WS) or HTTP status (REST)
}
```

Client-side codes: `CONNECTION_FAILED`, `CONNECTION_LOST`, `CONNECTION_CLOSED`,
`CLIENT_TIMEOUT`, `BAD_RESPONSE`.

## Events (WebSocket)

```ts
const off = browser.on("command.finished", (e) => {
  console.log(`${e.name} took ${e.duration_ms} ms (ok: ${e.ok})`); // typed payload
});
browser.on("tab.*", (data, topic) => console.log(topic, data)); // patterns: exact, prefix.*, *
off(); // stop listening (unsubscribes on the server when nobody listens any more)
```

`on()` sends `events.subscribe` before any later call, so events caused by calls made after
`on()` are never missed. `tab.*`, `tabs.*`, `agents.*`, `workspace.*` and `runs.*` are
subscribed by default; `command.*` is not. Raw control: `browser.events.subscribe({ topics })`.

## Low level

```ts
await browser.call("tabs.list");                 // typed by name
await browser.transport.call("x.y", { a: 1 });   // untyped, any name (e.g. a newer browser)
```

`createBrowser(transport)` (from `@browsermcp/protocol`) builds the typed object on any
`Transport` — e.g. a mock in tests. `WsTransport` and `HttpTransport` are exported too.

## Development

```sh
cargo xtask gen-sdk     # regenerate ../protocol/src/generated/commands.ts after changing commands
pnpm -r build           # protocol first, then this package (dist/)
pnpm --filter @browsermcp/sdk test   # tsc type tests + vitest against an in-process mock server
```

`test/types/sample.ts` is a type-level test (`@ts-expect-error` lines must fail to compile);
`test/types/script/` checks `scripts/examples/*.ts` against the generated
`crates/script/types/browser.d.ts`.
