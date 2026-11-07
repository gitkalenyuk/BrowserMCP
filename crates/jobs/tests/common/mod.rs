//! Shared test setup: a JobsService on a temp folder with the fake script host, called
//! through a real Registry (so argument schemas are exercised too).

#![allow(dead_code)] // each test file uses a different part

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bmcp_core::testing::FakeBackend;
use bmcp_core::{Caller, CmdError, Ctx, Event, EventBus, Registry};
use bmcp_jobs::{JobsConfig, JobsService};
use serde_json::{Value, json};
use tokio::sync::broadcast;

pub const WS_URL: &str = "ws://127.0.0.1:1/v1/ws";

pub struct Env {
    pub tmp: tempfile::TempDir,
    pub registry: Arc<Registry>,
    pub jobs: JobsService,
    pub events: EventBus,
}

pub fn fake_host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bmcp-fake-script-host"))
}

pub fn config(root: &Path) -> JobsConfig {
    JobsConfig {
        scripts_dir: root.join("scripts"),
        runs_dir: root.join("runs"),
        db_path: root.join("db/runs.sqlite"),
        host_exe: Some(fake_host()),
        ws_url: WS_URL.into(),
        retry_backoff: Duration::from_millis(10),
        auto_limit: false,
        max_parallel: 32,
        // The fake host always honours its own timeout (exit 124). A long grace keeps a slow
        // process start on a loaded machine from turning that into "killed by us".
        kill_grace: Duration::from_secs(60),
        settings_path: Some(root.join("config/scripts.json")),
    }
}

/// Generous upper bound for anything a test waits for: only reached when something is
/// really broken, never because the machine is busy.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// Wait until `path` exists (a script ran `touch <path>`).
pub async fn wait_file(path: &Path) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while !path.exists() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Path as text for a fake-host script line (forward slashes: no escaping questions).
pub fn script_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

impl Env {
    pub fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let config = config(tmp.path());
        Env::with_config(tmp, config)
    }

    pub fn with_config(tmp: tempfile::TempDir, config: JobsConfig) -> Env {
        let events = EventBus::new();
        let jobs = JobsService::open(config, events.clone()).unwrap();
        let mut registry = Registry::new();
        registry.extensions().insert(jobs.clone());
        bmcp_jobs::register(&mut registry);
        Env {
            tmp,
            registry: Arc::new(registry),
            jobs,
            events,
        }
    }

    /// A second service on the same folder (a "restarted browser").
    pub fn reopen(self) -> Env {
        let config = config(self.tmp.path());
        Env::with_config(self.tmp, config)
    }

    pub fn root(&self) -> &Path {
        self.tmp.path()
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        let ctx = Ctx::new(Caller::Rest, FakeBackend::new(), self.events.clone());
        self.registry.call(ctx, name, args).await
    }

    pub async fn ok(&self, name: &str, args: Value) -> Value {
        match self.call(name, args.clone()).await {
            Ok(v) => v,
            Err(e) => panic!("{name} {args} failed: {e:?}"),
        }
    }

    pub async fn save(&self, name: &str, code: &str) {
        self.ok("script.save", json!({"name": name, "code": code}))
            .await;
    }

    /// Poll run.status until `status` is one of `wanted`.
    pub async fn wait_status(&self, id: &str, wanted: &[&str]) -> Value {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let run = self.ok("run.status", json!({"id": id})).await;
            if wanted.contains(&run["status"].as_str().unwrap()) {
                return run;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "run never reached {wanted:?}: {run:#}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// All rows of a run (one page of up to 1000).
    pub async fn rows(&self, id: &str) -> Vec<Value> {
        let page = self
            .ok("run.results", json!({"id": id, "limit": 1000}))
            .await;
        page["rows"].as_array().unwrap().clone()
    }
}

/// Wait for the next event on `topic` whose data matches `pred`.
pub async fn wait_event(
    rx: &mut broadcast::Receiver<Event>,
    topic: &str,
    pred: impl Fn(&Value) -> bool,
) -> Event {
    tokio::time::timeout(PATIENCE, async {
        loop {
            let event = rx.recv().await.unwrap();
            if event.topic == topic && pred(&event.data) {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {topic} event"))
}

/// `[{"id":"0"}, {"id":"1"}, …]`
pub fn id_rows(n: usize) -> Value {
    Value::Array((0..n).map(|i| json!({"id": i.to_string()})).collect())
}
