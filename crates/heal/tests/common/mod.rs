//! Test setup: jobs (with the fake script host), heal (with canned AI answers), fake
//! `page.*` commands, a FakeBackend and a real Registry. No real (paid) model calls.

#![allow(dead_code)] // each test file uses a different part

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_ai::oneshot::{Completion, TextRequest};
use bmcp_core::testing::FakeBackend;
use bmcp_core::{
    BoxFuture, Caller, CmdError, Command, Ctx, Event, EventBus, Meta, Registry, TabId, Toolset,
};
use bmcp_heal::{HealConfig, HealService, Llm};
use bmcp_jobs::{JobsConfig, JobsService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;

pub fn fake_host() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bmcp-heal-fake-host"))
}

/// Canned AI answers, in order; every request is recorded.
#[derive(Default)]
pub struct FakeLlm {
    pub answers: Mutex<VecDeque<String>>,
    pub requests: Mutex<Vec<TextRequest>>,
}

impl FakeLlm {
    pub fn new(answers: &[String]) -> Arc<FakeLlm> {
        Arc::new(FakeLlm {
            answers: Mutex::new(answers.iter().cloned().collect()),
            requests: Mutex::default(),
        })
    }

    pub fn prompts(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.prompt.clone())
            .collect()
    }
}

impl Llm for FakeLlm {
    fn complete(&self, req: TextRequest) -> BoxFuture<'static, Result<Completion, CmdError>> {
        self.requests.lock().unwrap().push(req);
        let answer = self.answers.lock().unwrap().pop_front();
        Box::pin(async move {
            let text = answer.ok_or_else(|| CmdError::internal("no more canned answers"))?;
            Ok(Completion {
                provider: "fake".into(),
                model: Some("fake-1".into()),
                text,
            })
        })
    }
}

/// A heal answer as the model would send it.
pub fn answer(diff: &str, confidence: f64) -> String {
    json!({"diff": diff, "explanation": "the button is now called Log in", "confidence": confidence})
        .to_string()
}

// ------------------------------------------------------------------ fake page commands

