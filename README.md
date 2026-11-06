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
```

The first build downloads CEF (~500 MB) and copies its runtime next to `target/debug/browsermcp.exe`.

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
