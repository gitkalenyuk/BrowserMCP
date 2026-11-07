# bmcp-jobs

Scripts, mass runs, exports, cron triggers and the self-healing hook (plan phases 8–9).
Every function is a registry command, so it is on MCP (`/mcp?toolsets=script,run`), REST
(`POST /v1/cmd/run.start`) and WS at the same time.

## Scripts

`data/scripts/<name>.ts`; names are 1–64 chars of `A-Z a-z 0-9 _ -`, plus an optional version
suffix `.v2` (healed versions). A script runs in a separate `bmcp-script-host` process
(V8, `crates/script`) that sends `browser.*` calls back to `ws://127.0.0.1:<api port>/v1/ws`.

| Command | |
|---|---|
| `script.list` / `script.get {name}` / `script.save {name, code}` / `script.delete {name}` | the library |
| `script.run {name \| code, inputs?, timeout_secs?}` | run once, wait; → `{ok, exit_code, duration_ms, result, stdout, stderr, error}` |

**Structured output.** `browser.output({...})` prints one stdout line `##bmcp:result <json>`.
Several calls merge (object keys combine, later wins; a non-object replaces). It becomes
`result` in `script.run`, the row result in runs, and result columns in exports. Any program
that prints such lines works the same.

**Errors.** Host exit codes: 0 ok, 1 uncaught error, 2 setup error (e.g. browser not reachable),
124 timeout. On exit 1 the stderr line `error: Uncaught Error: browser.page.find failed:
NOT_FOUND: …` is parsed into `error: {code: "NOT_FOUND", command: "page.find", message, line}`
(`line` = top stack frame in the script). Other codes: `SCRIPT_ERROR`, `SCRIPT_TIMEOUT`,
`HOST_ERROR`, `CANCELLED`.

The host binary is found next to `browsermcp.exe` (also `target/<profile>/` in a cargo build),
or via `BMCP_SCRIPT_HOST`, `$CARGO_TARGET_DIR/{debug,release}`, or `JobsConfig::host_exe`.

## Mass runs

```text
run.start {script, data: {csv_path | json_path | xlsx_path | rows: [...]},
           parallel?: 4, retries?: 1, timeout_secs?: 300, stop_on_error?: false,
           heal?: "ask", profile?: "default"}     → run (id, options.parallel, counts …)
```

* Each row → one script process; its inputs (`inputs.<column>`, strings) go in a temporary
  JSON file passed as `--inputs-file` (no command-line length limit, NUL bytes allowed).
  CSV/Excel: first row = header (Excel: first sheet); JSON: array of objects.
* `parallel` is limited to `min(requested, CPUs/2, free RAM/400 MB)` (and 32).
* Failed rows are retried `retries` times with backoff 1 s, 2 s, 4 s … (max 30 s).
* All state is in `data/db/runs.sqlite`: `run.status {id}`, `run.list {limit?}`,
  `run.results {id, cursor?, limit?, status?}` (paged), `run.pause/resume/cancel {id}`.
  A run interrupted by a browser restart is `paused` after the next start; `run.resume`
  continues it.
* **Profiles:** each row's script host gets `BMCP_TAB_DEFAULTS` (JSON), which the host merges
  into every `tabs.open` call the script makes (keys the script sets itself win):
  * `profile` omitted / `"default"` → `{run}`: tabs in the browser window, shared profile;
  * `"isolated"` → `{profile: "<run>-r<row>-a<attempt>", incognito: true, window: "offscreen",
    run}`: every row attempt gets a fresh in-memory profile (own cookies) and windowless tabs;
  * any profile id (`profile.create`) → `{profile, run}`: rows share that profile.

  `run` sends the row's downloads to `data/runs/<run>/downloads/`.
  **Use `"isolated"` for `parallel > 1` rows that type or click:** in one window only the
  shown tab is visible — a background tab is hidden (`visibilityState: "hidden"`) and
  `page.act` refuses input to it (`NOT_INTERACTABLE: tab N is hidden`), which is what broke the
  first live test with `parallel: 3` (7 of 10 rows failed). Off-screen tabs are always visible
  and each has its own focus, so 3 rows typing at once each reach their own page (live:
  10/10 rows correct with `parallel: 3`, see `docs/phase2/profiles.md`).
