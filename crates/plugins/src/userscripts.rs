//! Userscripts: `data/userscripts/<name>.ts` run on matching page loads.
//!
//! [`watch`] listens to `tab.updated` on the event bus. [`NavTracker`] turns that stream into
//! navigation phases per tab: "the new URL is known" (for `@run-at document-start`) and "the
//! load finished" (`document-end` / `document-idle`). Each phase fires once per navigation, so
//! the many `tab.updated` events of one load (title, favicon, history flags) never run a
//! script twice; a reload or a new URL is a new navigation. A URL change without loading (a
//! single-page app's `history.pushState`) also counts as a finished navigation.
//!
//! A run is `bmcp-script-host run <file> --ws <url> --input tab=<id> --input url=<url>`
//! (the jobs runner's [`bmcp_jobs::host::run_host`]), at most [`MAX_PARALLEL`] at once and
//! never two of the same script in the same tab. Userscripts in `data/userscripts` are the
//! user's own and use the normal (unrestricted) connection; userscripts shipped by a plugin
//! run over that plugin's restricted connection, with its permissions.
//!
//! A userscript changes the page with `userscripts.inject {tab, js}` (JavaScript in an isolated
//! world of the tab: shared DOM, separate globals), or any other command.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::events::topics::{TAB_CLOSED, TAB_UPDATED};
use bmcp_core::{CmdError, ErrorCode};
use bmcp_jobs::host::{HostOutcome, HostRun, ScriptFailure, locate_host, run_host};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, broadcast};

use crate::matching::Header;
use crate::{Inner, io_err, load_disabled, lock, now_ms, save_disabled, topics};

/// Userscript processes at once (across all tabs).
pub const MAX_PARALLEL: usize = 4;

/// Navigation state of one tab, as far as `tab.updated` tells.
#[derive(Debug, Default)]
struct TabNav {
    url: String,
    loading: bool,
    /// `document-start` scripts already ran for the current navigation.
    start_done: bool,
    /// URL of the last finished navigation.
    last_url: String,
}

/// What one `tab.updated` completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavPhases {
    pub tab: i64,
    pub url: String,
    /// Run the `document-start` scripts now.
    pub start: bool,
    /// Run the `document-end` / `document-idle` scripts now.
    pub end: bool,
}

/// Turns `tab.updated` payloads into navigation phases (see the module docs).
#[derive(Debug, Default)]
pub struct NavTracker {
    tabs: HashMap<i64, TabNav>,
}

impl NavTracker {
    /// Feed one `tab.updated {id, url, loading, …}`; `None` when nothing should run.
    pub fn update(&mut self, data: &Value) -> Option<NavPhases> {
        let tab = data.get("id")?.as_i64()?;
        let nav = self.tabs.entry(tab).or_default();
        let (was_loading, old_url) = (nav.loading, nav.url.clone());
        if let Some(url) = data.get("url").and_then(Value::as_str) {
            nav.url = url.to_string();
        }
        if let Some(loading) = data.get("loading").and_then(Value::as_bool) {
            nav.loading = loading;
        }
        let (mut start, mut end) = (false, false);
        if nav.loading {
            // The new URL is known once it differs from the page we had (a reload keeps the
            // URL; its document-start scripts run when the load finishes, below).
            if !nav.start_done && !nav.url.is_empty() && nav.url != nav.last_url {
                start = true;
                nav.start_done = true;
            }
        } else if was_loading || nav.url != old_url {
            end = true;
            start = !nav.start_done;
            nav.start_done = false;
            nav.last_url = nav.url.clone();
        }
        (start || end).then(|| NavPhases {
            tab,
            url: nav.url.clone(),
            start,
            end,
        })
    }

    pub fn closed(&mut self, tab: i64) {
        self.tabs.remove(&tab);
    }
}

/// The last run of a userscript.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LastRun {
    pub tab: i64,
    pub url: String,
    pub ok: bool,
    pub duration_ms: u64,
    pub at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ScriptFailure>,
}

/// Runtime state of the userscript feature.
pub struct UserscriptRuntime {
    /// `(script key, tab)` running right now.
    running: Mutex<HashSet<(String, i64)>>,
    last_runs: Mutex<HashMap<String, LastRun>>,
    slots: Arc<Semaphore>,
}