/// Text the fake `page.snapshot` / `page.read` / `page.find` return.
#[derive(Default)]
pub struct Page {
    pub snapshot: Mutex<String>,
    pub read: Mutex<String>,
    pub find: Mutex<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnyArgs {
    pub tab: Option<TabId>,
    #[serde(flatten)]
    pub rest: serde_json::Map<String, Value>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TextOut {
    pub tab: TabId,
    pub text: String,
    pub total: usize,
}

macro_rules! fake_page_command {
    ($ty:ident, $name:literal, $field:ident) => {
        pub struct $ty;
        impl Command for $ty {
            const NAME: &'static str = $name;
            type Args = AnyArgs;
            type Output = TextOut;
            fn meta() -> Meta {
                Meta::new("test double", Toolset::Core)
            }
            async fn run(ctx: Ctx, args: AnyArgs) -> Result<TextOut, CmdError> {
                let page = ctx.extensions.get::<Page>().unwrap();
                let text = page.$field.lock().unwrap().clone();
                Ok(TextOut {
                    tab: args.tab.unwrap_or(TabId(1)),
                    total: text.lines().count(),
                    text,
                })
            }
        }
    };
}
fake_page_command!(FakeSnapshot, "page.snapshot", snapshot);
fake_page_command!(FakeRead, "page.read", read);
fake_page_command!(FakeFind, "page.find", find);

// ------------------------------------------------------------------ environment

pub struct Env {
    pub tmp: tempfile::TempDir,
    pub registry: Arc<Registry>,
    pub jobs: JobsService,
    pub heal: Arc<HealService>,
    pub events: EventBus,
    pub backend: Arc<FakeBackend>,
    pub page: Arc<Page>,
}

impl Env {
    /// `llm = None`: the real AI path (bmcp_ai::AiService, configure a provider first).
    pub fn new(llm: Option<Arc<FakeLlm>>) -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let events = EventBus::new();
        let jobs = JobsService::open(
            JobsConfig {
                scripts_dir: root.join("scripts"),
                runs_dir: root.join("runs"),
                db_path: root.join("db/runs.sqlite"),
                host_exe: Some(fake_host()),
                ws_url: "ws://127.0.0.1:1/v1/ws".into(),
                retry_backoff: Duration::from_millis(10),
                auto_limit: false,
                max_parallel: 8,
                // Long: the fake host honours its own timeout; a slow start on a busy
                // machine must not turn that into a kill.
                kill_grace: Duration::from_secs(60),
            },
            events.clone(),
        )
        .unwrap();

        let mut config = HealConfig::in_dir(root);
        config.host_exe = Some(fake_host());
        let mut heal = HealService::new(config);
        if let Some(llm) = llm {
            heal = heal.with_llm(llm);
        }

        let mut registry = Registry::new();
        bmcp_core::commands::register_builtin(&mut registry);
        registry.extensions().insert(jobs.clone());
        bmcp_jobs::register(&mut registry);
        registry
            .extensions()
            .insert(bmcp_ai::AiService::new(root, "http://127.0.0.1:9/mcp"));
        bmcp_ai::register(&mut registry);
        let heal = registry.extensions().insert(heal);
        bmcp_heal::register(&mut registry);
        let page = registry.extensions().insert(Page::default());
        registry
            .add::<FakeSnapshot>()
            .add::<FakeRead>()
            .add::<FakeFind>();
        *page.snapshot.lock().unwrap() = "- button \"Log in\" @1:e2".into();

        let registry = Arc::new(registry);
        bmcp_ai::attach(&registry);
        let backend = FakeBackend::with_tabs(&["https://shop.test/login"]);
        bmcp_heal::attach(&registry, backend.clone(), events.clone());
        Env {
            tmp,
            registry,
            jobs,
            heal,
            events,
            backend,
            page,
        }
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        let ctx = Ctx::new(Caller::Rest, self.backend.clone(), self.events.clone());
        self.registry.call(ctx, name, args).await
    }

    pub async fn ok(&self, name: &str, args: Value) -> Value {
        match self.call(name, args.clone()).await {
            Ok(v) => v,
            Err(e) => panic!("{name} {args} failed: {e:?}"),
        }
    }

    /// Save `script` and start a run over rows `[{user: a}, {user: b}, …]`, one at a time.
    pub async fn start(&self, script: &str, users: &[&str], heal: &str) -> String {
        self.ok("script.save", json!({"name": "login", "code": script}))
            .await;
        let rows: Vec<Value> = users.iter().map(|u| json!({"user": u})).collect();
        let run = self
            .ok(
                "run.start",
                json!({"script": "login", "data": {"rows": rows}, "parallel": 1,
                       "retries": 0, "heal": heal}),
            )
            .await;
        run["id"].as_str().unwrap().to_string()
    }

    pub async fn wait_status(&self, id: &str, wanted: &[&str]) -> Value {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
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
}

/// Wait for the next event on `topic` whose data matches `pred`.
pub async fn wait_event(
    rx: &mut broadcast::Receiver<Event>,
    topic: &str,
    pred: impl Fn(&Value) -> bool,
) -> Event {
    tokio::time::timeout(Duration::from_secs(20), async {
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

/// The broken fixture script (fake host DSL; line 5 is the broken step).
pub const BROKEN: &str = "\
// sign in (fake host script: only `result` and `fail` lines do something)
const url = inputs.url;
await act(\"textbox\", \"Email\", \"fill\", inputs.user);
await act(\"textbox\", \"Password\", \"fill\", need(\"password\"));
fail NOT_FOUND page.find no element matches role=button name=\"Sign in\"
result {\"user\": \"{{user}}\"}
";

/// The fix: line 5 replaced.
pub const FIX_DIFF: &str = "\
--- a/login.ts
+++ b/login.ts
@@ -4,3 +4,3 @@
 await act(\"textbox\", \"Password\", \"fill\", need(\"password\"));
-fail NOT_FOUND page.find no element matches role=button name=\"Sign in\"
+await act(\"button\", \"Log in\", \"click\");
 result {\"user\": \"{{user}}\"}
";
