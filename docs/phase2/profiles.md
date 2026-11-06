# CEF hooks: profiles, isolation, adblock gate, cookies, downloads, permissions, identity, CDP port

Date: 2026-11-06. CEF 156 Alloy, Windows 11. Follow-ups requested by `crates/adblock`,
`crates/identity`, `crates/data` and `crates/jobs`. CEF-free logic lives in the new crate
`crates/profiles` (bmcp-profiles, unit-tested); the CEF side is in `crates/cef/src/`:
`profiles.rs`, `osr.rs`, `net.rs`, `cookies.rs`, `downloads.rs`, `hooks.rs`.

## What exists now

| Item | Commands / API | CEF hook |
|---|---|---|
| Profiles | `profile.list`, `profile.create {id, proxy?, forward?, proxy_bypass?, identity?, accept_language?}`, `profile.delete {id, keep_data?}`, `tabs.open {profile?, incognito?, window?: "current"\|"new"\|"offscreen", run?}` | one `CefRequestContext` per profile (`data/profiles/<id>`; incognito = no cache path), prefs via `SetPreference`; proxy passwords only in the local forwarder (`forward: false` = password-less proxies) |
| Parallel isolation | `tabs.open {window: "offscreen"}`, `run.start {profile: "isolated"}` | windowless (OSR) browsers with their own size and focus |
| Adblock gate | `adblock.*` unchanged; `adblock.apply` now only does the cosmetic part (`report.network = "cef"`) | `GetResourceRequestHandler` → `OnBeforeResourceLoad` (`RV_CANCEL`), `GetResourceHandler` (redirect body), `OnResourceLoadComplete` (bytes), content settings for `js`/`images` before the navigation commits |
| Cookies | `cookies.get {profile?, incognito?, domain?}`, `cookies.set {…, cookies}`, `cookies.delete {…, domain?, name?}`, `cookies.apply_pending` | `CefCookieManager` visitor / `SetCookie`; `pending_cookies` are applied at startup and after every `import.chrome` |
| Downloads | `downloads.list` (filled now), events `download.started/progress/done` | `DownloadHandler`: no dialog, `data/runs/<run>/downloads/` or `data/downloads/` |
| Permissions | `permissions.set {origin, kind, allow?}`, `permissions.list`, `permissions.pending`, `permissions.answer` | `OnShowPermissionPrompt`, `OnRequestMediaAccessPermission`: rules, else the permission bar asks (window tabs) / deny (off-screen); `bmcp://ui` keeps its loopback grant, also in content tabs |
| Identity auto-apply | `profile.create {identity}` / `identity.assign` | new tabs and popups of such a profile start on `about:blank`, get `stealth.fingerprint_set`, then load; workers via `Target.setAutoAttach {waitForDebuggerOnStart, flatten}` |
| External CDP | `browsermcp --cdp-port <n>` | `Settings.remote_debugging_port`; refused while any profile has an identity |

## Automation cannot close the last window

`tabs.close` from an agent, script, MCP/REST/WS client or run never closes a window: if the
tab is the last one of its window, the new tab page is opened there first (and the close
waits until it exists); in the single layout the tab shows the new tab page instead. Only a
person (`Caller::Ui` — the browser UI, whose WebSocket now carries `Origin: bmcp://ui` and is
treated as the UI — or `Caller::Keymap`) closing the last tab closes the window, as before.
Every `tabs.close` (caller, tab), window close request, "last tab closed → window closes" and
`quit` is logged at INFO, so an unexplained shutdown can be traced in the logs. Decision
functions: `core::commands::tabs::keeps_window`, `cef::chrome::keep_window_plan` (tested).

## The parallel-rows bug (crates/jobs)

