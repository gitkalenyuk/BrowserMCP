# bmcp-identity

Hidden mode, Phase 10 foundations: fingerprint identities, stealth checks, device emulation,
proxy pools and a local proxy forwarder. No CEF dependency; everything goes through
`BrowserBackend::cdp` and is tested with `FakeBackend`. All commands are in the `stealth`
toolset (`/mcp?toolsets=core,stealth`).

## Commands

| Command | What |
|---|---|
| `identity.presets` | Device classes: `windows-1080p`, `windows-1440p`, `windows-laptop`, `windows-768`, `macos-mbp14`, `macos-mba13`, `android-pixel-8`, `android-galaxy-s23`, `iphone-15`, `ipad-air` |
| `identity.generate {os?, preset?, seed?, id?, save?, country?, timezone?, locale?, latitude?, longitude?}` | Consistent random identity, deterministic by seed |
| `identity.list` / `get` / `remove` / `assign {profile, id?}` | `data/config/identities.json` |
| `stealth.fingerprint_set {tab?, identity_id \| identity \| preset, reload?}` | Apply to a tab |
| `stealth.fingerprint_clear {tab?}` | Remove again |
| `stealth.check {tab?}` | Pass/warn/fail table of fingerprinting probes |
| `device.emulate {tab?, device}` / `device.clear` | "iPhone 15", "Pixel 8", "Galaxy S23", "iPad" (layout testing) |
| `proxy.pool_add {pool, proxies, strategy?}` / `pool_list` / `pool_check` / `pool_remove` / `next` | `data/config/proxies.json`; `round_robin`, `random`, `sticky` (per run id) |
| `proxy.forward_start {proxy \| pool, run?}` / `forward_stop` / `forward_list` | Local no-auth SOCKS5+HTTP proxy on 127.0.0.1 chained to an authenticated upstream |
| `geo.lookup {proxy? \| pool?, url?}` | Exit IP → country, city, timezone, coordinates, suggested locale |

Fit a fingerprint to a proxy: `geo.lookup {pool}` → `identity.generate {country, timezone,
latitude, longitude, save: true}` → `stealth.fingerprint_set {identity_id}`.

## What an identity changes

| Value | How |
|---|---|
| UA, Accept-Language, `navigator.platform`, UA client hints | `Network.setUserAgentOverride` (GREASE brand list computed like Chromium; full version taken from the running engine) |
| Timezone, locale | `Emulation.setTimezoneOverride`, `Emulation.setLocaleOverride` |
| Screen, DPR, mobile viewport | `Emulation.setDeviceMetricsOverride` (desktop keeps the real window size) |
| Touch | `Emulation.setTouchEmulationEnabled` |
| Location | `Emulation.setGeolocationOverride` |
| CPU cores | `Emulation.setHardwareConcurrencyOverride` (JS patch if the engine lacks it) |
| `deviceMemory`, `webdriver`, WebGL vendor/renderer | `src/js/patch.js` via `Page.addScriptToEvaluateOnNewDocument {runImmediately}`: prototype getters, native-looking `toString`, no globals |

The claimed browser is always Chrome with the engine's major version: other versions are
revealed by feature detection. iPhone/iPad presets claim Safari and are marked
`device_emulation` — fine for responsive testing, not for stealth.

## CEF follow-ups (owned by the `cef` crate) — done, see `crates/profiles` and `docs/phase2/profiles.md`

Status: 1–5 are implemented. `profile.create {id, proxy, identity, accept_language}`
(bmcp-profiles) stores the settings, starts this crate's forwarder and sets the prefs below on
the profile's own `CefRequestContext`; new tabs and popups of a profile with an assigned
identity are created on `about:blank`, get `stealth.fingerprint_set`, and only then load their
URL; their workers are auto-attached and patched (UA, Accept-Language, `hardwareConcurrency`,
`deviceMemory`, `languages`) before they run. The original notes:

1. **Per-profile proxy.** Each profile gets its own `CefRequestContext` (already one cache
   folder per profile). After `proxy.forward_start`, set on that context (UI thread):
   `SetPreference("proxy", {"mode": "fixed_servers", "server": "socks5://127.0.0.1:<port>"})`.
   Use the forwarder even for unauthenticated upstreams, so rotation can swap the upstream
   without touching Chromium. `socks5://` makes Chromium send host names, so DNS is resolved
   by the upstream.
2. **WebRTC.** On the same context: `SetPreference("webrtc.ip_handling_policy",
   "disable_non_proxied_udp")` (and `webrtc.multiple_routes_enabled = false`,
   `webrtc.nonproxied_udp_enabled = false`) so WebRTC cannot reveal the real or local IP.
3. **Auto-apply on new tabs.** CDP overrides belong to one tab's DevTools session. When a tab
   is created in a profile with an assigned identity (`identities.json` → `profiles`), run
   `stealth.fingerprint_set` before the first navigation (or pause the target with
   `Target.setAutoAttach {waitForDebuggerOnStart}` and apply, then resume). Same for popups.
4. **Workers.** `Page.addScriptToEvaluateOnNewDocument` does not reach dedicated/shared/service
   workers; `stealth.check` reports a "worker navigator" warning when they differ. Fix with
   `Target.setAutoAttach {flatten}` on worker targets and the same overrides there.
5. **Accept-Language at network level** for requests that bypass the tab session (service
   worker fetches): set `intl.accept_languages` on the request context too.

## Notes from the live run (CEF 156, Windows 11)

* `Page.enable` is required before `Page.addScriptToEvaluateOnNewDocument`, otherwise the
  script is accepted but never runs; with it, `runImmediately` patches the open page at once.
* The `acceptLanguage` override becomes `navigator.languages` verbatim, so identities store a
  plain list (`de-DE,de,en-US,en`) and Chromium adds the q-values to the header.
* Chromium 156 reports `deviceMemory` up to 32 (not capped at 8 any more).

## Known gaps

* Workers: `Emulation.setHardwareConcurrencyOverride` does not reach dedicated workers. Tabs
  of a profile with an assigned identity are fixed by the CEF worker auto-attach (follow-up
  4); a tab that only got `stealth.fingerprint_set` by hand still shows the real core count
  in its workers.
* No H.264 in the CEF build (sannysoft `VIDEO_CODECS` warns); real Chrome has it.
* Camera/microphone are denied unless `permissions.set` allows them for the site, so pages no
  longer get device labels by default.
* Fonts are the host's, `screen.availHeight` equals `screen.height` under emulation.
* `geo.lookup` uses ip-api.com's free endpoint, which is plain HTTP (the lookup request is not
  encrypted); pass `url` for another service with the same JSON format.
* Proxy passwords are stored as plain text in `data/config/proxies.json` (like the rest of the
  portable data folder) and only masked in command output.
