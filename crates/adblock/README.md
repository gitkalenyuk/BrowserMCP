# bmcp-adblock

Resource saving for BrowserMCP (plan Phase 4: M4.6 adblock, M4.8 reader/simplify).
No CEF dependency; everything is unit-tested with `bmcp_core::testing::FakeBackend`.

| Piece | File | What it does |
|---|---|---|
| `RequestFilter` | `src/engine.rs` | Brave `adblock` =0.13.5 engine: `check(url, source_url, type) -> Decision {block, redirect, important, …}`, `cosmetic(url) -> CosmeticResources {hide_selectors, style, scriptlets}`, `generic_selectors(classes, ids)`, `serialize`/`from_serialized` |
| filter lists | `src/lists.rs` | EasyList, EasyPrivacy (default), uBlock filters (optional), `data/filters/user.txt` (own rules). Conditional GET (`If-None-Match`/`If-Modified-Since`), compiled engine cached in `data/filters/engine.dat` |
| site policies | `src/policy.rs` | `data/config/simplify.json`: `{js, images, fonts, media, third_party, animations}` per host pattern (`*`, `example.com` incl. subdomains), overlaid from general to specific. `data/config/adblock.json`: on/off, allowlist, enabled lists |
| `ResourceGate` / `Gate` | `src/gate.rs` | the one per-request decision (policy first, then filter lists), per-tab stats. CEF-agnostic, built for CEF's IO thread |
| interim enforcement | `src/interim.rs` | **temporary**: CDP `Fetch` interception + cosmetic CSS for `adblock.apply`, until the CEF hook below exists |
| reader mode | `src/reader.rs`, `assets/` | Mozilla Readability 0.6.0 (Apache-2.0, `assets/Readability.js` + `assets/Readability-LICENSE.md`) in an isolated world |

## Commands

| Command | Toolset | |
|---|---|---|
| `adblock.update_lists {force?, lists?}` | data | download/update lists, recompile; per-list `updated`/`not_modified`/`failed` |
| `adblock.toggle {host?, on}` | data | global switch, or per-site allowlist (`on: false` = allow ads on that site) |
| `adblock.stats {tab?, reset?}` | data | per-tab counters (blocked by filter/policy, by type, by host, bytes) + interception overhead |
| `adblock.check {url, source_url?, type?}` | data | debug: decision, filter verdict, page policy (not counted) |
| `adblock.apply {tab?, on}` | data | interim enforcement in one tab (CDP `Fetch`); downloads lists on first use |
| `simplify.site {host, js?, images?, fonts?, media?, third_party?, animations?, clear?}` | data | store a site policy (true = allow, false = block) |
| `simplify.get {host}` | data | effective policy, matched patterns, CEF content settings, adblock on? |
| `simplify.list` | data | all policies + adblock settings |
| `simplify.measure {url, tab?, settle_ms?, timeout_ms?}` | data | load a page with the cache off; requests, blocked, bytes over the wire, load/DCL time, JS heap, DOM nodes, by type (compare with/without `adblock.apply`) |
| `simplify.reader {tab?, apply?, max_chars?}` | core | `{title, byline, text (cut), length}`; `apply` replaces the page with a clean dark/light-aware version |

## Threading model

`adblock::Engine` is built with the crate's default `single-thread` feature, so it is
`!Send`/`!Sync`. The shared `Gate` (`Send + Sync`, kept as `Arc<Gate>`) holds an immutable
`GateState` snapshot: serialized engine bytes (`Arc<Vec<u8>>`), settings, policies. Each thread
that asks the gate keeps, in a `thread_local!`, the snapshot it last saw plus its own
`RequestFilter` deserialized from the bytes (milliseconds; done once per thread and engine).
A request costs one atomic load to check that the snapshot is still current — no lock — plus
the engine lookup (microseconds). Changing settings/policies/lists swaps the snapshot; every
thread notices on its next request. Only the stats counters take a short mutex.

## CEF hook (follow-up for the CEF crate — not done here)

The interim CDP path pays one IPC round trip per request and misses service workers and
out-of-process iframes. The real enforcement is in CEF's network stack, on the IO thread:

1. **Construct the gate before the browser starts** (it must exist for the first request):
   `let adblock = Arc::new(AdblockService::new(data.root()))`, put the same instance into the
   registry (`registry.extensions().insert(...)` — or keep the `Arc` and insert a clone), and
   hand `adblock.gate.clone() as Arc<dyn ResourceGate>` to `bmcp_cef::LaunchOptions` (new field
   `resource_gate: Option<Arc<dyn ResourceGate>>`). Call `adblock.ensure_engine(false)` on the
   tokio runtime at startup so `engine.dat` is loaded without waiting for a command.
