# BrowserMCP UI

The browser's own chrome (tabs, omnibox, panels, Ctrl+K) and the new tab page.
Svelte 5 (runes) + Vite + TypeScript, no UI kit, icons are inline SVG. In the real browser it is
served from `bmcp://ui/` inside a CEF BrowserView; in development it runs in any browser against
an in-memory mock of the Rust core.

## Run

```bash
pnpm install            # once, from the repo root or ui/
pnpm dev                # http://localhost:5173 — chrome; /newtab.html — new tab page; /library.html — library
pnpm build              # -> ui/dist (index.html + palette.html + newtab.html + library.html, relative paths)
pnpm check              # svelte-check, fails on warnings
pnpm test               # vitest: fuzzy matcher, keymap, tab reducer, omnibox input, AI stream
                        # folding, net text parsers, request filter, rule/resume payloads,
                        # bookmark tree / drop positions, history day grouping, import wizard
```

From the repo root: `pnpm ui:dev`, `pnpm ui:build`, `pnpm ui:check`, `pnpm ui:test`.

### Which backend

The UI talks to the core **only** through `BrowserApi` (`src/lib/api`). The implementation is picked
at startup:

| How | Backend |
|---|---|
| default | `MockApi` — in-memory tabs, history, loading, agents browsing their own locked tabs, run progress |
| `?api=ws` | `WsApi` — JSON-RPC 2.0 over `ws://127.0.0.1:47800/v1/ws` |
| `?api=ws&ws=ws://host:port/v1/ws` | `WsApi` at another URL |
| `window.__BMCP__.ws` (injected) | `WsApi` — what the real browser uses: every `bmcp://ui/` page gets `<script>window.__BMCP__={"ws":"ws://127.0.0.1:<port>/v1/ws"}</script>` |
| `VITE_BMCP_API=ws` (build time) | `WsApi` |

### Inside the browser

`ui/dist` is embedded into `browsermcp.exe` at compile time (`crates/cef/build.rs`,
`src/ui_files.rs`); without a `ui/dist` a "UI not built" placeholder is embedded. So: `pnpm -C ui
build`, then build Rust. Faster loops:

* `browsermcp --ui-dir ui/dist` serves the files from disk (rebuild the UI, reload, no Rust build);
* `BMCP_DEV_UI=1 browsermcp --ui-dev http://localhost:5173` loads chrome and palette from Vite
  (hot reload; the new tab page still comes from `bmcp://ui/`).

In dev, `window.__bmcp.api.call('tabs.list')` works in the devtools console.

## Layout

```
ui/
├─ index.html            chrome entry
├─ palette.html          Ctrl+K palette alone (the browser shows it in an overlay view)
├─ newtab.html           new tab page entry (Vite multi-page)
├─ library.html          bookmarks / history / downloads / passwords / Chrome import (full tab)
└─ src/
   ├─ chrome/            App.svelte + main.ts for the chrome
   ├─ palette/           PaletteApp.svelte + main.ts for the overlay palette
   ├─ newtab/            NewTab.svelte, widgets/ (prompt, agents & runs, quick launch, sites)
   ├─ library/           Library.svelte + one component per section, FolderTree, ImportWizard, Dialog
   └─ lib/
      ├─ api/            types.ts (wire types + RpcMethods/ApiEvents), client.ts (BrowserApi),
      │                  mock.ts (MockApi), ws.ts (WsApi), index.ts (createApi)
      ├─ state/          tabs.ts (pure reducer + helpers), chrome.svelte.ts (ChromeController)
      ├─ data/           tree.ts (bookmark tree), history.ts (day groups, clear ranges), wizard.ts
      │                  (import state machine), library.ts (open/section/first run), edit.ts,
      │                  chrome-bookmarks.svelte.ts (bookmarks bar + omnibox star)
      ├─ components/     TabStrip, TabSidebar, TabItem, GroupLabel, Toolbar, Omnibox, WorkspaceSwitcher,
      │                  CommandPalette, SidePanel, panels/, AgentBanner, PageSlot, SettingsMenu, Icon…
      ├─ native.ts       window-local plumbing inside the browser (bmcp://cmd/: layout, palette, focus)
      ├─ keymap.ts       every shortcut: DEFAULT_KEYMAP + parser + keymap.json override merge
      ├─ commands.ts     palette entries (later: from the core's Command Registry)
      ├─ fuzzy.ts        palette matcher
      ├─ settings.svelte.ts  theme / accent / tab layout / panel / widgets (localStorage, try/catch)
      ├─ urls.ts         omnibox input -> URL or search
      └─ styles/app.css  design tokens (CSS custom properties on :root), light + dark
```

## How it fits the browser

