# @browsermcp/protocol

TypeScript types of everything BrowserMCP speaks, with no runtime dependencies:

* `src/generated/commands.ts` — **generated** by `cargo xtask gen-sdk` from the Rust command
  registry (`crates/registry` → `bmcp-schema` → JSON Schema → TS). For every command
  `XxxArgs` / `XxxOutput`, the `Commands` map (`Commands["tabs.list"]["output"]`),
  `COMMANDS` metadata (wire name, toolset, palette title…), the `Browser` interface and
  `createBrowser(transport)`. Do not edit; CI runs `cargo xtask gen-sdk --check`.
* `src/errors.ts` — `CmdError`, `ErrorCode`, `Retry`.
* `src/events.ts` — event payloads (`command.*`, `bus.lagged`), `topicMatches`.
* `src/transport.ts` — the `Transport` interface a `browser` object runs on.

Use `@browsermcp/sdk` for a ready WebSocket/HTTP client. The UI can import these types to
replace its hand-written `ui/src/lib/api/types.ts` command types.
