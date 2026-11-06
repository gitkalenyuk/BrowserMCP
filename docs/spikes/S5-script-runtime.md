# Spike S5: script runtime (deno_core sidecar)

**Question.** Can user automation scripts in TypeScript run in an embedded V8 inside a
separate sidecar process, calling browser commands through one generic op? And should we
build it on `rustyscript` or on raw `deno_core` (+ `deno_ast`)?

**Answer.** Yes. **Decision: raw `deno_core` + `deno_ast`.** Do not use `rustyscript`. This
changes the plan's "start with rustyscript" line (see "Decision" below).

Machine: Windows 11 Pro 26100, Intel i5-10400 (6C/12T), 32 GB RAM, Rust 1.98.1 MSVC.
All timings were measured on this machine only.

## What was built

Crate `crates/script` (package `bmcp-script`), binary `bmcp-script-host`:

```text
bmcp-script-host run <file.ts> [--input key=value]... [--mock | --ws <url>]
                     [--timeout-secs N] [--timings]
exit codes: 0 ok, 1 uncaught script error, 2 usage/setup error, 124 timeout
```

| File | What it does |
|---|---|
| `src/main.rs` | CLI with a hand-written parser. Starts a current-thread tokio runtime (one thread owns one isolate) and picks the sink. |
| `src/host.rs` | `run_script()`: creates a fresh `JsRuntime`, loads the module, runs the event loop to completion, and enforces the timeout. |
| `src/loader.rs` | `TsModuleLoader`: allows `file://` only, transpiles TS with `deno_ast` (types are stripped, not checked), and keeps source maps plus the original text so stacks show TS lines. |
| `src/ops.rs` | The 4 ops (`op_bmcp_call`, `op_bmcp_inputs`, `op_bmcp_log`, `op_bmcp_sleep`) and the `bmcp` extension, which carries the sink and inputs in `OpState`. |
| `src/js/prelude.js` | Defines the globals `browser` (a Proxy: `browser.ns.method(args)` becomes `call("ns.method", args)`), `inputs`, `console` and `sleep`. `then` is excluded so the proxies are not thenables. |
| `src/sink.rs` | The `CommandSink` trait (`call(name, args) -> Future<Result<Value>>`) and `MockSink`: `tabs.list` returns 2 tabs, `test.fail` returns an error, anything else echoes `{called, args}`. |
| `src/ws_sink.rs` | `WsSink`: JSON-RPC 2.0 over WebSocket (tokio-tungstenite). One socket task matches responses to requests by `id`, so parallel calls work. **Not tested against the real browser**, because `ws://127.0.0.1:47800/v1/ws` does not exist yet. It is tested against an in-process fake server. |
| `types/browser.d.ts` | Ambient types for editors. The examples reference it with `/// <reference path>`. |
| `scripts/examples/hello.ts` | Lists tabs, loops over `nav.goto`, uses `inputs`, takes a snapshot. |
| `scripts/examples/error.ts` | Throws to show the stack. |
| `tests/cli.rs` | Runs the real binary: hello output (exact match), error (exit 1, TS stack `error.ts:13:11` / `:20:1`), sink error is catchable, busy-loop timeout, pending-await timeout, missing file. |
| `tests/ws_sink.rs` | Round trip through `WsSink` with a fake JSON-RPC server: echo, `Promise.all` id matching, error reply. Also covers an unreachable browser. |
| `probe-rustyscript/` | Throwaway comparison: the same idea on `rustyscript`. It is a standalone package (not a workspace member) and is not built by the workspace. |

Verified: `cargo build -p bmcp-script`, `cargo test -p bmcp-script` (8 tests pass),
`cargo clippy -p bmcp-script --all-targets -- -D warnings`, `cargo fmt -p bmcp-script --check`.

### How one call flows

```text
script.ts   await browser.tabs.list()
prelude.js  Proxy  -> op_bmcp_call("tabs.list", null)          (async op, returns a Promise)
ops.rs      OpState -> Rc<dyn CommandSink> -> sink.call(...)
sink        MockSink: canned JSON   |   WsSink: {"jsonrpc":"2.0","id":1,"method":"tabs.list","params":null}
```

