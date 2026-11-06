# Portable Windows release

Plan decisions this implements: storage is portable (everything in `./data` next to the exe),
distribution is "for ourselves" (no installer, no signing, no auto-update), CEF is bumped by hand
once a month (`cargo xtask bump-cef`). macOS packaging is Phase 11 and not covered here.

## Building

```bash
cargo xtask dist [--skip-script-host] [--out <dir>] [--locales en-US,uk|all] [--no-build] [--no-zip]
```

1. `pnpm install --frozen-lockfile` (only if `node_modules` is missing) and `pnpm -C ui build`:
   `crates/cef/build.rs` embeds `ui/dist` into the exe. dist refuses an exe that still carries the
   "UI not built" placeholder.
2. `cargo build --release -p browsermcp -p bmcp-script` (profile: `lto = "thin"`,
   `codegen-units = 1`, `strip = "symbols"`). `--skip-script-host` drops `bmcp-script` (no V8).
3. Assembles `dist/BrowserMCP-<version>-win64/` and zips it to `dist/BrowserMCP-<version>-win64.zip`
   (deflate, `zip` crate), printing the size of every top-level entry.

The target folder comes from `cargo metadata`, so `CARGO_TARGET_DIR` is honoured. CEF runtime files
are copied from the CEF distribution the build actually used (the `CEF_DIR` line in
`target/release/build/cef-dll-sys-*/output`), not from `target/release`: `cef-dll-sys` copies the
whole distribution there (`libcef.lib`, `bootstrap*.exe`, `CMakeLists.txt`, `archive.json`) and never
deletes files of an older CEF.

### Layout