impl Default for UserscriptRuntime {
    fn default() -> Self {
        UserscriptRuntime {
            running: Mutex::new(HashSet::new()),
            last_runs: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(MAX_PARALLEL)),
        }
    }
}

/// A userscript as `userscripts.list` shows it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UserscriptInfo {
    /// File name without `.ts`; `<plugin>/<file>` for a plugin's userscript.
    pub name: String,
    /// `user` (data/userscripts) or `plugin:<name>`.
    pub source: String,
    pub enabled: bool,
    pub path: String,
    /// Parsed header; missing when it is invalid (see `error`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub header: Option<Header>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<LastRun>,
}

/// One script that may run: where it is and how it connects.
struct Candidate {
    name: String,
    source: String,
    path: PathBuf,
    header: Header,
    ws_url: String,
}

/// `name` rules for userscripts in `data/userscripts`: like saved scripts.
pub fn validate_name(name: &str) -> Result<(), CmdError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(CmdError::invalid_params(format!(
            "bad userscript name {name:?}: use 1-64 chars of A-Z a-z 0-9 _ - (no .ts)"
        )))
    }
}

impl Inner {
    fn userscript_path(&self, name: &str) -> Result<PathBuf, CmdError> {
        validate_name(name)?;
        Ok(self.config.userscripts_dir.join(format!("{name}.ts")))
    }

    /// Every userscript: the user's files, then those of running plugins.
    pub fn list_userscripts(&self) -> Vec<UserscriptInfo> {
        let disabled = load_disabled(&self.config.userscripts_dir);
        let last_runs = lock(&self.userscripts.last_runs).clone();
        let mut out = Vec::new();
        let mut add = |name: String, source: String, path: &Path, enabled: bool| {
            let parsed = std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|code| Header::parse(&code));
            out.push(UserscriptInfo {
                last_run: last_runs.get(&name).cloned(),
                path: path.display().to_string(),
                enabled,
                header: parsed.as_ref().ok().cloned(),
                error: parsed.err(),
                name,
                source,
            });
        };
        for (name, path) in self.user_files() {
            let enabled = !disabled.contains(&name);
            add(name, "user".into(), &path, enabled);
        }
        for (plugin, path, _) in self.plugin_userscripts() {
            let file = path.file_stem().unwrap_or_default().to_string_lossy();
            add(
                format!("{plugin}/{file}"),
                format!("plugin:{plugin}"),
                &path,
                true,
            );
        }
        out
    }

    /// `(name, path)` of `data/userscripts/*.ts`, sorted.
    fn user_files(&self) -> Vec<(String, PathBuf)> {
        let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(&self.config.userscripts_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_name()?.to_str()?.strip_suffix(".ts")?.to_string();
                (!name.starts_with('.') && path.is_file()).then_some((name, path))
            })
            .collect();
        files.sort();
        files
    }

    pub fn save_userscript(&self, name: &str, code: &str) -> Result<UserscriptInfo, CmdError> {
        let path = self.userscript_path(name)?;
        Header::parse(code).map_err(|e| {
            CmdError::invalid_params(format!("userscript {name}: {e}"))
                .with_hint("Start the file with a header like `// @match https://example.com/*`.")
        })?;
        std::fs::write(&path, code).map_err(|e| io_err(&path, e))?;
        self.list_userscripts()
            .into_iter()
            .find(|u| u.source == "user" && u.name == name)
            .ok_or_else(|| CmdError::internal("saved userscript not listed"))
    }

    pub fn delete_userscript(&self, name: &str) -> Result<bool, CmdError> {
        let path = self.userscript_path(name)?;
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path).map_err(|e| io_err(&path, e))?;
        let mut disabled = load_disabled(&self.config.userscripts_dir);
        if disabled.remove(name) {
            save_disabled(&self.config.userscripts_dir, &disabled)?;
        }
        Ok(true)
    }

    pub fn enable_userscript(&self, name: &str, enabled: bool) -> Result<(), CmdError> {
        let path = self.userscript_path(name)?;
        if !path.is_file() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                format!("userscript {name} not found"),
            ));
        }
        let mut disabled = load_disabled(&self.config.userscripts_dir);
        let changed = if enabled {
            disabled.remove(name)
        } else {
            disabled.insert(name.to_string())
        };
        if changed {
            save_disabled(&self.config.userscripts_dir, &disabled)?;
        }
        Ok(())
    }

    /// Enabled scripts that want to run for this phase of a navigation to `url`. The folder is
    /// read every time (a few small files), so edits apply to the next page load.
    fn candidates(&self, url: &str, phases: &NavPhases) -> Vec<Candidate> {
        let disabled = load_disabled(&self.config.userscripts_dir);
        let base = self.ws_url();
        let user = self
            .user_files()
            .into_iter()
            .filter(|(name, _)| !disabled.contains(name))
            .map(|(name, path)| (name, "user".to_string(), path, base.clone()));
        let from_plugins = self
            .plugin_userscripts()
            .into_iter()
            .map(|(plugin, path, ws)| {
                let file = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                (
                    format!("{plugin}/{file}"),
                    format!("plugin:{plugin}"),
                    path,
                    ws,
                )
            });
        user.chain(from_plugins)
            .filter_map(|(name, source, path, ws_url)| {
                let code = std::fs::read_to_string(&path).ok()?;
                let header = Header::parse(&code).ok()?;
                let phase_ok = if header.run_at.after_load() {
                    phases.end
                } else {
                    phases.start
                };
                (phase_ok && header.applies_to(url)).then_some(Candidate {
                    name,
                    source,
                    path,
                    header,
                    ws_url,
                })
            })
            .collect()
    }
}