Reproduced before the change with `login.ts × logins.csv`, `parallel: 3`: 7 of 10 rows failed
with `NOT_INTERACTABLE: tab N is hidden, so the browser ignores input to it`. Cause: all rows'
tabs live in one window, only the shown tab is visible; a background tab is a *hidden* page
(`visibilityState: "hidden"`, no frames, input dropped), and newly opened background tabs never
got a size at all. Typing with `DOM.focus` + `Input.insertText` did reach the right tab even
for hidden tabs (checked with 3 and 4 concurrent tabs) — the visibility, not the routing, was
the problem.

Fix:
* `tabs.open {window: "offscreen"}`: a windowless (off-screen rendering) browser, 1280×800,
  always visible, with its own focus (`SetFocus(true)`), nothing on the desktop. `tabs.close`
  closes it; all of them close when the last real window closes.
* `run.start {profile: "isolated"}`: each row attempt's script host gets
  `BMCP_TAB_DEFAULTS={"profile": "<run>-r<row>-a<attempt>", "incognito": true, "window":
  "offscreen", "run": "<run>"}`, merged into its `tabs.open` calls (script keys win) — a fresh
  in-memory profile and off-screen tabs per row, no script change needed.
* `window: "new"` opens a composed window of its own; background tabs of a window now get the
  page area's size even while hidden (so they have a layout), but they stay hidden.

## Live checks (this machine)

| Check | Result |
|---|---|
| `run.start login × 10 rows, parallel 3, profile "isolated"` | 10/10 correct: 7 `Welcome <own email>`, 3 expected `Wrong password` (cyd, eve, hal); 53.8 s, 16.5 s on the final build |
| 3 concurrent tabs `window: "offscreen"`, `page.act fill` email+password each | every tab has its own values |
| `profile.create work` + tabs in `work`, default and incognito; `cookies.set {profile: work}` | cookie only in `work` (default and incognito jars empty); `profile.delete work` refused while its tab is open; the incognito context is released with its last tab |
| Identity `idw` (4 cores, 8 GB, fr-FR) on profile `stealthy`, page with a dedicated worker | page AND worker: cores 4, memory 8, languages `fr-FR,fr,en-US,en`, header `Accept-Language: fr-FR,fr;q=0.9,…`; default profile: 12 / 32 / en-US in both |
| `profile.create work {accept_language: "de-DE,de"}`, request from a `work` tab | header `Accept-Language: de-DE,de;q=0.9` (default profile: `en-US,en;q=0.9`) |
| `profile.create proxied {proxy: "http://u:p@127.0.0.1:8768", proxy_bypass: "<-loopback>"}` (test proxy with Basic auth) | `proxy_server: socks5://127.0.0.1:<port>` (forwarder); the page loads and the proxy logs `CONNECT 127.0.0.1:8767` with valid credentials; `proxy` + `webrtc.ip_handling_policy` prefs set |
| `--cdp-port 9333` while profile `stealthy` has an identity | refused: `--cdp-port refused: these profiles use a stealth identity`, port closed |
| Popup (`target=_blank`) from a `stealthy` off-screen tab | re-opened as a prepared off-screen tab: worker patched too |
| Download, off-screen tab with `run: run-live-2` | `data/runs/run-live-2/downloads/report.csv`, `downloads.list` row `complete`, 200000 bytes |
| `getUserMedia({video})` in a window tab, no rule | `NotAllowedError` (handler: `media permission … allow=false`) |
| Playwright 1.49 `chromium.connectOverCDP("http://127.0.0.1:9333")` | lists the pages (incl. our `bmcp://ui` views), `selectOption` + `check` + `click` on form.html → `Saved: country=pl news=yes` |
| UI under the new CSP (via the CDP port) | renders and is connected (tab strip shows the page title); an injected inline script is blocked (`script-src-elem`), `new WebSocket("wss://example.com")` is blocked (`connect-src`) |
| Adblock gate, theverge.com (EasyList + EasyPrivacy), off-screen tab | gate stats for one load: 205 requests, 21 blocked (14 scripts, 6 XHR, 1 ping; doubleclick, amazon-adsystem, bounceexchange, google-analytics, …); `simplify.measure` round 2: 4.11 MB → 2.86 MB over the wire (−30 %), DOMContentLoaded 31.7 s → 19.0 s (slow, shared test machine; round 1 was noise) |