2. **`CefRequestHandler::GetResourceRequestHandler`** (UI/IO thread, per request; cef-rs
   `ImplRequestHandler::resource_request_handler`): return one shared
   `CefResourceRequestHandler` that holds the `Arc<dyn ResourceGate>` (or `None` for
   `bmcp://` UI browsers, which need no filtering).
3. **`CefResourceRequestHandler::OnBeforeResourceLoad(browser, frame, request, callback)`**
   (IO thread; cef-rs `ImplResourceRequestHandler::on_before_resource_load`):
   ```rust
   let info = RequestInfo {
       tab: browser.map(|b| TabId(b.identifier())),          // None: service worker etc.
       url: &request.url(),
       // Top-level page URL. For RT_MAIN_FRAME it is ignored (the request is the page).
       // `browser.main_frame().url()` (CefFrame is usable on any thread); fall back to
       // `request.referrer_url()` / `request_initiator` when there is no browser.
       source_url: &top_level_url,
       resource_type: ResourceType::from_cef(request.resource_type() as i32),
   };
   match gate.on_request(&info).action {
       GateAction::Allow => ReturnValue::CONTINUE,  // RV_CONTINUE
       GateAction::Block => ReturnValue::CANCEL,    // RV_CANCEL: the request fails (ERR_ABORTED)
       // Remember the decision (by request.identifier()) and serve the data: body from
       // GetResourceHandler with a CefResourceHandler; a data: redirect would be refused.
       GateAction::Redirect => ReturnValue::CONTINUE,
   }
   ```
   Never block or await here: `on_request` is synchronous and lock-free on the fast path.
4. **`CefResourceRequestHandler::OnResourceLoadComplete(..., status, received_content_length)`**
   → `gate.on_complete(tab, received_content_length as u64)` (bytes in `adblock.stats`).
5. **Content settings** (`simplify.get` → `content_settings`): on navigation commit (or when a
   policy changes) call `CefRequestContext::SetContentSetting(url, url,
   CEF_CONTENT_SETTING_TYPE_JAVASCRIPT | CEF_CONTENT_SETTING_TYPE_IMAGES,
   CEF_CONTENT_SETTING_VALUE_BLOCK/ALLOW)` for the site, replacing
   `Emulation.setScriptExecutionDisabled` of the interim path.
6. **Cosmetic filtering stays on CDP**: `Page.addScriptToEvaluateOnNewDocument` in the
   isolated world `bmcp-adblock` + the `DOMContentLoaded` generic pass (see `interim.rs`); once
   the hook exists, `adblock.apply` keeps only that part (no `Fetch.enable`).

### Resource type mapping

