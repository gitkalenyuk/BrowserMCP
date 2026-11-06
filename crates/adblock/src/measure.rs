//! `simplify.measure`: how heavy is a page? Load it once and count requests, bytes and time.
//!
//! Used to compare a page with and without `adblock.apply` / site policies. While measuring,
//! the HTTP cache is disabled (`Network.setCacheDisabled`) so every run downloads everything
//! and runs are comparable. Bytes are CDP's `encodedDataLength` (what went over the wire,
//! headers included) — unlike `performance` resource timing this also counts cross-origin
//! resources without `Timing-Allow-Origin`.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bmcp_cdpshare::CdpShare;
use bmcp_core::{CmdError, Command, Ctx, Meta, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

pub struct Measure;

/// Numbers the measurements, for their domain holder names.
static NEXT_MEASURE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeasureArgs {
    /// Page to load.
    pub url: String,
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Keep counting this long after the load event, for late requests (default 3000).
    pub settle_ms: Option<u64>,
    /// Give up waiting for the load event after this long (default 45000).
    pub timeout_ms: Option<u64>,
}

/// Requests and bytes of one resource type.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct TypeCost {
    pub requests: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MeasureOutput {
    pub tab: TabId,
    pub url: String,
    #[serde(flatten)]
    pub cost: PageCost,
}

/// What loading the page cost.
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
pub struct PageCost {
    /// Requests the page started.
    pub requests: u64,
    /// Requests that finished loading.
    pub finished: u64,
    /// Requests that failed (including blocked ones).
    pub failed: u64,
    /// Failed with `net::ERR_BLOCKED_BY_CLIENT` (adblock / site policy).
    pub blocked: u64,
    /// Bytes received over the network (`encodedDataLength`, headers included).
    pub bytes: u64,
    /// Time until the `load` event, milliseconds.
    pub load_ms: u64,
    /// Time until `DOMContentLoaded`, milliseconds (0 if not seen).
    pub dom_content_loaded_ms: u64,
    /// The load event did not come within `timeout_ms`.
    pub timed_out: bool,
    /// `Performance.getMetrics` after settling.
    pub js_heap_used_bytes: u64,
    pub dom_nodes: u64,
    pub by_type: BTreeMap<String, TypeCost>,
}

/// Tallies filled by the event collector.
#[derive(Default)]
struct Tally {
    types: HashMap<String, String>,
    out: PageCost,
    dom_content_loaded: Option<Instant>,
}

impl Command for Measure {
    const NAME: &'static str = "simplify.measure";
    type Args = MeasureArgs;
    type Output = MeasureOutput;

    fn meta() -> Meta {
        Meta::new(
            "Load a URL in a tab with the HTTP cache off and report its cost: requests, \
             blocked requests, bytes over the network, load time, JS heap, DOM nodes, by \
             resource type. Use to compare with/without adblock_apply or site policies.",
            Toolset::Data,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &MeasureArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: MeasureArgs) -> Result<MeasureOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let cdp = |method: &'static str, params: Value| {
            let backend = ctx.backend.clone();
            async move { backend.cdp(tab, method.to_string(), params).await }
        };
        let mut events = ctx.backend.cdp_events(tab).await?;
        // Network and Page are shared with other users of the tab (net capture, recordings):
        // hold them through the refcount, so our cleanup never switches them off for others.
        let share = CdpShare::from_ctx(&ctx);
        let holder = format!(
            "adblock.measure:{}",
            NEXT_MEASURE.fetch_add(1, Ordering::Relaxed)
        );
        let setup = async {
            share
                .acquire(&ctx.backend, tab, "Network", &holder, json!({}))
                .await?;
            cdp("Network.setCacheDisabled", json!({ "cacheDisabled": true })).await?;
            share
                .acquire(&ctx.backend, tab, "Page", &holder, json!({}))
                .await
        };
        if let Err(err) = setup.await {
            let _ = share.release(&ctx.backend, tab, "Network", &holder).await;
            return Err(err.into());
        }
        let _ = cdp("Performance.enable", json!({})).await;

        let tally = Arc::new(Mutex::new(Tally::default()));
        let collector = {
            let tally = tally.clone();
            tokio::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) if event.session_id.is_none() => count(
                            &mut tally.lock().unwrap_or_else(|e| e.into_inner()),
                            &event.method,
                            &event.params,
                        ),
                        Ok(_) | Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => break,
                    }
                }
            })
        };

        let started = Instant::now();
        let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(45_000));
        let navigated = ctx.backend.navigate(tab, args.url.clone(), timeout).await;
        let load_ms = started.elapsed().as_millis() as u64;
        tokio::time::sleep(Duration::from_millis(args.settle_ms.unwrap_or(3000))).await;
        collector.abort();

        let metrics = cdp("Performance.getMetrics", json!({})).await.ok();
        // Leave the tab as we found it (best effort).
        let _ = cdp("Performance.disable", json!({})).await;
        let _ = cdp(
            "Network.setCacheDisabled",
            json!({ "cacheDisabled": false }),
        )
        .await;
        let _ = share.release(&ctx.backend, tab, "Network", &holder).await;
        let _ = share.release(&ctx.backend, tab, "Page", &holder).await;

        let mut tally = tally.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = std::mem::take(&mut tally.out);
        out.timed_out = navigated.is_err();
        out.load_ms = load_ms;
        out.dom_content_loaded_ms = tally.dom_content_loaded.map_or(0, |t| {
            t.saturating_duration_since(started).as_millis() as u64
        });
        let metric = |name: &str| -> u64 {
            metrics
                .as_ref()
                .and_then(|m| m["metrics"].as_array())
                .and_then(|list| list.iter().find(|m| m["name"] == name))
                .and_then(|m| m["value"].as_f64())
                .map_or(0, |v| v as u64)
        };
        out.js_heap_used_bytes = metric("JSHeapUsedSize");
        out.dom_nodes = metric("Nodes");
        Ok(MeasureOutput {
            tab,
            url: args.url,
            cost: out,
        })
    }
}

