# BrowserMCP

A daily-driver browser that people **and** AI agents (Claude Code, Codex, any LLM) both drive.
Chromium via [CEF](https://github.com/tauri-apps/cef-rs), core in Rust. Every browser function is
a command in one registry, exposed automatically over MCP (and soon REST + WebSocket, CDP and a
TypeScript SDK). Agents read pages through the accessibility tree — no screenshots needed.

Status: **Phase 1** (spikes) — window, in-process CDP, MCP server, command registry, snapshot
engine, Svelte UI (not yet embedded), TypeScript script host. Plan: [docs/plan.md](docs/plan.md),
spike notes: [docs/spikes/](docs/spikes/).

## Build and run (Windows)

Requirements: Rust (rustup, MSVC toolchain), Visual Studio Build Tools with the C++ workload and a
**Windows 11 SDK (10.0.22621 or newer)**, CMake (the copy bundled with Visual Studio is found
automatically), Ninja, Node 22+ and pnpm (for `ui/`).

```bash
cargo xtask doctor          # check tools
cargo xtask run             # build + open https://example.com
cargo xtask run -- https://github.com --profile work
cargo xtask test            # Rust tests (add --all to include the V8 script host)
cargo xtask run -- --headless --window-size 1280x800   # no windows: automation host
cargo xtask e2e             # build, then the end-to-end suite (tests/e2e) against --headless
```

`--headless` opens no window at all: every tab is an off-screen page (screenshots, screencast
video and CDP input work), popups become tabs, and the process keeps running after its last tab
closes until `system.quit` (`--device-scale <f>` sets the pixel ratio; `browsermcp mcp-stdio
--headless` starts the browser that way).

The first build downloads CEF (~500 MB) and copies its runtime next to `target/debug/browsermcp.exe`.

## Portable release

```bash
cargo xtask dist                      # -> dist/BrowserMCP-<version>-win64/ and .zip
cargo xtask dist --skip-script-host   # without bmcp-script-host.exe (no V8 build)
cargo xtask dist --out D:\builds --locales en-US,uk,de   # or --locales all
cargo xtask dist --no-build           # repackage the existing target/release build
```

`dist` builds the UI (`pnpm -C ui build`, embedded into the exe), the release `browsermcp.exe` and
`bmcp-script-host.exe`, and assembles one folder: the two executables, the CEF runtime (DLLs,
`*.pak`, `icudtl.dat`, `v8_context_snapshot.bin`, `locales/` — en-US + uk by default), an empty
`data/`, `README.txt`, `THIRD_PARTY.txt` and `LICENSES/`. Unzip it anywhere and run
`browsermcp.exe`; everything it stores stays in `data/` next to it. Video recording needs
`ffmpeg.exe` next to the exe or on `PATH` (not bundled). CI builds the zip on `v*` tags and manual
runs (artifact only). Details, sizes and the sandbox status: [docs/release.md](docs/release.md).

## Updating Chromium

The `cef` crate is pinned exactly in the root `Cargo.toml`; every crate release tracks one CEF
build. Once a month, by hand:

```bash
cargo xtask bump-cef 157.0.0 --dry-run   # what would change
cargo xtask bump-cef 157.0.0             # re-pin + cargo update -p cef -p cef-dll-sys
```

It updates the pin and the CEF build noted above it, restores `Cargo.toml`/`Cargo.lock` if the
update fails, and prints the next steps: read the cef-rs changelog, `cargo xtask build` (downloads
the new CEF), `cargo xtask test` + clippy, a smoke run, `cargo xtask bench`, `cargo xtask dist`, and
— when the Chromium major changes — `ENGINE_CHROME_MAJOR` in `crates/identity`. Commit
`Cargo.toml` + `Cargo.lock` (the CI CEF cache is keyed by `Cargo.lock`).

## Connect an AI agent

While the browser runs, MCP is served at `http://127.0.0.1:47800/mcp` (local only; requests from
web pages are rejected).

**Claude Code** — this repo ships `.mcp.json`, or add it globally:

```bash
claude mcp add --transport http browsermcp http://127.0.0.1:47800/mcp
```

**Clients that only speak stdio** (and to auto-start the browser on demand):

```bash
claude mcp add browsermcp -- C:\path\to\browsermcp.exe mcp-stdio
```

More tool groups: `http://127.0.0.1:47800/mcp?toolsets=core,system,net`.

## Data

Everything is portable: profiles, logs and later recordings live in `data/` next to the executable
(`--data-dir <path>` overrides it). `data/runtime/endpoint.json` tells tools where the running
instance listens.

## Layout

| Path | What |
|---|---|
| `crates/app` | `browsermcp` binary: CLI, wiring, `mcp-stdio` proxy |
| `crates/core` | Command registry, `BrowserBackend` trait, events, portable paths (no CEF) |
| `crates/cef` | The only crate that talks to CEF (windows, UI-thread bridge, DevTools) |
| `crates/cdp` | Chrome DevTools Protocol client over a pluggable transport |
| `crates/cdpshare` | Shared use of a tab's CDP session: domain refcount, one `Fetch` coordinator per tab |
| `crates/perception` | Accessibility-tree snapshots with refs, find, diff |
| `crates/api` | MCP server (rmcp, Streamable HTTP) generated from the registry |
| `crates/script` | `bmcp-script-host`: TypeScript automation scripts on deno_core (sidecar) |
| `ui/` | Svelte 5 browser chrome and new-tab page (`pnpm ui:dev`) |
| `xtask` | `cargo xtask` automation |