| Entry | What |
|---|---|
| `browsermcp.exe` | the browser; also Chromium's renderer / GPU / utility sub-processes |
| `bmcp-script-host.exe` | TypeScript script host (V8). Found next to `browsermcp.exe` by `script.run` / mass runs (`bmcp_jobs::host::locate_host`) |
| `libcef.dll`, `chrome_elf.dll` | CEF / Chromium |
| `resources.pak`, `chrome_100_percent.pak`, `chrome_200_percent.pak`, `icudtl.dat`, `v8_context_snapshot.bin` | Chromium resources |
| `d3dcompiler_47.dll`, `dxcompiler.dll` | ANGLE (D3D11) and Dawn/WebGPU (D3D12) shader compilers |
| `vk_swiftshader.dll`, `vk_swiftshader_icd.json`, `vulkan-1.dll` | software GL fallback (WebGL without a usable GPU, VMs, RDP) |
| `locales/` | `en-US` + `uk` (and their `_FEMININE/_MASCULINE/_NEUTER` variants) by default; `--locales all` keeps all 220 files. Chromium falls back to en-US for any other system locale, so en-US is always kept |
| `data/` | empty; `DataDir::resolve` uses it (profiles, db, logs, scripts, recordings, runtime/endpoint.json) |
| `README.txt` | run, connect Claude Code (HTTP and stdio), files, ffmpeg |
| `THIRD_PARTY.txt` | bundled components, every Rust crate compiled into the two executables (normal dependencies from `cargo metadata --filter-platform x86_64-pc-windows-msvc`, build/dev-only crates excluded), the npm packages bundled into the UI (`pnpm licenses list --prod` + the Svelte runtime), then each distinct license text once with the packages that use it |
| `LICENSES/` | `Chromium-CREDITS.html` (CEF's CREDITS.html = chrome://credits), `CEF-LICENSE.txt` (BSD, taken from the CEF headers: the minimal distribution has no LICENSE.txt), `Readability-LICENSE.md` (Apache-2.0) |

Not bundled: `ffmpeg.exe` (video recording; put it next to the exe, set `BMCP_FFMPEG`, or install
it — `bmcp_record::video::find_ffmpeg`), filter lists (downloaded into `data/filters` on first use),
`bootstrap.exe` (see the sandbox section).

### Sizes (0.1.0, CEF 156.0.15)

Measured 2026-11-06 with the default `--locales en-US,uk`:

| Entry | Size |
|---|---|
| `libcef.dll` | 280.7 MB |
| `bmcp-script-host.exe` | 54.4 MB |
| `browsermcp.exe` (UI embedded) | 50.5 MB |
| `dxcompiler.dll` | 24.8 MB |
| `resources.pak` | 23.5 MB |
| `icudtl.dat` | 10.3 MB |
| `LICENSES/` (5.2 MB of it is Chromium's CREDITS.html) | 5.2 MB |
| `vk_swiftshader.dll` | 5.1 MB |
| `d3dcompiler_47.dll` | 4.5 MB |
| `chrome_elf.dll` | 2.7 MB |
| `locales/` (8 of 220 files; all would be ~50 MB) | 1.8 MB |
| everything else (`vulkan-1.dll`, 100/200 % paks, snapshot, THIRD_PARTY.txt 0.8 MB, README) | 3.9 MB |
| **folder total** | **467.4 MB** |
| **`BrowserMCP-0.1.0-win64.zip`** | **208.4 MB** |

THIRD_PARTY.txt: 408 Rust crates and 8 npm packages, 260 distinct license texts.

Ways to shrink further if it ever matters: `--skip-script-host` (−54 MB, no scripts), dropping
`dxcompiler.dll` (−25 MB, WebGPU loses its D3D12 shader compiler), `vk_swiftshader*` (−5 MB, no
software WebGL fallback). Not done by default: they are cheap insurance on unusual machines.

## Verification (done on the 0.1.0 build)

Ran the packaged `browsermcp.exe` from `dist/BrowserMCP-0.1.0-win64/` (working directory
`%TEMP%`, so a cwd-relative path would have shown up there) with `--api-port 47891`:

* `GET /v1/health` → `{"ok":true,"name":"BrowserMCP","version":"0.1.0","commands":178}`.
* MCP over Streamable HTTP: `initialize` → 200 with `Mcp-Session-Id`, serverInfo `browsermcp 0.1.0`;
  `tools/list` → 34 tools (default toolsets).
* REST `tabs.open {"url":"https://example.com/"}` → new tab; `tabs.list` a few seconds later shows
  it loaded ("Example Domain"). The first start also opened the `bmcp://ui/library.html` import
  page, i.e. the embedded Svelte UI (not the placeholder) is served.
* REST `script.run` with inline TypeScript (`browser.tabs.list()` + `browser.output(...)`) →
  `{"ok":true,"exit_code":0,"result":{"tabs":3,"sum":42}}` in 651 ms: `bmcp-script-host.exe` was
  found next to `browsermcp.exe` and talked back over the WebSocket.
* `browsermcp.exe mcp-stdio --api-port 47892` (against a second packaged instance): `initialize`
  and `tools/list` answered on stdout although the release exe is a GUI-subsystem binary.
* Stopped with `taskkill /PID <pid> /T /F` (own PIDs only); no package process left. A forced kill
  leaves `data/runtime/endpoint.json` behind (a normal window close removes it).
* Writes outside the package: every file under `%APPDATA%` and `%LOCALAPPDATA%` (Temp excluded)
  modified during the run belonged to other programs (Claude desktop/CLI, PowerShell, pnpm); no new
  `HKCU\Software` key. Everything of ours was under `data/`: `config/notify.json`,
  `db/browsermcp.sqlite`, `db/runs.sqlite`, `logs/cef.log`, `profiles/` (Chromium's `Local State`,
  component data, GPU caches, `profiles/default`), `runtime/endpoint.json`. `script.run` writes its
  inputs file to `%TEMP%` (allowed).

## CI

`.github/workflows/ci.yml` has a `release` job: on `v*` tags and manual `workflow_dispatch` it runs
after the `windows` job (fmt, clippy, tests) and executes `cargo xtask dist` with Node 24 +
pnpm 11.7.0, then uploads `dist/BrowserMCP-*-win64.zip` as a workflow artifact
(`BrowserMCP-<ref>-win64`). Nothing is published as a GitHub release.

## Updating Chromium

`cargo xtask bump-cef <cef crate version> [--dry-run]`: rewrites `cef = { version = "=X" }` in the
root `Cargo.toml`, runs `cargo update -p cef -p cef-dll-sys` (restoring both files if it fails),
refreshes the "CEF build (x / Chromium y)" comment from `Cargo.lock`, and prints the next steps (cef-rs
changelog, build, test + clippy, smoke run, bench, dist, `ENGINE_CHROME_MAJOR` in
`crates/identity` when the Chromium major changes, commit). See the README, "Updating Chromium".

## Windows sandbox (plan risk: "Windows sandbox у cef-rs")

Status: **off** (`Settings::no_sandbox = 1` in `crates/cef/src/lib.rs`). Renderer, GPU and
utility processes run with the browser's full user rights. Investigated for release builds; not
implemented.

How CEF does it now: since M138 `cef_sandbox.lib` can only be linked into binaries built inside the
Chromium tree, so the Windows sandbox moved into a prebuilt **bootstrap** executable
(`bootstrap.exe` for `/SUBSYSTEM:WINDOWS`, `bootstrapc.exe` for console), shipped in the
distribution we already download (4.3 MB each, unsigned). The application is built as a **DLL**:
rename `bootstrap.exe` to `browsermcp.exe`, put `browsermcp.dll` next to it; the bootstrap loads it
and calls its export `RunWinMain(HINSTANCE, LPWSTR, int, void* sandbox_info, cef_version_info_t*)`
(see `include/cef_sandbox_win.h`), and the DLL passes `sandbox_info` to `CefExecuteProcess` and
`CefInitialize`. The bootstrap checks the DLL against libcef.dll (sandbox-compat hash / Chromium
version, and the API hash — `cef-dll-sys` already compiles the wrapper with
`CEF_API_VERSION_LAST`, which is what bootstrap clients need).

What cef-rs offers: the `sandbox` feature (on by default; we use `default-features = false`) turns
on `USE_SANDBOX` for `libcef_dll_wrapper`, and its `build-util` bundler
(`cef::build_util::win::copy_app`) does exactly the rename: builds the crate with `--lib`, copies
`<name>.dll` + `<name>.pdb` and `bootstrap.exe` as `<name>.exe`. It does not generate the
`RunWinMain` export or the sandbox-info plumbing; the app has to.

What it would take here:

1. `crates/app`: a `cdylib` target exporting `#[unsafe(no_mangle)] extern "system" fn RunWinMain`
   that runs today's `main` with the given `sandbox_info`, keeping the `bin` for dev builds (two
   build modes, or always the DLL).
2. `crates/cef` (owned by another agent, not touched): a `sandbox` cargo feature forwarding
   `cef/sandbox`, and `start()` / `Browser::run()` taking the `sandbox_info` pointer for
   `execute_process` / `initialize`, with `no_sandbox = 0`.
3. `xtask dist`: ship `bootstrap.exe` as `browsermcp.exe` + `browsermcp.dll`.

Why not now (risks):

* **Our exe resources are lost.** `browsermcp.exe` would be CEF's binary: its manifest, its (empty)
  version info, no icon of ours. The supportedOS manifest from `crates/app/build.rs` is what keeps the
  GPU process alive; the bootstrap has CEF's own (cefsimple's), so that part is fine, but icon and
  version info need Resource Hacker / `rcedit` on every release, and CEF asks to code-sign all
  binaries after modification.
