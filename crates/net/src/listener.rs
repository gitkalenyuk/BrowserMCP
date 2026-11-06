//! The background task that reads one tab's `Network.*` events into its
//! [`crate::store::TrafficStore`] while capture is on.
//!
//! Paused requests (`Fetch.requestPaused`) do **not** come through here: the tab's Fetch
//! coordinator (`bmcp-cdpshare`) reads them from its own subscription and hands them to
//! [`crate::intercept::NetFetch`]. So when this listener falls behind on a busy page and loses
//! events, only traffic entries are incomplete — no request is left paused.
//!
//! It runs while capture is on, and ends by itself when the tab closes (the event channel
//! closes).

use bmcp_core::CdpEvents;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::task::JoinHandle;

use crate::state::Env;
use crate::store::body_from_cdp;

pub fn spawn(env: Env, mut events: CdpEvents) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    // Events of child sessions (iframes, workers) are not ours: we only
                    // enable Network/Fetch on the page target itself.
                    if event.session_id.is_some() {
                        continue;
                    }
                    handle(&env, &event.method, event.params);
                }
                Err(RecvError::Lagged(missed)) => {
                    // We were too slow and lost events: some captured requests stay
                    // incomplete. (Paused requests are not affected, see the module docs.)
                    env.events.emit(
                        "net.lagged",
                        json!({ "tab": env.tab, "missed": missed,
                                "warning": "traffic events were dropped; some entries are incomplete" }),
                    );
                }
                Err(RecvError::Closed) => break,
            }
        }
        // The tab is gone: forget what is paused there (Chromium dropped those requests).
        env.tn.lock().paused.clear();
    })
}

fn handle(env: &Env, method: &str, params: serde_json::Value) {
    if !method.starts_with("Network.") {
        return;
    }
    let fetch_body = {
        let mut inner = env.tn.lock();
        if !inner.capture {
            return;
        }
        inner.store.apply(method, &params);
        // With `bodies: true`, download each body as soon as it is complete (Chromium drops
        // bodies from its buffer later, e.g. after navigations).
        if inner.bodies && method == "Network.loadingFinished" {
            let request_id = params["requestId"].as_str().unwrap_or_default();
            inner
                .store
                .id_for(request_id)
                .filter(|id| {
                    inner
                        .store
                        .get(*id)
                        .is_some_and(|e| e.body.is_none() && e.resource_type != "websocket")
                })
                .map(|id| (id, request_id.to_string()))
        } else {
            None
        }
    };
    if let Some((id, request_id)) = fetch_body {
        let env = env.clone();
        tokio::spawn(async move {
            let reply = env
                .cdp(
                    "Network.getResponseBody",
                    json!({ "requestId": request_id }),
                )
                .await;
            // No body (204, redirects, evicted…) is normal; just leave it empty.
            if let Ok(reply) = reply {
                env.tn.lock().store.set_body(id, body_from_cdp(&reply));
            }
        });
    }
}
