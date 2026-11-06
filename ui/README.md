# BrowserMCP UI

The browser's own chrome (tabs, omnibox, panels, Ctrl+K) and the new tab page.
Svelte 5 (runes) + Vite + TypeScript, no UI kit, icons are inline SVG. In the real browser it is
served from `bmcp://ui/` inside a CEF BrowserView; in development it runs in any browser against
an in-memory mock of the Rust core.

## Run

```bash
pnpm install            # once, from the repo root or ui/
pnpm dev                # http://localhost:5173 — chrome; /newtab.html — new tab page
pnpm build              # -> ui/dist (index.html + newtab.html, relative asset paths)
pnpm check              # svelte-check, fails on warnings
pnpm test               # vitest: fuzzy matcher, keymap, tab reducer, omnibox input
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
| `VITE_BMCP_API=ws` (build time) | `WsApi` (what the CEF build should use) |

In dev, `window.__bmcp.api.call('tabs.list')` works in the devtools console.

## Layout

```
ui/
├─ index.html            chrome entry
├─ newtab.html           new tab page entry (Vite multi-page)
└─ src/
   ├─ chrome/            App.svelte + main.ts for the chrome
   ├─ newtab/            NewTab.svelte, widgets/ (prompt, agents & runs, quick launch, sites)
   └─ lib/
      ├─ api/            types.ts (wire types + RpcMethods/ApiEvents), client.ts (BrowserApi),
      │                  mock.ts (MockApi), ws.ts (WsApi), index.ts (createApi)
      ├─ state/          tabs.ts (pure reducer + helpers), chrome.svelte.ts (ChromeController)
      ├─ components/     TabStrip, TabSidebar, TabItem, GroupLabel, Toolbar, Omnibox, WorkspaceSwitcher,
      │                  CommandPalette, SidePanel, panels/, AgentBanner, PageSlot, SettingsMenu, Icon…
      ├─ keymap.ts       every shortcut: DEFAULT_KEYMAP + parser + keymap.json override merge
      ├─ commands.ts     palette entries (later: from the core's Command Registry)
      ├─ fuzzy.ts        palette matcher
      ├─ settings.svelte.ts  theme / accent / tab layout / panel / widgets (localStorage, try/catch)
      ├─ urls.ts         omnibox input -> URL or search
      └─ styles/app.css  design tokens (CSS custom properties on :root), light + dark
```

## How it fits the browser

- **Page area.** The chrome never draws web pages. `PageSlot` renders an empty
  `<div class="page-slot">` and reports its bounds with `ui.layout {x, y, w, h, dpr}` whenever they
  change; the core places the CEF content view there. With the WS backend the body is transparent so
  this works both if the UI is one view around a see-through hole and if it is split into strips
  (spike S1). In mock mode the slot shows a placeholder, and the new tab page in an iframe.
- **State.** The core is the source of truth. `ChromeController` folds events
  (`tab.created/updated/closed/activated`, `tabs.reset`, `agents.updated`, `workspace.activated`)
  through the pure `reduceTabs`, and every user action is an RPC call. Tab activation is optimistic.
- **Agents.** A tab with `lockedBy` belongs to an agent: its group label is filled with the agent's
  color and shows a lock, the tab shows a lock, and a banner offers **Take over** (`tabs.unlock`).
- **Shortcuts.** `keymap.ts` is the only table. Matching uses `KeyboardEvent.code` for letters and
  digits so Ctrl+T works on Cyrillic layouts. In CEF the core will forward shortcuts from page
  views (OnPreKeyEvent); a desktop browser in dev swallows some (Ctrl+T/W/Tab), use the palette there.
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
`scripts.list | run`, `data.bookmarks | topSites`, `ai.prompt`, `ui.layout`. Full params and results
are in `src/lib/api/types.ts` and will be replaced by the generated `@browsermcp/protocol`.

Wire format: requests `{jsonrpc:"2.0", id, method, params}`, responses `{id, result}` or
`{id, error:{code, message, data}}`, events are notifications `{jsonrpc:"2.0", method:"tab.updated", params}`.
`WsApi` reconnects with backoff, queues calls until connected and rejects in-flight calls on disconnect.
