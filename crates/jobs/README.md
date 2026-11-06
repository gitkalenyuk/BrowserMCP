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
           heal?: true, profile?: "default"}      → run (id, options.parallel, counts …)
```

* Each row → one script process with `--input <column>=<value>` (`inputs.<column>`, strings).
  CSV/Excel: first row = header (Excel: first sheet); JSON: array of objects.
* `parallel` is limited to `min(requested, CPUs/2, free RAM/400 MB)` (and 32).
* Failed rows are retried `retries` times with backoff 1 s, 2 s, 4 s … (max 30 s).
* All state is in `data/db/runs.sqlite`: `run.status {id}`, `run.list {limit?}`,
  `run.results {id, cursor?, limit?, status?}` (paged), `run.pause/resume/cancel {id}`.
  A run interrupted by a browser restart is `paused` after the next start; `run.resume`
  continues it.
* **Profiles:** today all rows share the browser's default profile (each row should open its
  own tab). `profile` only accepts `"default"`; TODO: isolated per-row profiles.
  **Known issue:** with `parallel > 1`, rows that type into pages interfere: `page.act fill`
  input lands in another (background) tab and background tabs report "no box". The live test
  below had to run with `parallel: 1` for correct results; parallel rows need isolated
  windows/profiles (or tab-targeted input) first.

**Self-healing.** When a row fails (after its retries) with `NOT_FOUND`, `NOT_INTERACTABLE`,
`STALE_REF`, `TIMEOUT` or `AMBIGUOUS_TARGET` from a `page.*` call, the row becomes `needs_heal`,
the run pauses and `run.heal_needed {id, row, script, error, step, line}` is emitted. An agent
proposes a fix and calls `run.heal_apply {id, patched_code}`: the code is saved as
`<name>.v2.ts` (then `.v3` …), the waiting rows go back into the queue and the run continues
with the new version. `run.resume` instead skips the fix (those rows count as failed).

**Exports.** `run.export {id, to: csv|xlsx|json|sqlite|webhook, path?, url?, status?, batch_size?}`.
Files default to `data/runs/<id>/results.<ext>`; columns `row, status, <inputs>, <result keys>,
error_code, error, attempts, duration_ms`. Webhook: `POST {run, script, batch, batches, rows}`
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
| `run.heal_needed` | `{id, row, script, error, step, line}` |
| `run.finished` | `{id, status, total, ok, failed, cancelled, needs_heal, duration_ms}` |
| `trigger.fired` | `{id, script, ok, exit_code, duration_ms, result, error}` |

## Tests

`cargo test -p bmcp-jobs` uses `bmcp-fake-script-host` (`src/bin/fake_script_host.rs`), a
V8-free stand-in with a tiny line DSL, so queue/retry/pause/cancel/heal/export/trigger tests
run in seconds. The live test (`tests/live/login.ts` + `logins.csv`) is manual: start the
browser, `script.save` the file, `run.start` with the CSV.
