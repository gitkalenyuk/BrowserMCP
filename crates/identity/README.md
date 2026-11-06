# bmcp-identity

Hidden mode, Phase 10 foundations: fingerprint identities, stealth checks, device emulation,
proxy pools, leases, split routing (PAC) and a local proxy forwarder. No CEF dependency;
everything goes through `BrowserBackend::cdp` and is tested with `FakeBackend`. All commands
are in the `stealth` toolset (`/mcp?toolsets=core,stealth`).

## Commands

| Command | What |
|---|---|
| `identity.presets` | Device classes: `windows-1080p`, `windows-1440p`, `windows-laptop`, `windows-768`, `macos-mbp14`, `macos-mba13`, `android-pixel-8`, `android-galaxy-s23`, `iphone-15`, `ipad-air` |
| `identity.generate {os?, preset?, seed?, id?, save?, country?, timezone?, locale?, latitude?, longitude?}` | Consistent random identity, deterministic by seed |
| `identity.list` / `get` / `remove` / `assign {profile, id?}` | `data/config/identities.json` |
| `identity.fit_to_proxy {identity_id, proxy_id, profile?, save?, url?}` | Geo lookup through the proxy → timezone, locale + languages, geolocation (city + ~2 km jitter); keeps UA/platform/hardware; lists what changed |
| `stealth.fingerprint_set {tab?, identity_id \| identity \| preset, reload?}` | Apply to a tab |
| `stealth.fingerprint_clear {tab?}` | Remove again |
| `stealth.check {tab?}` | Pass/warn/fail table of fingerprinting probes (checklist below) |
| `device.emulate {tab?, device}` / `device.clear` | "iPhone 15", "Pixel 8", "Galaxy S23", "iPad" (layout testing) |
| `proxy.pool_add {pool, proxies, strategy?}` / `pool_list` / `pool_check` / `pool_remove` / `next` | `data/config/proxies.json`; `round_robin`, `random`, `sticky` (per run id) |
| `proxy.lease {pool, run_id, row, rotate?}` / `proxy.release {pool, run_id, row?}` | One proxy per mass-run row: sticky until released, `rotate` for another one; least-used proxy first |
| `proxy.rules_set {profile, rules: [{match, via}], default?}` | Split routing → PAC script + Chromium `proxy` pref; emits `proxy.routes_changed` |
| `proxy.rules_test {profile, url}` | Which rule/route a URL takes (same logic as the PAC script) |
| `proxy.forward_start {proxy \| pool \| proxy_id, run?}` / `forward_stop` / `forward_list` | Local no-auth SOCKS5+HTTP proxy on 127.0.0.1 chained to an authenticated upstream |
| `geo.lookup {proxy? \| pool?, url?}` | Exit IP → country, city, timezone, coordinates, suggested locale |

**Proxy references** (`via`, `default`, `proxy_id`): `direct`, `fwd-N` (running forwarder),
`pool:NAME` (a pool proxy; per profile it is leased under run `profile.<id>`, so the profile
keeps its IP), `lease:POOL/RUN/ROW` (from `proxy.lease`), or a proxy URL.

**Rule patterns** (`match`): host glob (`*.example.com`, which also matches `example.com`),
URL glob (`http://example.com/api/*`), or `re:REGEX` (searched in the URL; case-insensitive;
only syntax that means the same in Rust and JavaScript). Chromium hands PAC scripts only
`scheme://host:port/` of https URLs, so path rules only work for plain http. Every proxy is
reached through a local forwarder (the PAC script holds only `SOCKS5 127.0.0.1:<port>` and
`DIRECT`), there is no `; DIRECT` fallback (a dead proxy fails instead of leaking the real IP),
and the pref sets `pac_mandatory`. No rules + default proxy → `fixed_servers`; no rules +
direct → `direct`.

Fit a fingerprint to a proxy: `identity.generate {save: true}` →
`identity.fit_to_proxy {identity_id, proxy_id: "pool:de", profile}` →
`stealth.fingerprint_set {identity_id}`. The offline table in `src/country.rs` (75 countries:
main timezone, languages, main city; a few multilingual regions such as Quebec, Wallonia,
Geneva, Ticino) fills in what the lookup leaves out.