- **Page area.** The chrome never draws web pages. `PageSlot` renders an empty
  `<div class="page-slot">` and reports its bounds whenever they change. Inside the browser the UI
  view fills the whole window and the tab's page view is placed exactly over the slot, so vertical
  tabs, the side panel and the agent banner simply move the page. The report goes over
  `bmcp://cmd/layout` (`native.ts`), because only that channel tells the browser which window it
  came from; in mock mode it is `ui.layout` (a no-op in the core). In mock mode the slot shows a
  placeholder, and the new tab page in an iframe.
- **Overlap.** Anything the chrome draws over the page slot (menus, the toast) is hidden under the
  page in the real browser. So the palette is its own page (`palette.html`) in a native overlay
  view, and the toast moves to the top of the window there. Dropdown menus that reach into the
  page area are still clipped (follow-up).
- **State.** The core is the source of truth. `ChromeController` folds events
  (`tab.created/updated/closed/activated`, `tabs.reset`, `agents.updated`, `workspace.activated`)
  through the pure `reduceTabs`, and every user action is an RPC call. Tab activation is optimistic.
- **Agents.** A tab with `lockedBy` belongs to an agent: its group label is filled with the agent's
  color and shows a lock, the tab shows a lock, and a banner offers **Take over** (`tabs.unlock`).
- **Shortcuts.** `keymap.ts` is the only table. Matching uses `KeyboardEvent.code` for letters and
  digits so Ctrl+T works on Cyrillic layouts. Keys pressed in a web page (or in the palette) are
  caught by the browser (`OnPreKeyEvent`, a mirror of `DEFAULT_KEYMAP` in
  `crates/cef/src/chrome/protocol.rs`) and handed to the chrome as `window.bmcp.run('tab.new')`, so
  they run through `ChromeController.run` exactly like keys pressed in the chrome. Only Ctrl+K, Esc
  and Ctrl+L/Alt+D/F6 are handled natively (overlay, focus). A desktop browser in dev swallows some
  (Ctrl+T/W/Tab), use the palette there.
- **New tab page.** A separate page with its own API connection. Widgets toggle under *Customize*.
  In dev, inside the chrome's iframe, it gets its own mock instance (so its agent state is not
  shared with the chrome's mock) and forwards shortcuts and link clicks to the chrome via
  `postMessage`.

## API

```ts
interface BrowserApi {
  readonly kind: 'mock' | 'ws';
  readonly status: 'connecting' | 'open' | 'closed';
  connect(): Promise<void>;
  close(): void;
  call<M extends keyof RpcMethods>(method: M, ...params): Promise<RpcMethods[M]['result']>;
  on<E extends keyof ApiEvents>(event: E, handler: (payload: ApiEvents[E]) => void): () => void;
  onStatus(handler: (s: ConnectionStatus) => void): () => void;
}
```

Methods: `tabs.list | open | close | activate | unlock`, `nav.goto | back | forward | reload | stop`,
`agents.list`, `workspaces.list | activate`, `runs.list | patch.approve | patch.reject`,
`scripts.list | run`, `data.bookmarks | top_sites`, `bookmarks.add | add_folder | remove | move | search`,
`history.search | delete`, `downloads.list`, `passwords.list | get | save | delete`,
`import.chrome_detect | chrome` (`types-data.ts`, mocked statefully in `mock-data.ts`), `ui.layout`, and for the side panels
`ai.providers | provider_set | provider_test | session_start | session_send | session_stop |
sessions_list | session_transcript`, `term.open | write | resize | close | list`,
`net.capture | list | get | search | ws_frames | har_export | rule_add | rule_list | rule_remove |
breakpoint | paused_list | resume`. Events: `tab.*`, `tabs.reset`, `agents.updated`, …, `ai.message
| tool_call | tool_result | done | session`, `term.output | exit`, `net.paused | pause_timeout |
intercept_error | lagged`. Full params and results are in `src/lib/api/types.ts` (they mirror the Rust
structs; will be replaced by the generated `@browsermcp/protocol`). `mock-panels.ts` fakes the
panel commands so `pnpm dev` demos the AI chat, a toy shell and live traffic without a backend.

`WsApi` sends `events.subscribe {"topics":["ai.*","term.*","net.*"]}` (a notification) after every
(re)connect, on top of the server's default topics.

### Library (bookmarks, history, downloads, passwords, import)

`bmcp://ui/library.html` is a full-tab page (content tabs may load exactly this URL, no query).
The chrome opens it with Ctrl+Shift+O (bookmarks), Ctrl+H (history), Ctrl+J (downloads), the
toolbar's folder button, the menu or the palette. The URL carries no section: the chrome writes
`bmcp.library.request` `{section, folder?, at}` to localStorage (same origin) and opens or
activates the library tab; the page takes the request on load or on the `storage` event (stale
after 15 s). Without one it reopens the last section, Bookmarks the first time.

* **Bookmarks**: folder tree (`data.bookmarks` per folder, level by level, `tree.ts`), list of the
  selected folder in the shared folder/bookmark order, drag rows to reorder or onto a folder (tree
  or row middle) to move (`bookmarks.move`, index without the moved node), add / edit / delete,
  search (`bookmarks.search`). There is no rename command: edits are add + move + remove
  (`edit.ts`), so an edited node gets a new id.
