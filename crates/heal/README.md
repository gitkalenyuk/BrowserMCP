# bmcp-heal

Closes the self-healing loop of mass runs ("пауза → ШІ дивиться snapshot → пропонує патч →
підтвердження 1 раз → решта прогонів з патчем") and adds `page.extract` with a cached,
AI-built selector plan ("ШІ один раз будує селектори → кеш → прогони без ШІ").

No CEF dependency: everything goes through the registry, the event bus and
`BrowserBackend::cdp`, so it is tested with `FakeBackend`, a fake script host and canned
AI answers (no paid calls).

## The loop

```text
run.start {script, data, heal: "ask" | "auto" | "off"}          (default "ask"; true/false still work)
  row breaks on a page.* selector error (NOT_FOUND, NOT_INTERACTABLE, STALE_REF, TIMEOUT, AMBIGUOUS_TARGET)
  └─ jobs: row needs_heal, run paused, heal hook → page snapshot stored in data/db/runs.sqlite
  └─ run.heal_needed {id, row, script, error, step, line, mode}
       └─ heal watcher: AI proposal (validated) ──► mode "ask":  run.heal_proposed
                                                └► mode "auto": confidence >= 0.8 and a dry run of
                                                                the failing row passes → applied,
                                                                run.heal_applied {auto: true};
                                                                otherwise run.heal_proposed {note}
  person / bot: run.heal_accept {proposal_id}  → run.heal_apply: <name>.vN saved, broken rows
                                                  re-queued, the rest of the run uses the patch
                run.heal_reject {proposal_id, feedback?} → next proposal (max 3 attempts),
                                                  then run.heal_failed; the run stays paused
```

## Commands

| Command | Returns |
|---|---|
| `run.heal_propose {id, provider?, model?, feedback?}` | `Proposal` (below); also emits `run.heal_proposed` |
| `run.heal_accept {proposal_id}` | `{id, proposal_id, script, requeued}`; emits `run.heal_applied` |
| `run.heal_reject {proposal_id, feedback?}` | the next `Proposal`; emits `run.heal_proposed` (or fails after 3 attempts with `run.heal_failed`) |
| `run.heal_proposals {id?}` | `{proposals: [Proposal + patched_code]}`, newest first |
| `page.extract {tab?, schema, hint?, cache_key?, rebuild?, provider?, model?}` | `{data, from_cache, plan_id, cache_key, rebuilt, missing?}` |

`Proposal` = `{proposal_id, id, row, script, diff, explanation, confidence, attempt,
max_attempts, checks, status: pending|accepted|rejected|superseded, provider, model?, created_ms}`.
Proposals live in memory (after a restart, propose again). Toolsets: `run` for `run.heal_*`,
`script` for `page.extract`.

## Events (the contract for notifications and the UI Runs panel)

Subscribe with `events.subscribe {"topics": ["run.*"]}`.

| Topic | Data | UI / Telegram |
|---|---|---|
| `run.heal_needed` | `{id, row, script, error: {code, command?, message, line?}, step, line, mode}` (jobs) | "Run paused: row N broke at line L" |
| `run.heal_proposed` | `{id, proposal_id, row, script, diff, patched_code, explanation, confidence, attempt, max_attempts, checks, mode, note?}` | show `diff` + `explanation`; buttons **Accept** → `run.heal_accept {proposal_id}`, **Reject** → `run.heal_reject {proposal_id, feedback}` |
| `run.heal_applied` | `{id, proposal_id, script, requeued, auto}` | "Patched (script → login.v2), run continues" |
| `run.heal_failed` | `{id, row, attempts, error}` | "No usable patch: fix by hand (`run.heal_apply`) or skip (`run.resume`)" |

`crates/notify` turns `run.heal_needed` into Skip row / Stop run buttons and
`run.heal_proposed` into **Accept** (`run.heal_accept {proposal_id}`) / **Reject**
(`run.heal_reject {proposal_id}`), plus Show diff on Telegram; a newer proposal replaces the
older question and `run.heal_applied` closes them. The Runs panel of the UI shows the proposal
(diff, explanation, confidence, checks) with Accept / Reject with feedback / Propose.

A newer `run.heal_proposed` for the same run supersedes the older one (accepting the old one
then fails with "not pending"). So does the end of the run's wait for that fix: the run finished
or was cancelled (`run.finished`), was resumed past the break (`run.resume`) or fixed by hand
(`run.heal_apply`); `run.heal_proposals`, accept and reject check the run first. `note` is set when "auto" mode did not apply the patch (low
confidence, failed dry run, or the per-run limit of 3 automatic patches).

## What the AI gets

`HealContext::prompt` (`src/context.rs`): the error (code, command, line, message), the failing
line ±15 with the line marked, every line with a semantic locator (`act("button", "Sign in")`,
`role:`/`name:`/`text:`, `page.find`…), the page snapshot stored by the hook, the whole script
with line numbers, and the problems of earlier attempts (validation errors, reviewer
feedback). It must answer one JSON object `{diff | patched_code, explanation, confidence,
justification?}`.

**Validation** (`src/patch.rs`): the diff must apply (hunks are matched by content, nearest to
the hinted line, so wrong line numbers are fine), must change something, may only touch lines
within ±15 of the failing line unless `justification` says why, and the patched script must
pass `bmcp-script-host check <file>` (parse + transpile, no V8). An invalid answer is sent back
to the model automatically and counts as an attempt.

**The provider** is `provider` from the call, else `default_provider` in `data/config/ai.json`
(`ai.provider_set {id, default: true}`), else the first configured provider (Claude Code CLI
when installed). One-shot completions go through `bmcp_ai::oneshot` (API providers: one model
turn without tools; Claude Code: `claude -p --output-format json --tools ""`; Codex:
`codex exec --json --sandbox read-only`).

**The snapshot** (`src/watch.rs`): jobs does not know which tab a script used, so the watcher
remembers the tab of the last script call (`page.*` / `nav.wait` with `tab`, WS callers). If
that tab is still open it is snapshotted; if the script closed it, its last URL is re-opened in
a background tab for the snapshot (state from earlier steps is missing); otherwise the most
recent tab. With `parallel > 1` this can be a sibling row's tab.

## page.extract

1. Cache miss → the AI gets the JSON Schema, a page snapshot, `page.read` text and a compact DOM
   outline (tag#id.class + key attributes) and answers a plan:
   `{"fields": [{"field", "css", "attr"?, "many"?, "fields"?, "post"?} | {"field", "role",
   "name"|"text"|"regex", "many"?, "post"?}]}` with `post = {regex?, type?, default?}`.
2. The plan runs without AI: one `Runtime.evaluate` for the CSS fields, `page.find` for role
   fields, then post-processing in Rust (whitespace, regex group, number/integer/boolean by
   `post.type` or the schema's type: `"$1,299.00"` → 1299).
3. A plan that fills every `required` property is saved to `data/extract-cache/<cache_key>.json`
   (`cache_key` default: `<host>-<schema hash>`, so all pages of a site share it).
4. Cached plan misses a required property → the AI rebuilds it once (seeing the old plan and
   what it found). A plan that still misses fields is returned with `missing` and not cached.

## Gaps / TODO

* Error `hint` / `candidates` of the failed browser call don't reach the AI: the script host
  only reports `code: message` on stderr (the prompt has room for them once it does).
* The failing row runs twice in "auto" mode (dry run, then the re-queued row).
* The snapshot is of the last tab a script named; isolated per-row tabs/profiles would make it
  exact.

## Tests

`cargo test -p bmcp-heal`: `tests/heal.rs` (ask/accept, reject with feedback, validation
retries and exhaustion, region justification, auto apply / threshold / failed dry run, the
real provider path against a wiremock OpenAI-compatible server) and `tests/extract.rs` (miss →
plan → cache, hit without AI, rebuild after a page change, uncacheable plans). The fake script
host is `src/bin/fake_host.rs`.