## What an identity changes

| Value | How |
|---|---|
| UA, Accept-Language, `navigator.platform`, UA client hints | `Network.setUserAgentOverride` (GREASE brand list computed like Chromium; full version taken from the running engine) |
| Timezone, locale | `Emulation.setTimezoneOverride`, `Emulation.setLocaleOverride` |
| Screen, DPR, mobile viewport | `Emulation.setDeviceMetricsOverride` (desktop keeps the real window size) |
| `screen.availWidth/availHeight` | patch script: screen minus the taskbar (Windows 11: 48 px, 10: 40 px) or menu bar (macOS: 25 px); phones keep the full screen |
| Touch | `Emulation.setTouchEmulationEnabled` |
| Location | `Emulation.setGeolocationOverride` |
| CPU cores | `Emulation.setHardwareConcurrencyOverride` (JS patch if the engine lacks it) |
| `deviceMemory`, `webdriver`, WebGL 1/2 vendor/renderer | `src/js/patch.js` via `Page.addScriptToEvaluateOnNewDocument {runImmediately}`: prototype getters, native-looking `toString`, no globals |

The claimed browser is always Chrome with the engine's major version: other versions are
revealed by feature detection. iPhone/iPad presets claim Safari and are marked
`device_emulation` — fine for responsive testing, not for stealth.

## Consistency checklist (`stealth.check`)

Headless / background tabs and emulation leave tells; `stealth.check` probes them:

| Probe | Pass when | Fix |
|---|---|---|
| `navigator.webdriver`, own properties, automation globals, headless UA | clean | patch script |
| UA vs UA-CH version / platform / mobile, `fullVersionList` | consistent | identity |
| `UA-CH high entropy vs identity` | `getHighEntropyValues` (platformVersion, architecture, bitness, model, wow64, fullVersionList, brands) = the override | identity (full version from the engine) |
| timezone vs Date offset, languages | consistent | identity |
| screen vs window, outer window size | real-looking | — |
| `screen.availHeight` | equals the identity's avail size (or, without identity, avail < screen on desktop) | patch script |
| `page visibility` | `visible` (hidden/background tabs throttle timers and rAF) | report only |
| WebGL / `WebGL2 renderer` | no SwiftShader, fits the OS, WebGL2 = WebGL1 | patch script (both contexts) |
| `fonts vs OS` | the installed fonts fit the claimed OS (Segoe UI vs Helvetica Neue vs Roboto ...) | report only (fonts are the host's) |
| `media devices` | devices exist and their labels are empty without a permission grant | report only (cef permission handling) |
| `permissions consistency` | `Notification.permission` agrees with `permissions.query` (flags the headless `denied`/`prompt` pattern) | cef permission handling |
| window.chrome, plugins, CDP console serialization, `Error.prepareStackTrace`, native-looking patches | clean | — |
| worker navigator | UA, cores, language, timezone same in a worker | worker coverage below |
| matches applied identity | every applied value visible | — |

## For the `cef` crate (per-profile wiring)

Status (cef crate, `docs/phase2/profiles.md`): per-profile `CefRequestContext`s exist, with
proxy / WebRTC / Accept-Language prefs set by `bmcp-profiles` (`profile.create {proxy,
identity, accept_language}`, its own proxy setting through this crate's `Forwarder`); new tabs
and popups of a profile with an assigned identity start on `about:blank`, get
`stealth.fingerprint_set`, then load; dedicated workers are auto-attached
(`waitForDebuggerOnStart`, `flatten`) and get the UA override plus a `navigator` patch (cores,
memory, languages) before they run — verified live. **Not wired yet** (they arrived in main
after that work): `profile_proxy_pref` / `proxy.rules_set` routes + `proxy.routes_changed`,
`cef_prefs::profile_prefs`, `identity_for_profile` and `worker_commands` below; bmcp-profiles
still uses its own equivalents.

All functions are on `bmcp_identity::Services` (`Services::from_ctx(&ctx)` or
`registry.extensions().get::<Services>()`), CEF-agnostic and tested here:

1. **Per-profile proxy.** When a profile's `CefRequestContext` is created:
   `services.profile_proxy_pref(profile).await?` → `ProfileProxy { pref, pac_script,
   forwarders }`; on the UI thread `SetPreference("proxy", pref)`. It starts the local
   forwarders the routes need (also after a restart) and reuses running ones. Subscribe to the
   event bus: on topic `proxy.routes_changed` (`bmcp_identity::ROUTES_CHANGED`, data
   `{profile, pref}`) set `pref` on that profile's context. A profile without rules gets
   `{"mode": "direct"}`. `socks5://` makes Chromium send host names, so DNS is resolved by the
   upstream.
2. **WebRTC + Accept-Language.** `bmcp_identity::cef_prefs::profile_prefs(identity, proxied)`
   → `[(name, value)]` to `SetPreference` on the same context: `webrtc.ip_handling_policy =
   disable_non_proxied_udp`, `webrtc.multiple_routes_enabled = false`,
   `webrtc.nonproxied_udp_enabled = false` when `proxied` (pref mode is not `direct`), and
   `intl.accept_languages` from the identity (covers requests outside the tab session, e.g.
   service worker fetches).
3. **Auto-apply on new tabs.** `services.identity_for_profile(profile)?` → `Option<Identity>`;
   run `stealth::apply(ctx, tab, &services.tabs, identity, Scope::Fingerprint)` (what
   `stealth.fingerprint_set` does) before the first navigation, or pause the target with
   `Target.setAutoAttach {waitForDebuggerOnStart}`, apply, then resume. Same for popups.
4. **Workers.** `Target.setAutoAttach {autoAttach: true, waitForDebuggerOnStart: true,
   flatten: true}` on tab sessions (and the browser session for service workers). On
   `Target.attachedToTarget {sessionId, targetInfo}`: send each
   `bmcp_identity::worker_commands(&identity, &targetInfo.type, engine_version)` with that
   `sessionId`, in order; a failure is never fatal (warn unless `optional`), and the last
   command (`Runtime.runIfWaitingForDebugger`) must always be sent. Non-worker targets get only
   that resume command, so it can run for every auto-attached target. See `src/worker.rs`.

## For the `jobs` crate (per-row isolated profiles)

Per row: `proxy.lease {pool, run_id, row}` (or `services.lease(pool, run, row, rotate)`) →
`proxy_id: "lease:POOL/RUN/ROW"` → `proxy.rules_set {profile: <row profile>, default:
proxy_id}` (the browser then routes that profile through the leased proxy) and optionally
`identity.fit_to_proxy {identity_id, proxy_id}`. A retry after a block: `proxy.lease
{..., rotate: true}` then `rules_set` again. When the run ends: `proxy.release {pool, run_id}`.
Leases are stored in `proxies.json`, so a resumed run keeps its rows' IPs.

## Notes from the live run (CEF 156, Windows 11)

* `Page.enable` is required before `Page.addScriptToEvaluateOnNewDocument`, otherwise the
  script is accepted but never runs; with it, `runImmediately` patches the open page at once.
* The `acceptLanguage` override becomes `navigator.languages` verbatim, so identities store a
  plain list (`de-DE,de,en-US,en`) and Chromium adds the q-values to the header.
* Chromium 156 reports `deviceMemory` up to 32 (not capped at 8 any more).

## Known gaps

* Workers: `Emulation.setHardwareConcurrencyOverride` on the tab does not reach dedicated
  workers. Tabs of a profile with an assigned identity get the CEF worker patch (cores,
  memory, UA, languages — verified live); a tab that only got `stealth.fingerprint_set` by hand
  still shows the real values in its workers. Timezone and WebGL in workers are not patched.
* No H.264 in the CEF build (sannysoft `VIDEO_CODECS` warns); real Chrome has it.
* Camera/microphone are denied unless `permissions.set` allows them for the site, so pages no
  longer get device labels by default.
* Fonts are the host's (`stealth.check` reports a mismatch with the claimed OS).
* The macOS avail size assumes a hidden or side dock (menu bar only).
* `geo.lookup` / `fit_to_proxy` use ip-api.com's free endpoint, which is plain HTTP (the
  lookup request is not encrypted); pass `url` for another service with the same JSON format.
* Proxy passwords are stored as plain text in `data/config/proxies.json` (like the rest of the
  portable data folder) and only masked in command output.