/// Watch `tab.updated` and start matching userscripts. Runs until shutdown.
pub(crate) async fn watch(inner: Arc<Inner>) {
    let mut events = inner.events.subscribe();
    let mut tracker = NavTracker::default();
    loop {
        let event = tokio::select! {
            event = events.recv() => event,
            _ = inner.shutdown.cancelled() => return,
        };
        let event = match event {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "userscripts fell behind the event bus");
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => return,
        };
        match event.topic.as_str() {
            TAB_UPDATED => {
                if let Some(phases) = tracker.update(&event.data) {
                    for candidate in inner.candidates(&phases.url, &phases) {
                        spawn_run(&inner, candidate, phases.tab, phases.url.clone());
                    }
                }
            }
            TAB_CLOSED => {
                if let Some(tab) = event.data.get("id").and_then(Value::as_i64) {
                    tracker.closed(tab);
                }
            }
            _ => {}
        }
    }
}

fn spawn_run(inner: &Arc<Inner>, candidate: Candidate, tab: i64, url: String) {
    let key = (candidate.name.clone(), tab);
    if !lock(&inner.userscripts.running).insert(key.clone()) {
        tracing::debug!(script = %candidate.name, tab, "userscript still running; skipped");
        return;
    }
    let inner = inner.clone();
    tokio::spawn(async move {
        let outcome = run_one(&inner, &candidate, tab, &url).await;
        lock(&inner.userscripts.running).remove(&key);
        let (ok, error, duration_ms) = match &outcome {
            Ok(o) => (o.ok, o.error.clone(), o.duration_ms),
            Err(err) => (
                false,
                Some(ScriptFailure {
                    code: err.code.as_str().to_string(),
                    command: None,
                    message: err.message.clone(),
                    line: None,
                }),
                0,
            ),
        };
        if let Some(err) = &error {
            tracing::warn!(script = %candidate.name, tab, %url, error = %err.message, "userscript failed");
        }
        lock(&inner.userscripts.last_runs).insert(
            candidate.name.clone(),
            LastRun {
                tab,
                url: url.clone(),
                ok,
                duration_ms,
                at_ms: now_ms(),
                error: error.clone(),
            },
        );
        inner.events.emit(
            topics::USERSCRIPT_RAN,
            json!({ "name": candidate.name, "source": candidate.source, "tab": tab,
                    "url": url, "ok": ok, "duration_ms": duration_ms, "error": error }),
        );
    });
}

async fn run_one(
    inner: &Arc<Inner>,
    candidate: &Candidate,
    tab: i64,
    url: &str,
) -> Result<HostOutcome, CmdError> {
    let exe = locate_host(inner.config.host_exe.as_deref())?;
    let _slot = inner
        .userscripts
        .slots
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| CmdError::internal("userscript slots closed"))?;
    let timeout = candidate
        .header
        .timeout_secs
        .map_or(inner.config.userscript_timeout, Duration::from_secs);
    let inputs = [
        ("tab".to_string(), tab.to_string()),
        ("url".to_string(), url.to_string()),
    ];
    let cancel = inner.shutdown.child_token();
    Ok(run_host(
        HostRun {
            exe: &exe,
            ws_url: &candidate.ws_url,
            script: &candidate.path,
            inputs: &inputs,
            env: &[],
            timeout,
        },
        &cancel,
    )
    .await)
}