Adding a browser command needs **no change** in the script host, because the command name is
just a string.

### Timeout / cancellation

`--timeout-secs N` covers two cases, and each needs its own mechanism:

- **JS is busy** (`while (true) {}`). The tokio timer can't fire because the thread is stuck
  inside V8. A watchdog OS thread calls `IsolateHandle::terminate_execution()`, which is
  thread-safe.
- **JS is idle in an `await`** (a slow browser command or `sleep`). V8 isn't running, so
  there is nothing to terminate. `tokio::time::timeout` drops the run future instead, which
  also drops any pending sink calls.

Both cases exit with code 124 within about 1 s of the limit (tested). A terminated isolate is
not reused; the process exits.

### Example output

```text
> bmcp-script-host run scripts/examples/hello.ts --input name=Ada --input times=3 --timings
hello, Ada
found 2 tabs
- 1: Example Domain <https://example.com/>
- 2: GitHub <https://github.com/>
goto #1 -> {"called":"nav.goto","args":{"url":"https://example.com/?page=1"}}
...
page.text -> {"called":"page.text","args":null}
done
[timings] runtime_init=21.4ms run=2.8ms total=25.4ms

> bmcp-script-host run scripts/examples/error.ts
got 2 tabs, now failing on purpose
error: Uncaught Error: step "click login" has no selector
    at checkStep (file:///C:/.../scripts/examples/error.ts:13:11)
    at file:///C:/.../scripts/examples/error.ts:20:1
```

A TS syntax error is reported with a caret under the bad token, at its TS position.

## rustyscript vs deno_core, as actually experienced

### Versions

| | **deno_core path (chosen)** | **rustyscript path (probe)** |
|---|---|---|
| Top crate | `deno_core =0.413.0` (current) | `rustyscript =0.12.3` (latest release) |
| deno_core | 0.413.0 | 0.355.0, through rustyscript's `^0.355` |
| V8 | `v8` 150.4.0, through the new `deno_v8` 0.5.0 facade | `v8` 137.3.0 |
| TS transpiler | `deno_ast =0.53.3` (the version deno_core 0.413 tests with) | `deno_ast =0.49.0`, pinned by rustyscript |
| Other | `deno_error =0.7.1`, `tokio =1.54.1`, `tokio-tungstenite =0.30.0`, `anyhow =1.0.104` | `default-features=false, features=["console"]` |

rustyscript's latest release trails current deno_core by about 58 releases and 13 V8
milestones. crates.io already lists a third-party "maintained replacement for rustyscript"
(`denoshim` 0.1.0), which is another sign rustyscript has fallen behind.

### Did it build?

- **deno_core:** yes, first try, apart from two API errors that were mine (below).
- **rustyscript:** **no**, not as published:
  1. With a fresh lockfile it failed: `swc_common 9.2.0` (from rustyscript's pinned
     `deno_ast 0.49`) uses `serde::__private`, which serde ≥ 1.0.220 no longer has. The
     workaround is a lockfile-only downgrade: serde 1.0.219, serde_json 1.0.140, uuid 1.18.1.
     **In our workspace this would force old serde on every crate, because the workspace
     shares one `Cargo.lock`.** Any `cargo update` breaks it again.
  2. With default features it failed: `deno_crypto → aws-lc-rs → aws-lc-sys 0.47` needs
     **NASM** on Windows. The workaround is `default-features = false, features = ["console"]`.

### Measurements (release profile = workspace settings: `lto="thin"`, `codegen-units=1`, `strip`)

| Metric | deno_core (`bmcp-script-host`) | rustyscript probe |
|---|---|---|
| Clean release build (includes V8 download) | **350 s** | 237 s |
| Incremental release (touch 1 file, mostly LTO link) | 72 s | 59 s |
| Clean debug build (includes V8 download) | ~170 s | not measured |
| Incremental debug (touch 1 file) | **14.5 s** | not measured |
| Release exe size | **53.9 MB** | 49.0 MB |
| Idle isolate process memory (working set / private) | **~20 MB / ~9–10 MB** | ~19 MB / ~8 MB |
| Isolate creation incl. prelude (median of 10) | **~26 ms** (21–30) | ~23 ms (19–26) |
| hello.ts: transpile + evaluate + 7 calls | **~3 ms** | ~4.5 ms |
| Wall time per `run hello.ts` process | **~48 ms** | ~48 ms |
| Process start + exit without V8 (`--help`) | ~20 ms | - |

