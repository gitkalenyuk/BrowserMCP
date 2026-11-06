# BrowserMCP

A daily-driver browser that people **and** AI agents (Claude Code, Codex, any LLM) both drive.
Chromium via [CEF](https://github.com/tauri-apps/cef-rs), core in Rust. Every browser function
will be an API: MCP, REST + WebSocket, CDP for Playwright/Puppeteer, and a TypeScript SDK.

Status: **Phase 0** — toolchain and the first window. The full plan lives in [docs/plan.md](docs/plan.md).

## Build and run (Windows)

Requirements: Rust (rustup, MSVC toolchain), Visual Studio Build Tools with the C++ workload
and a **Windows 11 SDK (10.0.22621 or newer)**, CMake (the copy bundled with Visual Studio is found
automatically), Ninja, Node 22+ and pnpm (needed from Phase 2).

```bash
cargo xtask doctor          # check tools
cargo xtask run             # build + open https://example.com
cargo xtask run -- https://github.com --profile work
cargo xtask test            # all Rust tests
```

The first build downloads CEF (~500 MB) and copies its runtime next to `target/debug/browsermcp.exe`.
Set `CEF_PATH` to a shared folder to download it only once for every checkout.

## Data

Everything is portable: profiles, logs and later recordings live in `data/` next to the executable
(`--data-dir <path>` overrides it).

## Layout

| Path | What |
|---|---|
| `crates/app` | `browsermcp` binary: CLI, wiring |
| `crates/core` | Domain model, portable paths, command registry (no CEF, fully unit-tested) |
| `crates/cef` | The only crate that talks to CEF |
| `xtask` | `cargo xtask` automation |

More crates (cdp, perception, browser, net, api, …) arrive phase by phase as described in the plan.
