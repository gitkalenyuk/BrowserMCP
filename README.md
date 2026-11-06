# BrowserMCP

A daily-driver browser that people **and** AI agents (Claude Code, Codex, any LLM) both drive.
Chromium via [CEF](https://github.com/tauri-apps/cef-rs), core in Rust, browser chrome in
Svelte 5.

Every browser function is one command in a single registry (`crates/core`). From that registry
BrowserMCP generates all of its interfaces automatically: MCP tools, a REST API with OpenAPI,
a WebSocket JSON-RPC API with events, a typed TypeScript SDK, and the `browser.*` object of
automation scripts. Agents read pages through the accessibility tree as compact text with
stable refs (`@7:e3`), not screenshots.

Status: Windows only for now (macOS later). The plan, in Ukrainian, is
[docs/plan.md](docs/plan.md). All other docs are listed in [docs/README.md](docs/README.md).

## Features

Commands are named `<namespace>.<action>`. On MCP the dot becomes `_` (`nav.goto` is the tool
`nav_goto`). Commands are grouped into **toolsets**. MCP lists only `core` and `system` by
default; ask for more with `/mcp?toolsets=core,net,record`.

| Area | Toolset | Main commands | Crate |
|---|---|---|---|
| Tabs and navigation | `core` | `tabs.list/open/close/activate/freeze`, `nav.goto/back/forward/reload/stop/wait`, `input.keys` | core, browser, governor |
| Seeing and acting on pages | `core` | `page.snapshot/find/act/act_batch/diff/read/text/screenshot`, `simplify.reader` | browser, perception |
| Traffic | `net` | `net.capture/list/get/search/ws_frames/har_export`, `net.rule_add/rule_list/rule_remove` (block, redirect, headers, mocks), `net.breakpoint/paused_list/resume`, `net.replay`, `net.api_to_code` | net, cdpshare |
| Recording and codegen | `record` | `record.start/stop/list/get`, `record.codegen` (action log to a TypeScript script), `record.video_start/video_stop` (needs ffmpeg) | record |
| Scripts | `script` | `script.list/get/save/delete/run` | jobs, script |
| Mass runs and triggers | `run` | `run.start/status/list/results/pause/resume/cancel/export`, `run.heal_apply` (self-healing), `trigger.set/list/remove` (cron) | jobs |
| Daily-driver data | `data` | `data.bookmarks`, `bookmarks.*`, `history.search/delete`, `data.top_sites`, `downloads.list`, `passwords.*`, `import.chrome` | data |
| Resource saving | `data` | `adblock.*` (Brave engine + EasyList), `simplify.site/get/list/measure` (per-site JS/images/fonts/… off) | adblock |
| Hidden mode | `stealth` | `identity.*` (fingerprint identities), `stealth.fingerprint_set/clear/check`, `device.emulate`, `proxy.*` (pools, leases, PAC rules, local forwarder), `geo.lookup`, `input.humanlike` | identity, browser |
| Built-in AI and terminal | `ai` | `ai.providers/provider_set/provider_test`, `ai.session_start/send/stop/transcript`, `ai.sessions_list`, `term.open/write/resize/close/list` | ai |
| Notifications and UI | `ui` | `ui.notify`, `ui.ask_user`, `notify.send/test/config_get/config_set` (UI, Windows toasts, Telegram, webhook), `hints.show/hide/config` (Vim-style link hints), `clipboard.write` | notify, hints |
| System | `system` (`core` for `system.ping`) | `system.ping`, `governor.status/config/recommend_parallel` (tab freezing, RAM/CPU limits) | core, governor |

The full list with argument and result types is the generated
[sdk/protocol/src/generated/commands.ts](sdk/protocol/src/generated/commands.ts); a running
browser serves it at `GET /v1/commands` and `GET /openapi.json`.

A page snapshot looks like this (refs only on interactive elements):

```text
snap 7 tab=3 "Sign in – Acme" https://acme.com/login vp=1280x720 y=0/1840
- main
  - heading "Sign in" h1
  - textbox "Email" @7:e3 req
  - button "Sign in" @7:e6 disabled
```

Errors are structured for agents: `{code, message, retry: now|after_wait|after_snapshot|no,
hint, candidates?}` with codes such as `STALE_REF`, `AMBIGUOUS_TARGET`, `NOT_INTERACTABLE`,
`TAB_LOCKED`, `TIMEOUT`.

## Build and run (Windows)

Requirements: Rust (rustup, MSVC toolchain; the version is pinned in `rust-toolchain.toml`),
Visual Studio Build Tools with the C++ workload and a **Windows 11 SDK (10.0.22621 or newer)**,
CMake (the copy bundled with Visual Studio is found automatically), Ninja, Node 22+ and pnpm.

```bash
cargo xtask doctor          # check that every tool is there
pnpm install                # once: UI + SDK dependencies
pnpm -C ui build            # the browser chrome (ui/dist is embedded at build time)
cargo xtask run             # build + open https://example.com
cargo xtask run -- https://github.com --profile work
cargo build -p bmcp-script  # the script host (V8), needed for script.run / run.start
```

The first build downloads CEF (~500 MB; set `CEF_PATH` to share one download between
checkouts) and copies its runtime next to `target/debug/browsermcp.exe`. Without `ui/dist` the
build embeds a small placeholder UI. `browsermcp --help` lists the options (`--data-dir`,
`--profile`, `--api-port`, `--ui-dir`, `--ui-dev`, …); unknown `--switches` go to Chromium.

## Connect an AI agent

While the browser runs, everything is served on `127.0.0.1:47800` (local only, no tokens;
requests from web pages are rejected by a Host/Origin check):

| URL | What |
|---|---|
| `http://127.0.0.1:47800/mcp` | MCP (Streamable HTTP); `?toolsets=core,net,…` for more tool groups |
| `http://127.0.0.1:47800/v1` | REST: `POST /v1/cmd/{name}`, `GET /v1/commands`, `GET /v1/health` |
| `http://127.0.0.1:47800/openapi.json` | OpenAPI 3.1 |
| `ws://127.0.0.1:47800/v1/ws` | WebSocket: JSON-RPC 2.0 calls + event notifications |

The running instance writes these URLs (and its pid) to `data/runtime/endpoint.json`.

**Claude Code.** This repo ships [.mcp.json](.mcp.json); or add it globally:

```bash
claude mcp add --transport http browsermcp http://127.0.0.1:47800/mcp
```

**Clients that only speak stdio** (this also starts the browser on demand):

```bash
claude mcp add browsermcp -- C:\path\to\browsermcp.exe mcp-stdio
```

**Codex** (`~/.codex/config.toml`):

```toml
[mcp_servers.browsermcp]
command = 'C:\path\to\browsermcp.exe'
args = ["mcp-stdio"]
```

**Any other MCP client:** point it at `http://127.0.0.1:47800/mcp` (Streamable HTTP) or run
`browsermcp.exe mcp-stdio` as a stdio server.

The browser also has its own AI side panel (Claude Code, Codex, or API providers: Anthropic,
Gemini, OpenAI-compatible) whose agents get the MCP tools automatically; see
[docs/phase5/panels.md](docs/phase5/panels.md).

## REST, WebSocket and SDK

REST (`curl` on Windows: use `curl.exe`; the body is the command's arguments):

```bash
curl -X POST http://127.0.0.1:47800/v1/cmd/tabs.open -H "content-type: application/json" -d "{\"url\":\"https://example.com\"}"
curl -X POST http://127.0.0.1:47800/v1/cmd/page.snapshot -d "{}"
```

WebSocket JSON-RPC (`method` = command name; subscribe to events with `events.subscribe`):

```text
-> {"jsonrpc":"2.0","id":1,"method":"tabs.list","params":{}}
<- {"jsonrpc":"2.0","id":1,"result":{"tabs":[{"id":1,"title":"Example Domain","url":"https://example.com/","loading":false}]}}
-> {"jsonrpc":"2.0","id":2,"method":"events.subscribe","params":{"topics":["command.*","run.*"]}}
<- {"jsonrpc":"2.0","method":"command.finished","params":{"name":"tabs.list","ok":true,…,"seq":12}}
```

TypeScript SDK ([sdk/ts](sdk/ts/README.md), Node 22+; not published yet, use it from this
workspace after `pnpm -r build`):

```ts
import { connect } from "@browsermcp/sdk";

const browser = await connect(); // ws://127.0.0.1:47800/v1/ws
const { tab } = await browser.tabs.open({ url: "https://example.com" });
await browser.nav.wait({ tab, for: "load" });
console.log((await browser.page.snapshot({ tab })).text);
browser.on("tab.*", (data, topic) => console.log(topic, data));
```

Details: [crates/api/README.md](crates/api/README.md) (routes, error statuses, event topics,
security) and [sdk/ts/README.md](sdk/ts/README.md).

## Scripts, runs and triggers

Scripts are TypeScript files in `data/scripts/`, run by `bmcp-script-host`, a separate V8
process (a crashing script never takes the browser down). They get the same typed `browser.*`
object as the SDK, plus `inputs` (strings) and `browser.output({...})` for structured results.

```ts
// data/scripts/title.ts
const { tab } = await browser.tabs.open({ url: inputs.url });
await browser.nav.wait({ tab, for: "load" });
browser.output({ url: inputs.url, title: (await browser.tabs.list()).tabs.find((t) => t.id === tab)?.title });
```

```text
script.save  {name: "title", code: "…"}
script.run   {name: "title", inputs: {url: "https://example.com"}}      → {ok, result, stdout, error, …}
run.start    {script: "title", data: {csv_path: "C:/urls.csv"}, parallel: 4, retries: 1}
run.status   {id}  ·  run.results {id, cursor?}  ·  run.export {id, to: "xlsx"}
trigger.set  {kind: "cron", spec: "0 9 * * Mon-Fri", script: "title", inputs: {url: "…"}}
```

Run data comes from CSV, JSON, Excel or inline rows; one script process per row, retries with
backoff, pause/resume/cancel, everything in `data/db/runs.sqlite` (a run survives a browser
restart). When a row breaks on a page selector (`NOT_FOUND`, `STALE_REF`, …) the run pauses and
emits `run.heal_needed`; an agent answers with `run.heal_apply {id, patched_code}`, which saves
`<name>.v2` and continues. Exports: CSV, Excel, JSON, SQLite, webhook. See
[crates/jobs/README.md](crates/jobs/README.md).

Record instead of writing by hand: `record.start`, use the page, `record.stop`, then
`record.codegen {id}` gives a script to save.

Without a browser, try the host on its own (`--mock` answers `browser.*` with canned data):

```bash
cargo run -p bmcp-script -- run scripts/examples/hello.ts --input name=Ada
```

Types for editors: reference `crates/script/types/browser.d.ts` (generated) as
`scripts/examples/*.ts` do. Plugins and userscripts are planned, not built yet (`data/plugins/`
is reserved for them).

## Data folder

Everything is portable: it lives in `data/` next to the executable (`--data-dir <path>`
overrides it).

```text
data/
  config/        settings per feature, plain JSON: ai.json (provider keys included),
                 notify.json, governor.json, hints.json, identities.json, proxies.json,
                 adblock.json, simplify.json
  profiles/<id>/ Chromium profile (cookies, cache, …) of one profile
  db/            browsermcp.sqlite (bookmarks, history, downloads, passwords — unencrypted
                 by design), runs.sqlite (runs, rows, triggers)
  filters/       adblock filter lists, the compiled engine, user.txt (your own rules)
  scripts/       saved scripts (<name>.ts, healed versions <name>.v2.ts, …)
  runs/<id>/     run exports (results.csv, …)
  recordings/<id>/  action logs (actions.jsonl, meta.json)
  videos/ har/ logs/
  runtime/       endpoint.json (pid + URLs of the running instance)
```

## Contributing

The same checks run in CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)):

