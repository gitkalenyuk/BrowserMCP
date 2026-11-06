//! The background part: one task reading the event bus.
//!
//! * `run.heal_needed` → [`HealService::on_heal_needed`] (propose; ask or auto);
//! * `command.started` from a script (WebSocket caller) with a `tab` argument on `page.*` /
//!   `nav.wait` → remembered as "the tab scripts work in";
//! * `tab.created` / `tab.updated` → the URL of every tab (also after it closes).
//!
//! **Which tab is the row's?** The jobs crate does not know which tab a script used, so the
//! snapshot hook takes the tab of the last script call that named one. With `parallel > 1`
//! that can be another row's tab (same page, usually the same breakage). Scripts often close
//! their tab in `finally`; then the hook re-opens the tab's last URL in a background tab for
//! the snapshot (the state built by earlier steps, e.g. a filled form, is not there).

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use bmcp_core::{Event, TabId};
use bmcp_jobs::HealTrigger;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::HealService;

/// A remembered script tab older than this is not trusted any more.
const TAB_MEMORY: Duration = Duration::from_secs(30 * 60);
/// Size of the snapshot stored for the AI.
const SNAPSHOT_CHARS: usize = 12_000;
/// Forget URLs when this many tabs were seen (a long session).
const MAX_URLS: usize = 2_000;

/// Which tabs scripts use and the URL of every tab.
#[derive(Debug, Default)]
pub struct TabTracker {
    last_script_tab: Option<(TabId, Instant)>,
    urls: HashMap<TabId, String>,
}

impl TabTracker {
    pub fn on_event(&mut self, event: &Event) {
        let d = &event.data;
        match event.topic.as_str() {
            "command.started" => {
                let name = d["name"].as_str().unwrap_or_default();
                let from_script = d["caller"]["kind"] == "ws" || d["caller"]["kind"] == "script";
                let tab = d["args"]["tab"].as_i64();
                if from_script
                    && (name.starts_with("page.") || name == "nav.wait")
                    && let Some(tab) = tab
                {
                    self.last_script_tab = Some((TabId(tab as i32), Instant::now()));
                }
            }
            "tab.created" => self.url(&d["tab"]["id"], &d["tab"]["url"]),
            "tab.updated" => self.url(&d["id"], &d["url"]),
            _ => {}
        }
    }

    fn url(&mut self, id: &Value, url: &Value) {
        if let (Some(id), Some(url)) = (id.as_i64(), url.as_str()) {
            if self.urls.len() > MAX_URLS {
                self.urls.clear();
            }
            self.urls.insert(TabId(id as i32), url.to_string());
        }
    }

    /// The tab of the latest script call (if recent) and its last known URL.
    pub fn script_tab(&self) -> Option<(TabId, Option<String>)> {
        let (tab, at) = self.last_script_tab?;
        (at.elapsed() < TAB_MEMORY).then(|| (tab, self.urls.get(&tab).cloned()))
    }
}

/// The watcher task (see the [module docs](self)). Ends when the service is dropped.
pub(crate) async fn watcher(heal: Weak<HealService>, mut rx: broadcast::Receiver<Event>) {
    loop {
        let event = match rx.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "heal watcher lagged behind the event bus");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        let Some(heal) = heal.upgrade() else { return };
        if event.topic == bmcp_jobs::topics::RUN_HEAL_NEEDED {
            // Proposing calls the AI (seconds to minutes): don't block the event stream.
            let heal = heal.clone();
            tokio::spawn(async move { heal.on_heal_needed(&event.data).await });
        } else {
            heal.state().tabs.on_event(&event);
        }
    }
}

impl HealService {
    /// The jobs snapshot hook: a page snapshot of the broken row's tab, with a header line
    /// saying where it came from. `None` when nothing could be snapshotted.
    pub async fn snapshot_for(&self, trigger: &HealTrigger) -> Option<String> {
        let known = self.state().tabs.script_tab();
        let open: Vec<TabId> = match self.links() {
            Ok(links) => links
                .backend
                .list_tabs()
                .await
                .map(|tabs| tabs.into_iter().map(|t| t.id).collect())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let row = trigger.row;
        match known {
            Some((tab, url)) if open.contains(&tab) => {
                let text = self.snapshot_text(Some(tab)).await?;
                let url = url.unwrap_or_default();
                Some(format!(
                    "(snapshot of tab {tab} {url}, taken right after row {row} failed)\n{text}"
                ))
            }
            Some((_, Some(url))) if url.starts_with("http") || url.starts_with("file:") => {
                self.snapshot_reopened(&url, row).await
            }
            _ => {
                let text = self.snapshot_text(None).await?;
                Some(format!(
                    "(the row's tab is unknown: snapshot of the most recently opened tab after \
                     row {row} failed)\n{text}"
                ))
            }
        }
    }

    async fn snapshot_text(&self, tab: Option<TabId>) -> Option<String> {
        let mut args = json!({ "max_chars": SNAPSHOT_CHARS });
        if let Some(tab) = tab {
            args["tab"] = json!(tab);
        }
        match self.call("page.snapshot", args).await {
            Ok(out) => out["text"].as_str().map(str::to_string),
            Err(err) => {
                tracing::warn!(%err, "heal snapshot failed");
                None
            }
        }
    }

    /// The script closed its tab: open its last URL in the background, snapshot, close.
    async fn snapshot_reopened(&self, url: &str, row: u64) -> Option<String> {
        let opened = self
            .call("tabs.open", json!({ "url": url, "background": true }))
            .await
            .ok()?;
        let tab = opened["tab"].as_i64()?;
        // Best effort: older builds or tests may lack nav.wait.
        let _ = self
            .call(
                "nav.wait",
                json!({ "tab": tab, "for": "load", "timeout_secs": 15 }),
            )
            .await;
        let text = self.snapshot_text(Some(TabId(tab as i32))).await;
        let _ = self.call("tabs.close", json!({ "tab": tab })).await;
        Some(format!(
            "(the script closed its tab; this is {url} re-opened after row {row} failed: state \
             from earlier steps such as filled fields is missing)\n{}",
            text?
        ))
    }
}

/// For tests: feed events to a tracker without a running watcher.
pub fn track(heal: &Arc<HealService>, event: &Event) {
    heal.state().tabs.on_event(event);
}
