# bmcp-cdpshare

Several subsystems drive the same CDP session of a tab (`net`, `record`, `adblock`, …). Some
CDP state belongs to the session, not to the caller, so this crate owns it. One `CdpShare`
lives in the registry's `Extensions` (`CdpShare::from_ctx`).

**Rule:** never send `<Domain>.enable/disable`, `Fetch.enable/disable` or
`Target.setAutoAttach` directly, and never answer a `Fetch.requestPaused` read from
`cdp_events` yourself. Use this crate.

## Auto-attach owner

`Target.setAutoAttach` is one setting per session; every call replaces the last one. Holders:
`frames` (bmcp-browser OOPIF support: no pausing) and `identity` (bmcp-profiles: new workers
start paused until they got the identity).

```rust
let switched_on = share.hold_auto_attach(&backend, tab, None, "identity", true).await?;
if !switched_on { share.reannounce_auto_attach(&backend, tab, None).await?; }
```

* Sent as `{autoAttach: true, flatten: true, waitForDebuggerOnStart: <any holder wants it>}`,
  per session (`None` = the page, `Some(id)` = a flattened child session), again only when the
  merged setting changes; off when the last holder leaves.
* A holder that asks for pausing must resume every target it sees attach paused
  (`Runtime.runIfWaitingForDebugger`, iframes too).
* Chromium announces existing targets only when auto-attach is switched on. `hold_auto_attach`
  returns false when it was on already; a holder that needs the existing targets then calls
  `reannounce_auto_attach` (off + on; also after lost events).

## Domain refcount

```rust
share.acquire(&backend, tab, "Network", "record:rec-1", json!({})).await?;
// …
share.release(&backend, tab, "Network", "record:rec-1").await?;
```

* `<Domain>.enable` goes out for the first holder. It goes out again when the merged
  parameters grow: numbers take the largest value, booleans are OR-ed, other fields keep the
  first holder's value.
* `<Domain>.disable` goes out when the last holder lets go.
* Holder names make it idempotent: holding twice under one name counts once.
* `Page` is never disabled. Code outside the refcount enables it too (identity), and nothing
  disabled it before.

Current holders:
* `net`: `Network`, while `net.capture` is on.
* `record:<id>`: `Page` always, plus `Network` with `traffic: true`.
* `adblock`: `Page`, while `adblock.apply` is on.
* `adblock.measure:<n>`: `Network` and `Page`, while `simplify.measure` runs.

## Fetch coordinator

Each subsystem registers a `FetchClient`: a name, an order, its `Fetch.RequestPattern`s and a
`FetchHandler`.

* `Fetch.enable` is sent with the union of all clients' patterns (deduplicated, in client
  order).
* `Fetch.disable` is sent only when the last client is gone.
* The coordinator has its own event subscription and is the only reader of
  `Fetch.requestPaused`.
* Each pause is offered to the clients whose patterns match it, in this order:
  `ORDER_ADBLOCK` (10), then `ORDER_NET` (20), then a default continue. A client returns one
  of three verdicts:
  * `Pass`: ask the next client.
  * `Answer(a)`: the coordinator sends `a`.
  * `Hold`: the client answers later with `CdpShare::answer`, as net breakpoints do.

**Exactly one answer.** Every pause is tracked until it is answered. The entry is removed
before the answer is sent, so a second answer gets `AnswerError::NotPaused`. If an answer fails,
a plain continue is sent instead. The request is also continued in these cases:
* the handler panics or takes longer than 30 s;
* the request is held but not answered within 120 s.

After the answer, every interested client hears about it through `FetchHandler::done`, which it
uses for stats and to drop stale state.

## Lost events

A lost `Fetch.requestPaused` (broadcast `Lagged`) leaves its request paused forever.
`Fetch.disable` + `enable` would release it, but it would also release requests that clients
hold on purpose, so the coordinator does not do that. It does two things instead:

1. **It makes lag unlikely.** The event loop does O(1) work per event, and no client reads
   pauses from its own slower subscription. A lagging `net.capture` listener only loses
   traffic entries.
2. **It sweeps.** Chromium names Fetch requests `interception-job-<N>.<hop>`, with N counting
   up. After a lag the coordinator waits 500 ms so that in-flight events can still arrive. It
   then sends `Fetch.continueRequest` for each `interception-job-<N>.0` in a range:
   * The range starts 32 below the last job seen before the lag.
   * It ends at the highest job seen since. If no newer job was seen, it ends at a bounded
     guess.
   * It skips ids that are pending (being decided or held) and ids already answered for good.

   For ids that are not paused in this session, Chromium answers "Invalid InterceptionId",
   which is ignored.

The sweep is best effort:
* It relies on Chromium's id scheme.
* It only covers the first hop of each request.
* A swept request goes through without being offered to the clients, so adblock does not block
  it.

Counters are available from `CdpShare::fetch_stats`.