Notes:
- The prebuilt V8 static lib (`rusty_v8.lib`) is **212 MB** and is downloaded once per
  profile (debug and release). `target/` reached 4.5 GB, and the probe's own target was
  1.9 GB.
- The clean-build gap comes mostly from the newer and larger swc/deno_ast 0.53, plus
  tokio-tungstenite. Day-to-day work is incremental debug at about 15 s.
- About 20 MB per idle process means a pool of, say, 8 parallel script processes costs
  about 160 MB. A second isolate inside the same process would cost less, but that was not
  measured.
- Startup could drop further with a V8 startup snapshot (`RuntimeOptions::startup_snapshot`).
  That isn't needed now.

### TS support

| | deno_core + deno_ast | rustyscript |
|---|---|---|
| Transpile `.ts/.tsx/.mts` | yes (our loader) | yes (built in) |
| Type checking | no (strip only, like `deno run --no-check`) | no |
| Top-level await | yes | yes |
| Error position | correct TS line:col for **every frame**, with function names | correct line:col of the **top frame only**, and the quoted source line came from the *transpiled* code, so it showed the wrong line (`const tabs = ...` instead of the `throw`) |
| Imports | `file://` only, by design | file + optional URL imports via features |

### Ergonomics

**rustyscript is nicer to start with.** `Runtime::new`, `register_async_function("name",
async_callback!(|a: String, b: Value| ...))` and `Module::load`, with a timeout built into
`RuntimeOptions`. The probe is 69 lines. But it hides deno_core while still leaking it (its
`Error`, `deno_core::Extension`, versions), and you're stuck on its deno_core version.

**deno_core is more code, but every line is ours.** About 750 lines of Rust including
comments, plus a 72-line prelude. About 290 of those lines are the CLI and `WsSink`, which
either approach needs. Only the loader (about 130 lines) and the timeout (about 30 lines)
are things rustyscript would have given us. Friction I actually hit in deno_core 0.413:
- `#[op2]` rejects a bare `Value`. It wants the spelled-out `serde_json::Value` path
  (error: "Invalid or deprecated #[serde] type").
- Putting state into `OpState` after `JsRuntime::new` is **too late**: the extension's ESM
  prelude runs inside `new`, so it panicked with "required type ... is not present in
  GothamState". The fix is `extension!(..., options = {...}, state = |state, options| ...)`
  and `bmcp::init(sink, inputs)`.
- The prelude imports ops from `ext:core/ops`. That is an internal specifier, not a
  documented API.
- deno_core gives only a minimal `console` and timers. We supply `console` and `sleep`
  ourselves, so the host controls where logs go.

## Decision

**Use raw `deno_core` + `deno_ast`, pinned exactly, in a separate `bmcp-script-host` process.**

Why:
1. rustyscript is stuck on a year-old deno_core/V8 and **does not build** on Windows without
   lockfile surgery that would downgrade serde for the whole workspace. That alone rules it
   out for the workspace.
2. What it saves is small (loader + timeout, about 160 lines) and is already written and
   tested here.
3. Error stacks are better with our own loader, and that matters for "self-healing" and for
   showing errors in the UI.
4. The deno_core surface we touch is small and contained: `ops.rs`, `loader.rs` and
   `host.rs`. All browser commands go through one op, so registry growth never touches V8
   code.

**Upgrade policy:** bump `deno_core` + `deno_ast` + `deno_error` together, on purpose, about
quarterly (not monthly like CEF). Take `deno_ast` from deno_core's own `[dev-dependencies]`
for that version, then run `cargo test -p bmcp-script`.

**Fallbacks, in order:**
1. Stay on the pinned deno_core indefinitely. It's a sidecar with no fs/net, so old V8 is a
   low risk.
2. The same scripts can run outside V8 entirely. The planned `@browsermcp/sdk` (WS
   transport) exposes the same `browser.*` API, so a user script could run under stock
   `deno`/`node` as a subprocess talking to the same WS endpoint. The script format does not
   lock us into embedded V8.
