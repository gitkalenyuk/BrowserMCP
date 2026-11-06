# bmcp-api

All BrowserMCP API surfaces, on one port (`127.0.0.1:47800`, `--api-port` to change). Every
surface is generated from the command `Registry` (`crates/core`), so a new command shows up in
all of them without touching this crate.

| Route | What |
|---|---|
| `/mcp` | MCP (Streamable HTTP), see `src/mcp.rs` |
| `GET /v1/health` | `{"ok":true,"name":"BrowserMCP","version":"0.1.0","commands":6}` |
| `GET /v1/commands?toolsets=core,net` | `{"commands":[CommandInfo…]}` with `args_schema` / `output_schema`; no `toolsets` = all except `hidden` |
| `POST /v1/cmd/{name}` | body = arguments (JSON object, empty = `{}`); 200 + output, or error status + `CmdError` |
| `GET /openapi.json` | OpenAPI 3.1, one `POST /v1/cmd/{name}` path per command |
| `GET /v1/ws` | WebSocket, JSON-RPC 2.0 + event notifications |

The running browser writes the URLs to `<data>/runtime/endpoint.json`:
`{"pid":…,"mcp":"http://127.0.0.1:47800/mcp","rest":"http://127.0.0.1:47800/v1","ws":"ws://127.0.0.1:47800/v1/ws"}`.

## REST errors

Body: `{"code":"TAB_NOT_FOUND","message":"tab 99 not found","retry":"no","hint":"…","candidates":[…]}`.
Status by code (`rest::status_for`): `INVALID_PARAMS` 400 · `NEEDS_APPROVAL`, `BLOCKED_IN_STEALTH` 403 ·
`NOT_FOUND`, `TAB_NOT_FOUND` 404 · `STALE_REF`, `AMBIGUOUS_TARGET`, `NO_TABS`,
`NAVIGATION_INTERRUPTED` 409 · `TAB_CLOSED` 410 · `NOT_INTERACTABLE` 422 · `TAB_LOCKED` 423 ·
`INTERNAL` 500 · `NOT_IMPLEMENTED` 501 · `PAGE_CRASHED` 502 · `SHUTTING_DOWN` 503 · `TIMEOUT` 504.

## WebSocket (JSON-RPC 2.0)

```text
-> {"jsonrpc":"2.0","id":1,"method":"tabs.list","params":{}}
<- {"jsonrpc":"2.0","id":1,"result":{"tabs":[{"id":1,"loading":false,"title":"Example Domain","url":"https://example.org/"}]}}
-> {"jsonrpc":"2.0","id":4,"method":"nav.goto","params":{"tab":"x"}}
<- {"jsonrpc":"2.0","id":4,"error":{"code":-32602,"message":"invalid arguments for nav.goto: …","data":{"code":"INVALID_PARAMS",…}}}
```

* `method` = command name, dotted or wire (`nav_goto`). Requests run concurrently; answers may
  come in any order. No `id` = notification (runs, no answer). No batches.
* Error codes: `-32700` bad JSON, `-32600` bad request, `-32601` unknown command, `-32602` invalid
  params, `-32000` other command errors. `error.data` is always the `CmdError` — switch on
  `data.code`.
* `events.subscribe {"topics":["command.*","tab.*"]}` / `events.unsubscribe {"topics":[…]}` (no
  topics = all) → `{"topics":[…now active…]}`. Patterns: exact, `prefix.*`, or `*`.
* New connections start subscribed to `tab.*`, `tabs.*`, `agents.*`, `workspace.*`, `runs.*`
  (what the UI renders; the UI client never calls `events.subscribe`). `command.*` must be asked for.
* Notifications: `{"jsonrpc":"2.0","method":"<topic>","params":{…event data…,"seq":12}}`.
  A connection that fell behind gets `{"method":"bus.lagged","params":{"missed":N}}` and should
  re-read state.

## Event topics

Emitted today (by the registry, `crates/core/src/command/registry.rs`), for every caller
(MCP, REST, WS, …; `caller` is e.g. `{"kind":"ws","conn":1}`, `{"kind":"rest"}`):

| Topic | Data |
|---|---|
| `command.started` | `{call_id, name, caller, args, tab}` |
| `command.finished` | `{call_id, name, caller, duration_ms, ok, error}` (`error` = code or null) |
| `command.rejected` | `{call_id, name, caller, args, error, message}` (unknown command, bad args, tab locked) |
| `bus.lagged` | `{missed}` — WS only, generated per connection |

**Not emitted yet:** tab lifecycle events. They will come from the CEF layer (`crates/cef`
emitting on the `EventBus`); the WS server forwards them unchanged. The UI
(`ui/src/lib/api/types.ts`, `ApiEvents`) expects exactly these payloads:

```ts
'tab.created':   { tab: TabInfo; index: number };
'tab.updated':   { id: TabId } & Partial<Omit<TabInfo, 'id'>>;
'tab.closed':    { id: TabId };
'tab.activated': { id: TabId };
'tabs.reset':    { tabs: TabInfo[]; groups: TabGroup[]; activeId: TabId | null };
'agents.updated':      { agents: Agent[] };
'workspace.activated': { id: WorkspaceId };
'runs.updated':        { runs: Run[]; patches: Patch[] };

// UI TabInfo (camelCase!):
{ id: number; title: string; url: string; loading: boolean; favicon: string | null;
  canGoBack: boolean; canGoForward: boolean; groupId: string | null; lockedBy: string | null }
```

Note the WS server adds `seq` to every payload object.

## Security (`src/guard.rs`)

* Bound to 127.0.0.1 only.
* `Host` must be `localhost`, `127.0.0.1` or `[::1]` (any port) on every route — blocks DNS
  rebinding. (rmcp checks it itself for `/mcp`.)
* `Origin`: absent → allowed (curl, scripts, the script host). `bmcp://ui` → allowed, with
  `Access-Control-Allow-Origin` and preflight support. `http://localhost:5173` /
  `http://127.0.0.1:5173` → allowed only with `BMCP_DEV_UI=1` (Vite dev server). Anything else
  → 403 `{"code":"FORBIDDEN",…}`. `/mcp` keeps refusing every `Origin`.