`ResourceType::from_cef` (numbers are Chromium's, never renumbered) and the adblock request
type each one is checked as (`ResourceType::adblock_name`):

| CEF `cef_resource_type_t` | value | `ResourceType` | adblock type (`$option`) |
|---|---|---|---|
| `RT_MAIN_FRAME` | 0 | `main_frame` | `document` (never blocked by the gate) |
| `RT_SUB_FRAME` | 1 | `sub_frame` | `sub_frame` (`$subdocument`) |
| `RT_STYLESHEET` | 2 | `stylesheet` | `stylesheet` |
| `RT_SCRIPT` | 3 | `script` | `script` |
| `RT_IMAGE` | 4 | `image` | `image` |
| `RT_FONT_RESOURCE` | 5 | `font` | `font` |
| `RT_SUB_RESOURCE` | 6 | `other` | `other` |
| `RT_OBJECT` | 7 | `object` | `object` |
| `RT_MEDIA` | 8 | `media` | `media` |
| `RT_WORKER` | 9 | `worker` | `script` |
| `RT_SHARED_WORKER` | 10 | `worker` | `script` |
| `RT_PREFETCH` | 11 | `prefetch` | `other` |
| `RT_FAVICON` | 12 | `favicon` | `image` |
| `RT_XHR` (XHR and `fetch()`) | 13 | `xhr` | `xmlhttprequest` |
| `RT_PING` | 14 | `ping` | `ping` |
| `RT_SERVICE_WORKER` | 15 | `worker` | `script` |
| `RT_CSP_REPORT` | 16 | `csp_report` | `csp_report` |
| `RT_PLUGIN_RESOURCE` | 17 | `object` | `object` |
| `RT_NAVIGATION_PRELOAD_MAIN_FRAME` | 19 | `main_frame` | `document` |
| `RT_NAVIGATION_PRELOAD_SUB_FRAME` | 20 | `sub_frame` | `sub_frame` |

WebSocket handshakes do not pass `OnBeforeResourceLoad` in CEF; they are only filtered on the
interim CDP path (`Network.ResourceType` `WebSocket` → `websocket`).

CDP `Network.ResourceType` (interim path, `ResourceType::from_cdp`): `Document` →
`main_frame` (main frame id) / `sub_frame`, `Stylesheet`, `Script`, `Image`, `Font`,
`Media`/`TextTrack` → `media`, `XHR` → `xhr`, `Fetch`/`EventSource` → `fetch`, `Ping`,
`CSPViolationReport` → `csp_report`, `WebSocket`, `Prefetch`, everything else → `other`.

## Interim path details (`adblock.apply`)

* Subscribes to the tab's CDP events, then `Fetch.enable {patterns: [{urlPattern: "*",
  requestStage: "Request"}]}` and `Page.enable`.
* `Fetch.requestPaused` → gate → `Fetch.failRequest {errorReason: "BlockedByClient"}` /
  `Fetch.fulfillRequest` (redirect) / `Fetch.continueRequest`. If a block answer fails, it
  still tries `continueRequest`; if events were lost (`Lagged`), it re-enables Fetch, which
  releases every paused request.
* Paused requests are answered with browser-side `Fetch.*` calls only. Renderer-handled
  commands (`Page.addScriptToEvaluateOnNewDocument`, `Runtime.*`, `Emulation.*`) are held
  back by Chromium while a main-frame navigation is pending, so awaiting one before
  continuing the navigation request deadlocks until the 30 s CDP timeout (seen live).
* When the main frame commits (`Page.frameNavigated`): the host's hide CSS is registered with
  `Page.addScriptToEvaluateOnNewDocument {worldName: "bmcp-adblock"}` (the previous one is
  removed; later same-host documents get it at document start), it is injected into the new
  page at once, and the site policy's `js`/`animations` are applied with
  `Emulation.setScriptExecutionDisabled` / `Emulation.setEmulatedMedia` (so for the first
  document of a host, inline scripts may run before JS is switched off — the CEF content
  setting fixes that).
* The current document gets the CSS via `Page.createIsolatedWorld` + `Runtime.evaluate
  {contextId}` (no `DOM.*`, no `Runtime.enable`), and again on `Page.domContentEventFired` with
  the generic class/id rules for the classes/ids found on the page.
* Overhead is measured per session (`adblock.stats` → `interception`: decision µs, round trip
  ms, lagged events).

## Live numbers (2026-11-06, interim CDP path, EasyList + EasyPrivacy)

`simplify.measure` (HTTP cache off, 5 s settle), dev build with `adblock`/`regex` deps at
opt-level 3, two rounds each:

| Page | Run | Requests | Blocked | Bytes | Load event | DOMContentLoaded |
|---|---|---|---|---|---|---|
| theverge.com | without | 236 / 241 | 0 | 5.34 / 5.43 MB | >45 s (timeout) / 21.0 s | 6.5 / 0.9 s |
| theverge.com | `adblock.apply` | 184 / 183 | 10 / 10 | 3.01 / 3.01 MB (−44%) | 22.9 / 12.4 s | 1.1 / 1.1 s |
| edition.cnn.com | without | 103 / 102 | 0 | 23.3 / 23.5 MB | 4.4 / 2.1 s | 2.8 / 1.3 s |
| edition.cnn.com | `adblock.apply` | 98 / 94 | 9 / 8 | 22.9 / 22.9 MB (−2%) | 2.1 / 2.1 s | 1.4 / 1.4 s |
| edition.cnn.com | + `simplify.site cnn.com {images,media,fonts: false}` | 106 | 71 | 5.2 MB (−78%) | 2.4 s | 1.8 s |

Interception overhead (`adblock.stats` → `interception`): decision avg 0.4–0.5 ms (max 10 ms
incl. building the per-thread engine), answer round trip avg 1.5–12 ms, max 40 ms, 0 lost
events, 0 errors. Blocking the ad scripts also prevents the requests they would have made, so
"requests" drops by more than "blocked". In a plain dev build (adblock unoptimized) the
decision averaged 31 ms — the workspace should add
`[profile.dev.package.adblock] opt-level = 3` (and for `regex*`).

Reader mode on https://en.wikipedia.org/wiki/Ad_blocking: 790 ms, title "Ad blocking",
45 635 chars of text; `apply: true` renders the clean dark/light page.

## Known gaps

* No redirect/scriptlet resources (uBlock `resources.json`) are loaded yet, so `$redirect`
  rules block instead of redirecting and `##+js(...)` scriptlets are not run.
* Procedural cosmetic filters (`:has-text`, `:upward`, …) are not applied (only plain hide
  selectors).
* Interim path: only the page target is intercepted — OOPIF iframes and service workers are
  not; another component enabling `Fetch` on the same tab (e.g. `bmcp-net` rules/breakpoints)
  would conflict: both would answer the same paused requests.
* A strict page CSP (`style-src` without `'unsafe-inline'`) can block the injected `<style>`;
  reader mode additionally uses a constructable stylesheet for that case.