```bash
cargo fmt --all --check
cargo clippy --workspace --exclude bmcp-script --all-targets -- -D warnings
cargo xtask test                 # Rust tests without the V8 script host
cargo test -p bmcp-script        # the script host (V8: ~200 MB download, slow first build)
cargo xtask gen-sdk --check      # generated TypeScript is up to date
pnpm install && pnpm -r build && pnpm -r test
pnpm -C ui check                 # svelte-check, fails on warnings
```

* **Adding a command:** implement `bmcp_core::Command` in the crate that owns the feature and
  register it in that crate's `register` function (new crates: one line in
  `crates/registry`). Then run `cargo xtask gen-sdk` and commit the regenerated
  `sdk/protocol/src/generated/commands.ts` and `crates/script/types/browser.d.ts`. MCP, REST,
  WS, the SDK and scripts pick it up automatically.
* **No CEF outside `crates/cef`.** Everything else talks to the browser through the
  `BrowserBackend` trait and CDP, and is tested with `bmcp_core::testing::FakeBackend`.
* JSON objects keep their key order (serde_json `preserve_order` is on for the whole
  workspace); every crate uses `serde_json.workspace = true`.
* UI development without the browser: `pnpm ui:dev` (in-memory mock backend); see
  [ui/README.md](ui/README.md).