3. `denoshim` (new, unproven) or rustyscript, if it catches up. Re-evaluate with the probe
   in `crates/script/probe-rustyscript`.
4. deno_core 0.413 has an experimental `quickjs` backend (through `deno_v8`/`v8x`) that could
   shrink the binary. It's unexplored.

## Open risks

- **V8 symbol clash if ever linked into the CEF process.** libcef.dll carries Chromium's V8,
  and rusty_v8 is also built with Chromium's toolchain and custom libc++ (`std::__Cr`
  namespace, `v8_use_custom_libcxx` is on by default). Two V8s plus two libc++ copies in one
  process risk ODR/symbol clashes, doubled memory, and a script crash taking the browser
  down. **Rule: `bmcp-script` must never be a dependency of `crates/app`/`crates/cef`.**
  Suggested CI guard: `cargo tree -p <app crate> -i deno_core` must be empty.
- **Windows CRT linkage warning.** Both paths print `LNK4098: defaultlib 'libcmt.lib'
  conflicts with use of other libs`: the prebuilt V8 lib uses the static CRT. It's harmless
  so far, and clippy doesn't link so `-D warnings` passes. A full `RUSTFLAGS=-Dwarnings`
  link step would fail on it. crates.io even has a fork, "v8-taktcode", whose description
  says it fixes Windows CRT linkage, so others hit this too.
- **Build-time download.** V8 comes from GitHub releases (denoland/rusty_v8) during
  `cargo build`, at 212 MB per profile. Offline or locked-down builds need `RUSTY_V8_ARCHIVE`
  or `RUSTY_V8_MIRROR`. In CI, `cargo xtask test` (`cargo test --workspace`) now also builds
  this crate, which adds minutes on a cold cache. Rust-cache should hold it. The CEF cache key
  is `hashFiles('Cargo.lock')`, so this commit invalidates it once.
- **API churn.** Seen within this one spike: the new `deno_v8` facade crate, op2 type
  strictness, extension init through options, and the internal `ext:core/ops` import. It's
  manageable because everything is pinned and contained in 3 files.
- **Cancellation of in-flight commands.** On timeout we drop the pending future, but the
  browser keeps executing the command. The WS protocol will need a cancel notification
  (for example `$/cancelRequest`) in phase 3/8. A *synchronous* op can't be interrupted by
  `terminate_execution`. All our ops are async except log and inputs, which are instant.
- **One isolate per thread (plan).** A terminated isolate must be thrown away and recreated
  (about 25 ms). With one process per script run, as now, that's automatic.
- **No type checking at run time.** Only the editor (Monaco/VS Code + `browser.d.ts`) catches
  type errors. Real `tsc` checking would be a separate tool.
- **Minimal JS environment.** There is no `setTimeout`, `fetch`, `URL` or `TextEncoder` yet.
  Adding the `deno_web`/`deno_url` extensions pulls more deps, and their versions must match
  deno_core exactly. Upside: scripts have no fs/net access at all by default (sandbox), and
  imports are `file://` only.
- **Not checked:** behaviour under Windows Defender first-run scanning of a 54 MB unsigned
  exe, macOS builds (Phase 11), and many concurrent isolates.

## How to run

```powershell
# build + test (first build downloads the ~212 MB V8 lib)
cargo build -p bmcp-script
cargo test  -p bmcp-script

# run the examples against the mock browser
cargo run -p bmcp-script -- run scripts/examples/hello.ts --input name=Ada --input times=3 --timings
cargo run -p bmcp-script -- run scripts/examples/error.ts                 # exit 1, TS stack
cargo run -p bmcp-script -- run crates/script/tests/fixtures/busy_loop.ts --timeout-secs 2   # exit 124

# real browser endpoint (does not exist yet -> exit 2 "cannot connect")
cargo run -p bmcp-script -- run scripts/examples/hello.ts --ws ws://127.0.0.1:47800/v1/ws

# the rustyscript probe (standalone, NOT a workspace member)
cargo build --release --manifest-path crates/script/probe-rustyscript/Cargo.toml
crates/script/probe-rustyscript/target/release/probe-rustyscript.exe scripts/examples/hello.ts
```