/// `userscripts.inject`: run `js` (the body of an async function) in an isolated world of the
/// tab's main frame and return what it returns (JSON). No `Runtime.enable`: the page cannot
/// see the isolated world, only the DOM changes it makes.
pub async fn inject(
    ctx: &bmcp_core::Ctx,
    tab: bmcp_core::TabId,
    js: &str,
) -> Result<Value, CmdError> {
    let frames = ctx
        .backend
        .cdp(tab, "Page.getFrameTree".into(), json!({}))
        .await?;
    let frame_id = frames["frameTree"]["frame"]["id"]
        .as_str()
        .ok_or_else(|| CmdError::internal("Page.getFrameTree returned no main frame"))?
        .to_string();
    let world = ctx
        .backend
        .cdp(
            tab,
            "Page.createIsolatedWorld".into(),
            json!({ "frameId": frame_id, "worldName": "bmcp-userscripts" }),
        )
        .await?;
    let context = world["executionContextId"]
        .as_i64()
        .ok_or_else(|| CmdError::internal("Page.createIsolatedWorld returned no context"))?;
    let reply = ctx
        .backend
        .cdp(
            tab,
            "Runtime.evaluate".into(),
            json!({
                "expression": format!("(async () => {{\n{js}\n}})()"),
                "contextId": context,
                "awaitPromise": true,
                "returnByValue": true,
            }),
        )
        .await?;
    if let Some(details) = reply.get("exceptionDetails") {
        let message = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("JavaScript exception");
        return Err(CmdError::invalid_params(format!(
            "injected script threw: {message}"
        )));
    }
    Ok(reply["result"]["value"].clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upd(id: i64, url: &str, loading: bool) -> Value {
        json!({ "id": id, "url": url, "loading": loading, "title": "t" })
    }

    fn phases(t: &mut NavTracker, v: Value) -> Option<(bool, bool)> {
        t.update(&v).map(|p| (p.start, p.end))
    }

    #[test]
    fn one_run_per_navigation() {
        let mut t = NavTracker::default();
        // New tab: loading starts with the URL known → start; load finishes → end.
        assert_eq!(
            phases(&mut t, upd(1, "https://a.test/", true)),
            Some((true, false))
        );
        assert_eq!(phases(&mut t, upd(1, "https://a.test/", true)), None);
        assert_eq!(
            phases(&mut t, upd(1, "https://a.test/", false)),
            Some((false, true))
        );
        // Title / favicon updates of the loaded page: nothing.
        assert_eq!(phases(&mut t, upd(1, "https://a.test/", false)), None);
        // Reload (same URL): start + end together when it finishes.
        assert_eq!(phases(&mut t, upd(1, "https://a.test/", true)), None);
        assert_eq!(
            phases(&mut t, upd(1, "https://a.test/", false)),
            Some((true, true))
        );
        // Navigation where `loading` comes before the new address.
        assert_eq!(phases(&mut t, upd(1, "https://a.test/", true)), None);
        assert_eq!(
            phases(&mut t, upd(1, "https://b.test/", true)),
            Some((true, false))
        );
        assert_eq!(
            phases(&mut t, upd(1, "https://b.test/", false)),
            Some((false, true))
        );
        // pushState: new URL without loading → one finished navigation.
        assert_eq!(
            phases(&mut t, upd(1, "https://b.test/2", false)),
            Some((true, true))
        );
        // Other tabs are independent; a closed tab starts over.
        assert_eq!(
            phases(&mut t, upd(2, "https://b.test/2", true)),
            Some((true, false))
        );
        t.closed(1);
        assert_eq!(
            phases(&mut t, upd(1, "https://b.test/2", false)),
            Some((true, true))
        );
        // Payload without an id is ignored.
        assert_eq!(t.update(&json!({ "url": "x" })), None);
    }

    #[test]
    fn names() {
        assert!(validate_name("example-banner_2").is_ok());
        for bad in ["", "../x", "a.ts", "a b", &"x".repeat(65)] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }
}