* **`mcp-stdio` and the CLI.** The proxy must never start CEF; under the bootstrap every launch,
  including `browsermcp.exe mcp-stdio`, goes through the bootstrap into `RunWinMain`. It would still
  work (our code runs first), but `--help`/stdio output from a GUI-subsystem exe and exit codes need
  re-testing; `bootstrapc.exe` would bring back a console window for the browser itself.
* **Startup and process model.** Today `bmcp_cef::start()` is the first thing in `main`; under
  the bootstrap the Rust runtime starts inside a DLL loaded by a C++ exe (panics across the FFI
  boundary must be caught, `std::env::current_exe()` becomes the bootstrap — fine for
  `DataDir`, `locate_host` and `find_ffmpeg`, since the names stay the same).
* **Sandboxed renderers break assumptions** that only an end-to-end pass would catch: anything a
  renderer-side handler does with files or the network, GPU-process restrictions on some drivers,
  and the "another instance may be using the data folder" path.
* **Two build modes** (bin for `cargo xtask run`, DLL + bootstrap for dist) mean two code paths
  to keep working, and `cargo xtask test` would not cover the shipped one.

Recommendation: keep the sandbox off for the self-use release (we only browse with it ourselves and
let agents drive it); do it as a separate task owned by the `crates/cef` maintainer, in this order:
cdylib + `RunWinMain` behind a feature, `sandbox_info` through `bmcp-cef`, a dist mode that ships
the bootstrap, then the full e2e suite (`cargo xtask bench`) against the sandboxed package. Until
then the README.txt in the package states that the sandbox is off.