/// Count one Network/Page event.
fn count(tally: &mut Tally, method: &str, params: &Value) {
    let id = params["requestId"].as_str().unwrap_or_default().to_string();
    match method {
        "Network.requestWillBeSent" => {
            // Redirects reuse the request id: count the request once.
            if params.get("redirectResponse").is_some() {
                return;
            }
            let ty = params["type"].as_str().unwrap_or("Other").to_string();
            tally.out.requests += 1;
            tally.out.by_type.entry(ty.clone()).or_default().requests += 1;
            tally.types.insert(id, ty);
        }
        "Network.loadingFinished" => {
            let Some(ty) = tally.types.get(&id).cloned() else {
                return; // started before we were measuring
            };
            let bytes = params["encodedDataLength"].as_f64().unwrap_or(0.0) as u64;
            tally.out.finished += 1;
            tally.out.bytes += bytes;
            tally.out.by_type.entry(ty).or_default().bytes += bytes;
        }
        "Network.loadingFailed" => {
            if !tally.types.contains_key(&id) {
                return;
            }
            tally.out.failed += 1;
            let text = params["errorText"].as_str().unwrap_or_default();
            if text.contains("BLOCKED_BY_CLIENT") || params.get("blockedReason").is_some() {
                tally.out.blocked += 1;
            }
        }
        "Page.domContentEventFired" if tally.dom_content_loaded.is_none() => {
            tally.dom_content_loaded = Some(Instant::now());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_requests_bytes_and_blocks() {
        let mut t = Tally::default();
        count(
            &mut t,
            "Network.requestWillBeSent",
            &json!({"requestId": "1", "type": "Script"}),
        );
        count(
            &mut t,
            "Network.requestWillBeSent",
            &json!({"requestId": "1", "type": "Script", "redirectResponse": {}}),
        );
        count(
            &mut t,
            "Network.requestWillBeSent",
            &json!({"requestId": "2", "type": "Image"}),
        );
        count(
            &mut t,
            "Network.loadingFinished",
            &json!({"requestId": "1", "encodedDataLength": 1500.0}),
        );
        count(
            &mut t,
            "Network.loadingFailed",
            &json!({"requestId": "2", "errorText": "net::ERR_BLOCKED_BY_CLIENT"}),
        );
        // Unknown ids (requests from before the measurement) are ignored.
        count(
            &mut t,
            "Network.loadingFinished",
            &json!({"requestId": "old", "encodedDataLength": 99.0}),
        );
        let o = &t.out;
        assert_eq!(
            (o.requests, o.finished, o.failed, o.blocked, o.bytes),
            (2, 1, 1, 1, 1500)
        );
        assert_eq!(o.by_type["Script"].bytes, 1500);
        assert_eq!(o.by_type["Image"].requests, 1);
    }
}