* **Proxies per row:** `run.start {profile: "isolated", proxy_pool: "res", fit_identity?}`.
  Before each attempt the [`RowProfileHook`] (installed by the app, implemented by
  `bmcp_profiles::RowProfiles`) leases a proxy of the pool to the row (`proxy.lease`, sticky
  per row; retries pass `rotate: true`), routes the attempt's profile through it and, with
  `fit_identity`, gives it an identity fitted to the proxy's exit IP. After the attempt the
  profile is forgotten; at the row's end the lease is released; at the run's end all of them.
  A failed setup (dead proxy, failed geo lookup) fails the attempt like a script error, so the
  retry gets another proxy. Without the hook, `proxy_pool` is refused (`NOT_IMPLEMENTED`).

**Self-healing.** When a row fails (after its retries) with `NOT_FOUND`, `NOT_INTERACTABLE`,
`STALE_REF`, `TIMEOUT` or `AMBIGUOUS_TARGET` from a `page.*` call, the row becomes `needs_heal`,
the run pauses and `run.heal_needed {id, row, script, error, step, line}` is emitted. An agent
proposes a fix and calls `run.heal_apply {id, patched_code}`: the code is saved as
`<name>.v2.ts` (then `.v3` …), the waiting rows go back into the queue and the run continues
with the new version. Only a run with an open heal request (`heal` in `run.status`) takes a
patch: a second, outdated one for the same break is refused. `run.resume` instead skips the fix
(those rows count as failed).
`heal` is `"ask"` (default), `"auto"` or `"off"` (`true`/`false` = ask/off); the mode is
in the event as `mode`. A hook (`JobsService::set_heal_hook`, installed by `crates/heal`) takes a
page snapshot before the event goes out; it is stored in the `heal_snapshots` table
(`JobsService::heal_snapshot`). Proposing, accepting and "auto" are `crates/heal` (see its
README: `run.heal_propose/accept/reject`, `run.heal_proposed/applied/failed`).

**Exports.** `run.export {id, to: csv|xlsx|json|sqlite|webhook, path?, url?, status?, batch_size?, raw?}`.
Files default to `data/runs/<id>/results.<ext>`; columns `row, status, <inputs>, <result keys>,
error_code, error, attempts, duration_ms`, unique ignoring case (`input.status`,
`result.<key>`, then `_2`, … on clashes). CSV cells starting with `= + - @` tab or CR get a
leading `'` (XLSX: the quote-prefix flag) so spreadsheets do not run scraped formulas;
`raw: true` writes them unchanged. Webhook: `POST {run, script, batch, batches, rows}`
batches (100 rows), 3 attempts on network errors/408/429/5xx. `google_sheets` →
`NOT_IMPLEMENTED` (needs OAuth).

## Triggers

`trigger.set {kind: "cron", spec, script, inputs?, id?}`, `trigger.list`, `trigger.remove {id}`.
`spec` is local time, 5 fields (`"0 9 * * Mon-Fri"`) or 6–7 with seconds first. Checked every
second; missed firings while the browser was closed are skipped; a trigger never overlaps
itself. TODO: webhook triggers (need an HTTP route in `crates/api`).

## Events

Subscribe on WS with `events.subscribe {"topics":["run.*","trigger.*"]}` (not in the default
set, which has `runs.*` for the UI).

| Topic | Data |
|---|---|
| `run.started` | `{id, script, total, parallel}` |
| `run.progress` | `{id, done, ok, failed, needs_heal, cancelled, running, pending, total}` |
| `run.row_failed` | `{id, row, error, attempts}` |
| `run.heal_needed` | `{id, row, script, error, step, line, mode}` |
| `run.finished` | `{id, status, total, ok, failed, cancelled, needs_heal, duration_ms}` |
| `trigger.fired` | `{id, script, ok, exit_code, duration_ms, result, error}` |

## Tests

`cargo test -p bmcp-jobs` uses `bmcp-fake-script-host` (`src/bin/fake_script_host.rs`), a
V8-free stand-in with a tiny line DSL, so queue/retry/pause/cancel/heal/export/trigger tests
run in seconds. The live test (`tests/live/login.ts` + `logins.csv`) is manual: start the
browser, `script.save` the file, `run.start` with the CSV.
