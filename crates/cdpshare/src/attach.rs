//! `Target.setAutoAttach` owner: one setting per CDP session, several users.
//!
//! Auto-attach is state of the session it is sent on, and every call replaces the previous
//! setting. Two users need it on a tab's page session:
//!
//! * OOPIF support in `bmcp-browser` (`frames.rs`): `{autoAttach, flatten}`, no pausing;
//! * identity tabs in `bmcp-profiles`: `{autoAttach, waitForDebuggerOnStart, flatten}`, so
//!   a new worker is paused until it got the identity's overrides.
//!
//! When the first switched it without `waitForDebuggerOnStart`, new workers of an identity
//! tab ran their first lines unpatched. Here each user holds auto-attach under its own name
//! with the pausing it needs; the session gets `autoAttach: true, flatten: true` and
//! `waitForDebuggerOnStart` = whether *any* holder wants it. A holder that asks for pausing
//! must resume every target that attaches paused (`Runtime.runIfWaitingForDebugger`, also for
//! iframes) — the others never see paused targets they did not ask for.
//!
//! Flattened sessions only (`flatten: true` is always sent). `Target.setAutoAttach` goes out
//! when the merged setting changes. Chromium announces the existing targets only when
//! auto-attach is switched *on*, so a holder joining a session where it is on already (and
//! anybody after lost events) calls [`crate::CdpShare::reannounce_auto_attach`]: off and on
//! again, which detaches and re-announces every target.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use bmcp_core::BackendResult;
use serde_json::{Value, json};

use crate::{TabShare, lock};

/// One tab's auto-attach state, per session (`None` = the page session).
#[derive(Default)]
pub(crate) struct TabAttach {
    /// Held while sending, so two changes never interleave their CDP calls.
    config: tokio::sync::Mutex<()>,
    data: Mutex<HashMap<Option<String>, Session>>,
}

#[derive(Default)]
struct Session {
    /// holder → wants `waitForDebuggerOnStart`.
    held: BTreeMap<String, bool>,
    /// The `waitForDebuggerOnStart` we sent last with `autoAttach: true`. Present = on.
    sent: Option<bool>,
}

impl Session {
    fn wanted(&self) -> Option<bool> {
        (!self.held.is_empty()).then(|| self.held.values().any(|w| *w))
    }
}

/// The `Target.setAutoAttach` parameters.
pub fn params(on: bool, wait_for_debugger: bool) -> Value {
    json!({ "autoAttach": on, "waitForDebuggerOnStart": on && wait_for_debugger, "flatten": true })
}

impl TabAttach {
    /// Holders on a session (sorted).
    pub fn holders(&self, session: &Option<String>) -> Vec<String> {
        lock(&self.data)
            .get(session)
            .map(|s| s.held.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The `waitForDebuggerOnStart` in effect (`None` = auto-attach is off).
    pub fn waiting(&self, session: &Option<String>) -> Option<bool> {
        lock(&self.data).get(session).and_then(|s| s.sent)
    }
}

async fn send(ts: &TabShare, session: &Option<String>, params: Value) -> BackendResult<Value> {
    ts.backend
        .cdp_session(
            ts.tab,
            session.clone(),
            "Target.setAutoAttach".into(),
            params,
        )
        .await
}

pub(crate) async fn hold(
    ts: &TabShare,
    session: Option<String>,
    holder: &str,
    wait_for_debugger: bool,
) -> BackendResult<bool> {
    let _config = ts.attach.config.lock().await;
    let (previous, wanted, was_on) = {
        let mut data = lock(&ts.attach.data);
        let s = data.entry(session.clone()).or_default();
        let previous = s.held.insert(holder.to_string(), wait_for_debugger);
        let wanted = s.wanted().unwrap_or(false);
        if s.sent == Some(wanted) {
            return Ok(false); // already on with this setting
        }
        (previous, wanted, s.sent.is_some())
    };
    match send(ts, &session, params(true, wanted)).await {
        Ok(_) => {
            if let Some(s) = lock(&ts.attach.data).get_mut(&session) {
                s.sent = Some(wanted);
            }
            Ok(!was_on)
        }
        Err(err) => {
            // Undo: this holder does not hold it (or keeps its old setting).
            let mut data = lock(&ts.attach.data);
            if let Some(s) = data.get_mut(&session) {
                match previous {
                    Some(old) => s.held.insert(holder.to_string(), old),
                    None => s.held.remove(holder),
                };
                if s.held.is_empty() && s.sent.is_none() {
                    data.remove(&session);
                }
            }
            Err(err)
        }
    }
}

pub(crate) async fn unhold(
    ts: &TabShare,
    session: Option<String>,
    holder: &str,
) -> BackendResult<()> {
    let _config = ts.attach.config.lock().await;
    // `Some(wait)` = stay on with this pausing, `None` = switch off.
    let next = {
        let mut data = lock(&ts.attach.data);
        let Some(s) = data.get_mut(&session) else {
            return Ok(());
        };
        if s.held.remove(holder).is_none() {
            return Ok(());
        }
        match s.wanted() {
            // Somebody else still holds it with the same setting.
            Some(wanted) if s.sent == Some(wanted) => return Ok(()),
            Some(wanted) => Some(wanted),
            None => {
                let was_on = s.sent.is_some();
                data.remove(&session);
                if !was_on {
                    return Ok(());
                }
                None
            }
        }
    };
    send(ts, &session, params(next.is_some(), next.unwrap_or(false))).await?;
    if let Some(wait) = next
        && let Some(s) = lock(&ts.attach.data).get_mut(&session)
    {
        s.sent = Some(wait);
    }
    Ok(())
}

pub(crate) async fn reannounce(ts: &TabShare, session: Option<String>) -> BackendResult<()> {
    let _config = ts.attach.config.lock().await;
    let Some(wanted) = lock(&ts.attach.data)
        .get(&session)
        .and_then(Session::wanted)
    else {
        return Ok(()); // nobody holds it: nothing to re-announce
    };
    let _ = send(ts, &session, params(false, false)).await;
    let result = send(ts, &session, params(true, wanted)).await;
    if let Some(s) = lock(&ts.attach.data).get_mut(&session) {
        s.sent = result.is_ok().then_some(wanted);
    }
    result.map(|_| ())
}

/// Forget a child session that went away (its state dies with it in Chromium).
pub(crate) fn forget_session(ts: &TabShare, session: &str) {
    lock(&ts.attach.data).remove(&Some(session.to_string()));
}