* **History**: search, grouped by local day, remove one page (`history.delete {url}`), remove a
  day, clear a range: "older than 4 weeks" / "all time" use `{before}`; recent ranges remove each
  page visited since then (with all its visits, the core has no "visits after" delete).
* **Downloads**: `downloads.list`, polled while one is in progress. "Show in folder" is a stub.
* **Passwords**: listed without passwords; the eye asks for confirmation, then reads that one
  login with `passwords.list {origin, reveal:true}` and hides it again after 30 s. Add, edit (empty
  password field keeps the current one), delete with confirmation.
* **Import from Chrome** (`ImportWizard.svelte`, state machine in `wizard.ts`): profiles from
  `import.chrome_detect`, parts bookmarks+history / passwords CSV (with export instructions) /
  cookies JSON / nothing, per-part results of `import.chrome`. On first launch the chrome opens it
  once (`bmcp.import.offered` flag) when bookmarks and history are empty.

Chrome side: the omnibox star (`BookmarkStar`, Ctrl+D) adds the page to the bookmarks bar or
removes all its bookmarks; the bookmarks bar (`BookmarksBar`, Ctrl+Shift+B, on by default) shows
folder 1, its folders open in the library (a dropdown would be covered by the page). The core
sends no data events, so pages bump `bmcp.data.rev` in localStorage after edits and the bar/star
re-read on it (and on window focus). `VITE_BMCP_EMPTY_DATA=1` starts the mock empty (wizard).

### Side panels

* **AI** (`panels/AiPanel.svelte`, `ai/store.svelte.ts`, `ai/thread.ts`): provider picker from
  `ai.providers`, a settings sub-view (`AiSettings.svelte`: key / base URL / model / CLI path via
  `ai.provider_set`, free check via `ai.provider_test`), sessions as colored chips, one thread per
  session folded from `ai.*` events (`text_delta` streams, `text` replaces and closes the row; tool
  calls are collapsible rows), stop button. New chat = `ai.session_start`, reply = `ai.session_send`.
* **Terminal** (`TerminalPanel.svelte`, `XTermView.svelte`, `term/store.svelte.ts`): xterm.js +
  fit addon per terminal tab, shell picker (pwsh / cmd / bash), resize follows the panel
  (`term.resize`, debounced). Output is buffered from chrome start, also for terminals opened over
  REST or by agents, and replayed when a view attaches (without `ESC[6n` queries; the core answers
  ConPTY's first one itself).
* **Traffic** (`TrafficPanel.svelte`, `NetDetails`, `NetRules`, `NetBreakpoints`, `net/*.ts`):
  capture toggle for the active tab, `net.list` polled every second with a cursor (re-reading from
  the oldest pending request), virtualized list with filter (`method:post status:4xx -word`) and
  type chips, details (headers / request / response bodies via `net.get` parts, WS frames), HAR
  export to `data/har/`, rules (block / redirect / mock / rewrite body) and breakpoints with an
  editor that resumes with only the changed fields (`net.resume`). The net commands return compact
  text for agents; `net/parse.ts` reads it.
* **Recording** and **Runs** (`RecordingPanel`, `RecordingDetail`, `StepList`, `CodeView`,
  `RunsPanel`, `NewRunForm`, `RunDetail`, `TriggersView`; logic in `record/` and `runs/`): types
  come from the generated protocol (`api/types-record-runs.ts` adds them to `RpcMethods` /
  `ApiEvents` by declaration merging), fakes in `api/mock-record-runs.ts`. Record / stop the active
  tab (video, traffic), live steps from `record.step`, saved recordings, `record.codegen` (with
  parametrize) shown with Prism (lazy chunk), an inputs form inferred from `inputs.<name>` /
  `need("name")` and *Save & run* (`script.save` + `script.run`); runs with progress bars from
  `run.progress`, a paged results table (`run.results` cursors, status filter), pause / resume /
  cancel, export, the heal flow (`run.heal_needed` → patch editor → `run.heal_apply`) and cron
  triggers with a local preview of the next firings (`runs/cron.ts` follows the Rust `cron` crate:
  weekdays 1 = Sunday, day-of-month AND weekday). `WsApi` also subscribes `record.*`, `run.*`,
  `trigger.*`.

`VITE_BMCP_PANEL=traffic pnpm build` opens that panel on a profile without saved settings
(screenshots of a fresh profile).

Wire format: requests `{jsonrpc:"2.0", id, method, params}`, responses `{id, result}` or
`{id, error:{code, message, data}}`, events are notifications `{jsonrpc:"2.0", method:"tab.updated", params}`.
`WsApi` reconnects with backoff, queues calls until connected and rejects in-flight calls on disconnect.
