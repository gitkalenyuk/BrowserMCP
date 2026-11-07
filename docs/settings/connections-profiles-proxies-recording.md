# Settings: Connections & API, Profiles & identity, Proxies, Recording & scripts

Date: 2026-11-07. Four sections of `bmcp://ui/settings.html` for what the browser could already do
from the API but not from its own UI. Code: `ui/src/settings/{Connections,Profiles,Proxies,Recording}Section.svelte`,
logic in `ui/src/lib/settings/e2-*.ts` (vitest next to it), strings in
`ui/src/lib/i18n/{en,uk}/settings-{connections,profiles,proxies,recording}.ts`, wire types in
`ui/src/lib/api/types-e2-settings.ts`, fakes in the E2 block at the end of `ui/src/lib/api/mock-settings.ts`.

## Sections

* **Connections & API** (`api.info`, `api.config`, `agents.list`, `tabs.list`): API status and
  port, `data/runtime/endpoint.json`, MCP / REST / WebSocket / OpenAPI URLs; a toolsets picker
  (command count per toolset) that builds `…/mcp?toolsets=…`; ready-to-copy
  `claude mcp add --transport http browsermcp <url>`, the stdio variant with the real exe path
  (`browsermcp.exe mcp-stdio`), `codex mcp add`, Codex `config.toml` (stdio and HTTP) and an
  `mcpServers` JSON; the CDP port for Playwright / Puppeteer (on/off with a confirmation, port,
  what runs now, "restart to apply" (never for a port opened with `--cdp-port`), refused while
  a profile has a stealth identity, snippets; a broken `connections.json` is reported while the
  URLs and commands still show);
  connected agents and locked tabs (every 4 s); the security model (127.0.0.1, Host / Origin
  guard, no tokens by the user's choice).
* **Profiles & identity** (`profile.*`, `identity.*`, `stealth.check`, `proxy.pool_list`,
  `proxy.forward_list`): create (name → id), rename, delete with confirmation (keep data
  optional; the launch profile and profiles with tabs cannot be deleted), per profile: proxy
  (direct / a pool / a URL; a masked URL is never sent back), Accept-Language, WebRTC policy
  (automatic = limited to the proxy when proxied, normal, public address only, only through the
  proxy; warnings for a leak or a direct profile without UDP), *Open tabs in memory*
  (incognito default; not offered for the launch profile), identity (assign a saved one, or create
  one from a preset: desktop / mobile stealth or device emulation), *Fit to the proxy*
  (`identity.fit_to_proxy` through the profile's pool or local forwarder; shows what changed),
  a fingerprint check of the profile's first tab; saved identities with who uses them, delete.
  While the CDP port is open the section says so and offers no identity (the core refuses it).
* **Proxies** (`proxy.pool_*`, `proxy.rules_get/set/test`, `proxy.forward_list`, `geo.lookup`):
  pools with health summary (working / down / not checked, median latency, last check), rotation
  (in turn / random / sticky), health check, per-proxy table, which profiles use a pool, delete
  with confirmation; add one proxy (HTTP / SOCKS5, host, port, login; *Test* = geo lookup through
  it) or a pasted list to an existing or new pool; routing per profile: default route + ordered
  rules (`*.host`, URL globs, `re:`), test a URL; local forwarders.
* **Recording & scripts** (`record.settings`, `script.settings`, `system.open_path`): ffmpeg
  status (where it was found, version) or how to install it (`winget install Gyan.FFmpeg`,
  `ffmpeg.exe` next to `browsermcp.exe`, `BMCP_FFMPEG`); default video fps / format / quality;
  script host status; default script timeout, timeout per row, rows at once (with the machine's
  recommendation and the hard limit) and retries; recordings / videos / scripts / runs folders
  with *Open folder*.

## Backend added for them

| Command | Crate | What |
|---|---|---|
| `api.info` | api (`src/info.rs`) | port, URLs, endpoint.json, exe path, data folder, toolsets with command counts, CDP state |
| `api.config` | api | CDP port on/off + port in `data/config/connections.json`; refused while a profile has an identity; only the browser UI (`Caller::Ui`) may turn it on, agents and scripts get `NEEDS_APPROVAL` |
| `record.settings` | record (`src/settings.rs`) | video defaults in `data/config/recording.json` (used by `record.video_start` and `record.start {video}` when they leave values out; the recording's video now gets the right extension), ffmpeg status, folders |
| `script.settings` | jobs (`src/settings.rs`) | defaults of `script.run` (and cron triggers) timeout, `run.start` timeout / parallel / retries in `data/config/scripts.json`, script host status, recommended parallelism |
| `proxy.rules_get` | identity | a profile's routing with passwords masked |
| `profile.create {incognito_default, webrtc}` | profiles | stored in `profiles.json`; `tabs.open {profile}` without `incognito` uses the profile's default (core asks bmcp-profiles through the `ProfileTabDefaults` extension); `webrtc` overrides the three WebRTC prefs; `profile.list` reports both plus `webrtc_applied` |

`proxy.rules_set` now keeps the stored password when it gets a masked URL (`user:***@`) back, so a
UI can send back what `proxy.rules_get` showed. The app reads `connections.json` at startup:
`--cdp-port` wins, otherwise the saved setting; both are refused while a profile has a stealth
identity (as before). While the port is open (`bmcp_core::CdpLaunch` in the registry
extensions), `identity.assign`, `profile.create {identity}` and mass runs with `fit_identity`
refuse to give a profile a stealth identity, so the two are never combined at runtime either.
`bmcp_core::config_file` holds the small atomic JSON load/save these use. ffmpeg is started
without a console window (`CREATE_NO_WINDOW`), so opening the Recording section does not flash
one in release builds.

## Verified

* Rust: unit tests for each settings file (defaults, partial files, patch / reset, validation,
  restart-needed logic), registry tests for `api.info` / `api.config` (incl. the stealth refusal),
  `record.settings` (an mp4 default makes `record.start {video}` write `video.mp4`),
  `script.settings` (saved defaults reach `script.run` and `run.start`, explicit values win),
  `proxy.rules_get` + masked round trip.
* UI: vitest for the helpers (commands, TOML quoting, toolsets URL, proxy URLs and lists, health,
  rule checks, profile save diff, fit reference, recording forms), the mock flows, and that every
  key the sections use exists (`e2-i18n.test.ts`). Clicked through all four sections in
  `pnpm dev` (mock): create a profile, identity from a preset, fit to proxy, pool health check,
  proxy test, routing test, video form validation.
* Real browser: `tests/e2e/tests/settings-api.test.ts` (`cargo xtask e2e --filter settings-api`):
  `api.info` reports the harness's API and CDP ports, `api.config` saves without touching the
  running port, `record.settings` / `script.settings` see ffmpeg / the script host as the harness
  does, `proxy.rules_get` masks passwords. The dev UI (`BMCP_DEV_UI=1`, Vite on 5173,
  `?api=ws`) against a headless browser: all four sections load real data; creating a profile,
  an identity from a preset and a pool from a pasted list work.

## Gaps

* Not clicked through inside the browser's own settings tab (only the dev UI against the core).
* *Fit to the proxy* and *Test* call the IP lookup service (ip-api.com) through the proxy, so
  they were only exercised with the mock.
* Device emulation per profile is an identity with a device-emulation preset (per tab:
  `device.emulate`).
* HTTPS (TLS to the proxy) and SOCKS4 upstream proxies are refused by the core's proxy parser
  (`crates/identity/src/proxy/config.rs`); supporting HTTPS would need TLS in the local
  forwarder (`forward.rs`), so the forms offer HTTP and SOCKS5 only.
* The stdio MCP variant always lists the default toolsets (the picker applies to HTTP).
* Video size is the tab's page size (not configurable; headless: `--window-size`).
