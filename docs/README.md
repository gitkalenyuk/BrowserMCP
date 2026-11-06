# BrowserMCP docs

Start with the [top-level README](../README.md) (what BrowserMCP is, build/run, connecting
agents, the crate map).

## Plan

* [plan.md](plan.md) — the project plan (in Ukrainian): decisions, architecture, threading
  rules, project tree, snapshot format, MCP catalog, phases 0–11, risks.

## Spikes (Phase 1 go/no-go)

* [spikes/S1-S2-S3.md](spikes/S1-S2-S3.md) — S1 CEF Views window composition (UI view +
  content view + overlay, Alloy style), S2 UI-thread bridge + in-process CDP, S3 MCP server
  (rmcp, Streamable HTTP).
* [spikes/S5-script-runtime.md](spikes/S5-script-runtime.md) — S5 script runtime: TypeScript in
  a deno_core sidecar, rustyscript vs raw deno_core.
* [spikes/S6-video.md](spikes/S6-video.md) — S6 tab video (CDP screencast → ffmpeg) and the
  recorder binding without `Runtime.enable`.

## Phase notes

* [phase2/tabs.md](phase2/tabs.md) — Phase 2: tabs and the real Svelte chrome (window
  composition, vertical tabs, palette, new tab).
* [phase2/profiles.md](phase2/profiles.md) — CEF hooks: profiles and isolation (off-screen
  tabs), adblock gate, cookies, downloads, permissions, identity auto-apply, CDP port.
* [phase4/library.md](phase4/library.md) — Phase 4 UI: library (bookmarks, history), bookmarks
  bar, password manager, Chrome import wizard.
* [phase5/panels.md](phase5/panels.md) — Phase 5/6 UI: AI chat, terminal and traffic side
  panels.
* [phase7/panels-record-runs.md](phase7/panels-record-runs.md) — Phase 7 UI: recording and runs
  side panels (codegen, self-healing).
* [phase8/scripts-debugger.md](phase8/scripts-debugger.md) — Phase 8: Scripts side panel
  (Monaco) and the step debugger.
* [settings/settings.md](settings/settings.md) — the settings page (`bmcp://ui/settings.html`),
  the AI heal flow in the Runs panel, heal proposals as notification questions, plugin palette
  entries.

## Checks

* [checks/2026-11-06-mvp-claude-code.md](checks/2026-11-06-mvp-claude-code.md) — MVP check:
  Claude Code drives BrowserMCP by refs only, no screenshots.
* [tests/bench](../tests/bench/README.md) — the MVP benchmark (`cargo xtask bench`): 10 tasks
  on local fixture sites for Claude Code / Codex.

## Component READMEs

| README | Covers |
|---|---|
| [crates/api](../crates/api/README.md) | Routes (MCP, REST, OpenAPI, WebSocket), REST error statuses, JSON-RPC, event topics, Host/Origin guard |
| [crates/perception](../crates/perception/README.md) | Snapshot format and modes, refs, diff, API, golden tests |
| [crates/cdpshare](../crates/cdpshare/README.md) | Sharing one tab's CDP session: domain refcount, the `Fetch` coordinator, lost events |
| [crates/adblock](../crates/adblock/README.md) | Adblock and simplify commands, threading, the planned CEF hook, the interim CDP path, numbers, gaps |
| [crates/identity](../crates/identity/README.md) | Hidden mode: identities, stealth checks, devices, proxy pools, PAC, forwarder |
| [crates/jobs](../crates/jobs/README.md) | Scripts, mass runs, per-row profiles, the self-healing hook, exports, cron triggers, run events |
| [crates/heal](../crates/heal/README.md) | The self-healing loop (propose / accept / reject / auto), `page.extract` and its plan cache |
| [ui](../ui/README.md) | The Svelte chrome: running it, mock vs real backend, layout, library, side panels |
| [sdk/protocol](../sdk/protocol/README.md) | Generated command types, `createBrowser` |
| [sdk/ts](../sdk/ts/README.md) | The TypeScript SDK: connecting, errors, events |

Crates without a README document themselves in their `src/lib.rs` (or `src/main.rs`) module
docs: `cargo doc --workspace --exclude bmcp-script --no-deps --open`.