`simplify.measure` counts only `ERR_BLOCKED_BY_CLIENT` failures; requests cancelled in CEF fail
with `ERR_ABORTED`, so its `blocked` column stays 0 — use `adblock.stats` for the gate.

## Pitfalls found

* **`SetPreference` "always fails"**: cef-rs passes `CefString::default()` as a NULL pointer,
  and CEF's C wrapper returns 0 when the `error` out-parameter is NULL. Pass a non-empty
  string (`CefString::from(" ")`) so CEF has somewhere to write the error.
* **Profile paths on Windows**: a context's `cache_path` must be a direct child of
  `root_cache_path`, compared as strings: `C:/data\profiles\work` (forward slashes from
  `--data-dir` plus joined backslashes) → "Cannot create profile at path". All paths handed to
  CEF are normalized (`native_path`).
* **Browsers in a fresh profile**: `CreateBrowserSync` and `browser_view_create` fail (None)
  while the new request context is still initializing. Tab creation waits for
  `OnRequestContextInitialized` (`profiles::when_ready`); off-screen browsers use the async
  `CreateBrowser`, and their pending intent is found through a key stored in the browser's
  client (there is no view id).
* **Accept-Language** is set twice: `RequestContextSettings.accept_language_list` when the
  context is created and the `intl.accept_languages` pref (also for later changes).
* **No `GetAuthCredentials` for proxies**: with an authenticated HTTP proxy set directly in
  the `proxy` pref, CEF 156 never called `CefRequestHandler::GetAuthCredentials` (logged);
  Chromium's own login handling took over and the browser crashed (segfault) shortly after
  the 407. So passwords always go through the forwarder; `profile.create {forward: false}`
  with a password is refused.
* **Downloads need the tab**: CEF reports progress through the browser's client; closing the
  tab right after a download starts leaves the row `in_progress`.
* **Transition type flags**: `Request::transition_type()` returns a Rust enum, but the value
  is a source OR-ed with flags (not a declared variant); it is read through the C function as
  an integer.
* Doc comments inside `wrap_*!` macros do not compile; use `//` comments there.

## Known gaps

* Identity tabs share `Target.setAutoAttach` (one setting per session) with the OOPIF support
  of `crates/browser/src/frames.rs` (`waitForDebuggerOnStart: false`, `flatten: true`): after a
  snapshot has switched it, new workers of that tab are still patched on attach, but no longer
  paused first, so a worker's very first lines may see the real values. Both use flattened
  sessions; our watcher resumes every target it paused (iframes included).
* The newer identity APIs from main (`profile_proxy_pref` / `proxy.rules_set` routes,
  `cef_prefs::profile_prefs`, `worker_commands`) are not wired; bmcp-profiles has its own
  proxy setting, prefs and worker patch.

* `getUserMedia` in an *off-screen* tab never settles (no permission callback arrives).
* `webrtc.multiple_routes_enabled` / `webrtc.nonproxied_udp_enabled` are not registered in
  this CEF build; only `webrtc.ip_handling_policy` is set.
* Off-screen tabs appear in `tabs.list` (with `active: true`) — now flagged `offscreen: true`, and
  the UI's tab strip leaves them out (`docs/phase2/chrome-features.md`).
* Popups re-opened for identity profiles / off-screen openers lose `window.opener`.
* Accept-Language of the global (launch) profile can only follow `profile.create {id:
  "default", accept_language}` once its context exists; identity tabs use the CDP override.
* After a restart, the first window of the launch profile may load before stored profile
  settings (proxy forwarder) are re-applied.
* `--cdp-port` exposes our own `bmcp://ui` pages (full API rights) to any local CDP client —
  that is why it is off by default.