## Crate map

| Crate | Package | What |
|---|---|---|
| [crates/adblock](crates/adblock/README.md) | `bmcp-adblock` | Adblock (Brave engine + EasyList), per-site simplification, reader mode |
| [crates/ai](crates/ai) | `bmcp-ai` | Built-in AI: model providers, agent loop, Claude Code / Codex sessions, PTY terminals |
| [crates/api](crates/api/README.md) | `bmcp-api` | API servers: MCP (Streamable HTTP), REST + OpenAPI, WebSocket JSON-RPC, Host/Origin guard |
| [crates/app](crates/app) | `browsermcp` | The `browsermcp` binary: CLI, wiring of all services, `mcp-stdio` proxy |
| [crates/browser](crates/browser) | `bmcp-browser` | Page control: snapshot/find/act/diff/read/screenshot, navigation, waits, keys, human-like input |
| [crates/cdp](crates/cdp) | `bmcp-cdp` | Chrome DevTools Protocol client over a pluggable transport (in-process CEF or WebSocket) |
| [crates/cdpshare](crates/cdpshare/README.md) | `bmcp-cdpshare` | Shared use of a tab's CDP session: domain refcount, one `Fetch` coordinator per tab |
| [crates/cef](crates/cef) | `bmcp-cef` | The only crate that talks to CEF: process startup, windows, views, handlers, `bmcp://ui` |
| [crates/core](crates/core) | `bmcp-core` | Command registry, `BrowserBackend` trait, event bus, domain model, portable paths |
| [crates/data](crates/data) | `bmcp-data` | Bookmarks, history, downloads, passwords, Chrome import (SQLite) |
| [crates/governor](crates/governor) | `bmcp-governor` | Tab freezing/discarding and automatic limits from RAM/CPU |
| [crates/hints](crates/hints) | `bmcp-hints` | Vim-style link hints drawn inside the page |
| [crates/identity](crates/identity/README.md) | `bmcp-identity` | Hidden mode: fingerprint identities, stealth checks, device emulation, proxy pools, local proxy forwarder |
| [crates/jobs](crates/jobs/README.md) | `bmcp-jobs` | Script library, mass runs (queue, data, exports), cron triggers, self-healing |
| [crates/net](crates/net) | `bmcp-net` | Traffic: capture, HAR, rules/mocks, breakpoints, replay, API-to-code |
| [crates/notify](crates/notify) | `bmcp-notify` | Notifications: UI toasts, Windows toasts, Telegram bot with answer buttons, webhook |
| [crates/perception](crates/perception/README.md) | `bmcp-perception` | CDP accessibility tree to compact, token-efficient snapshot text with refs |
| [crates/record](crates/record) | `bmcp-record` | Recording: action log (API + human), codegen to TypeScript, screencast video via ffmpeg |
| [crates/registry](crates/registry) | `bmcp-registry` | Builds THE command registry (every command crate registers here) |
| [crates/schema-dump](crates/schema-dump) | `bmcp-schema-dump` | `bmcp-schema`: prints every command with its JSON Schemas for `gen-sdk` |
| [crates/script](crates/script) | `bmcp-script` | `bmcp-script-host`: runs TypeScript scripts in an embedded V8 (deno_core) sidecar |

Other parts:

| Path | What |
|---|---|
| [ui/](ui/README.md) | Svelte 5 browser chrome, new-tab page, library, side panels (`pnpm ui:dev`) |
| [sdk/protocol](sdk/protocol/README.md) | `@browsermcp/protocol`: generated command types and the `createBrowser` wrapper |
| [sdk/ts](sdk/ts/README.md) | `@browsermcp/sdk`: WebSocket / HTTP transports, events, `connect()` |
| `xtask/` | `cargo xtask doctor/build/run/test/gen-sdk` |
| `scripts/examples/` | Example scripts |
| `tests/fixtures/` | Local test pages and recorded CDP data |
| [docs/](docs/README.md) | Plan, spike notes, phase notes |
