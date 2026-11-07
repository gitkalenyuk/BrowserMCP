//! Claude Code and Codex sessions end to end against fake CLIs (`tests/fake/*.mjs`, run with
//! node through a `.cmd` shim like npm installs them): approval requests and answers,
//! permission modes and folders, continuing stopped chats (also after a browser restart),
//! closing chats. No real CLI, no model calls.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bmcp_ai::AiService;
use bmcp_core::events::Event;
use bmcp_core::testing::FakeBackend;
use bmcp_core::{Caller, CmdError, Ctx, ErrorCode, EventBus, Registry};
use serde_json::{Value, json};
use tokio::sync::broadcast;

const TIMEOUT: Duration = Duration::from_secs(60);

struct Harness {
    registry: Arc<Registry>,
    fake: Arc<FakeBackend>,
    bus: EventBus,
    data: PathBuf,
}

impl Harness {
    /// A browser whose data folder is `data` (sessions saved there are restored).
    fn open(data: &Path) -> Self {
        let mut registry = Registry::new();
        bmcp_core::commands::register_builtin(&mut registry);
        registry
            .extensions()
            .insert(AiService::new(data, "http://127.0.0.1:9/mcp"));
        bmcp_ai::register(&mut registry);
        let registry = Arc::new(registry);
        bmcp_ai::attach(&registry);
        Harness {
            registry,
            fake: FakeBackend::with_tabs(&["https://a.test/"]),
            bus: EventBus::new(),
            data: data.to_path_buf(),
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.call_as(Caller::Ui, name, args).await
    }

    async fn call_as(&self, caller: Caller, name: &str, args: Value) -> Result<Value, CmdError> {
        let ctx = Ctx::new(caller, self.fake.clone(), self.bus.clone());
        self.registry.call(ctx, name, args).await
    }

    /// Point `provider` at a fake CLI living in `fake_dir`.
    async fn use_fake(&self, provider: &str, fake_dir: &Path) {
        let script = if provider == "codex" {
            "codex"
        } else {
            "claude"
        };
        let shim = fake_shim(fake_dir, script);
        self.call(
            "ai.provider_set",
            json!({ "id": provider, "path": shim.to_string_lossy() }),
        )
        .await
        .unwrap();
    }

    async fn stop_all(&self) {
        let list = self.call("ai.sessions_list", json!({})).await.unwrap();
        for s in list["sessions"].as_array().unwrap() {
            self.call("ai.session_stop", json!({ "id": s["id"] }))
                .await
                .unwrap();
        }
    }
}

/// `<dir>/<name>.cmd` that runs `tests/fake/<name>.mjs` with FAKE_DIR=<dir>.
fn fake_shim(dir: &Path, name: &str) -> PathBuf {
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fake")
        .join(format!("{name}.mjs"));
    let shim = dir.join(format!("{name}.cmd"));
    let body = format!(
        "@echo off\r\nset \"FAKE_DIR=%~dp0\"\r\nnode \"{}\" %*\r\n",
        script.display()
    );
    std::fs::write(&shim, body).unwrap();
    shim
}

fn jsonl(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// `{argv, cwd}` of every run of the fake CLI.
fn runs(fake_dir: &Path) -> Vec<Value> {
    jsonl(&fake_dir.join("runs.jsonl"))
}

fn argv(run: &Value) -> Vec<String> {
    run["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

fn same_dir(a: &str, b: &Path) -> bool {
    std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap()
}

/// The next event matching `pred`.
async fn wait_for(rx: &mut broadcast::Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Event {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let event = rx.recv().await.unwrap();
            if pred(&event) {
                return event;
            }
        }
    })
    .await
    .expect("event did not arrive in time")
}

/// Events until (and including) the next `ai.done` of `session`.
async fn until_done(rx: &mut broadcast::Receiver<Event>, session: &str) -> Vec<Event> {
    let mut events = Vec::new();
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let event = rx.recv().await.unwrap();
            let done = event.topic == "ai.done" && event.data["session"] == session;
            events.push(event);
            if done {
                return;
            }
        }
    })
    .await
    .expect("no ai.done in time");
    events
}

fn pending_permission(e: &Event) -> bool {
    e.topic == "ai.permission" && e.data["status"] == "pending"
}

fn of<'a>(events: &'a [Event], topic: &str) -> Vec<&'a Event> {
    events.iter().filter(|e| e.topic == topic).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_code_asks_before_tools_and_relays_the_answers() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();

    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "please run bash" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    assert_eq!(info["access"]["permission_mode"], "ask");

    // The tool call waits for a person: an event, and listable (a reloaded UI finds it).
    let pending = wait_for(&mut rx, pending_permission).await;
    assert_eq!(pending.data["session"], id.as_str());
    assert_eq!(pending.data["tool"], "Bash");
    assert_eq!(pending.data["kind"], "tool");
    assert_eq!(pending.data["input"]["command"], "echo hi");
    assert_eq!(pending.data["title"], "Claude wants to run echo hi");
    let pid = pending.data["id"].as_str().unwrap().to_string();
    let list = h.call("ai.permissions_list", json!({})).await.unwrap();
    assert_eq!(list["permissions"][0]["id"], pid.as_str());
    let listed = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert_eq!(listed["sessions"][0]["pending_permissions"], 1);

    // No agent of the browser may approve it: not the agent itself (over its MCP connection),
    // not an API agent of another chat.
    for caller in [
        Caller::Mcp {
            session: "m1".into(),
            agent: Some(id.clone()),
        },
        Caller::Agent {
            id: "anthropic-7".into(),
        },
    ] {
        let err = h
            .call_as(
                caller,
                "ai.permission_answer",
                json!({ "id": pid, "decision": "allow" }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NeedsApproval);
    }

    // Deny with a reason: the agent gets it, and the chat shows it on the failed tool call.
    h.call(
        "ai.permission_answer",
        json!({ "id": pid, "decision": "deny", "reason": "not on this machine" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, &id).await;
    let denied = of(&events, "ai.permission");
    assert_eq!(denied[0].data["status"], "denied");
    assert_eq!(denied[0].data["answer"], "not on this machine");
    let result = of(&events, "ai.tool_result")[0];
    assert_eq!(result.data["name"], "Bash");
    assert_eq!(result.data["ok"], false);
    assert!(
        result.data["summary"]
            .as_str()
            .unwrap()
            .contains("not on this machine"),
        "{}",
        result.data
    );
    let answers: Vec<Value> = jsonl(&fake.path().join("stdin.jsonl"))
        .into_iter()
        .filter(|l| l["type"] == "control_response")
        .collect();
    assert_eq!(answers[0]["response"]["response"]["behavior"], "deny");
    assert!(
        h.call("ai.permissions_list", json!({})).await.unwrap()["permissions"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // "Ask me" = Claude Code's default mode with prompts routed to us; browser tools always
    // allowed; the default working folder.
    let run = &runs(fake.path())[0];
    let args = argv(run);
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-prompt-tool", "stdio"]),
        "{args:?}"
    );
    assert!(
        args.windows(2)
            .any(|w| w == ["--allowedTools", "mcp__browsermcp"]),
        "{args:?}"
    );
    assert!(!args.contains(&"--permission-mode".to_string()), "{args:?}");
    assert!(same_dir(
        run["cwd"].as_str().unwrap(),
        &h.data.join("ai").join("workspace")
    ));
    let stdin = jsonl(&fake.path().join("stdin.jsonl"));
    assert_eq!(
        stdin[0]["request"]["subtype"], "initialize",
        "initialize comes first"
    );

    // "Always allow": runs now, and the next Bash call does not ask again.
    h.call("ai.session_send", json!({ "id": id, "text": "bash again" }))
        .await
        .unwrap();
    let pending = wait_for(&mut rx, pending_permission).await;
    h.call(
        "ai.permission_answer",
        json!({ "id": pending.data["id"], "decision": "allow_always" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, &id).await;
    let result = of(&events, "ai.tool_result")[0];
    assert_eq!(
        (result.data["ok"].as_bool(), result.data["summary"].as_str()),
        (Some(true), Some("ran echo hi"))
    );
    h.call(
        "ai.session_send",
        json!({ "id": id, "text": "bash once more" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, &id).await;
    assert!(
        of(&events, "ai.permission").is_empty(),
        "must not ask again"
    );
    assert_eq!(of(&events, "ai.tool_result")[0].data["ok"], true);
    let allow: Vec<Value> = jsonl(&fake.path().join("stdin.jsonl"))
        .into_iter()
        .filter(|l| l["type"] == "control_response")
        .collect();
    assert_eq!(
        allow[1]["response"]["response"]["updatedPermissions"][0]["rules"][0]["toolName"],
        "Bash"
    );
    assert_eq!(allow[2]["response"]["response"]["behavior"], "allow");

    // The transcript keeps the answered requests.
    let transcript = h
        .call("ai.session_transcript", json!({ "id": id }))
        .await
        .unwrap();
    let statuses: Vec<&str> = transcript["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "permission")
        .map(|e| e["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["denied", "allowed"]);
    h.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_cancels_pending_approvals() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "bash" }),
        )
        .await
        .unwrap();
    let pending = wait_for(&mut rx, pending_permission).await;
    let stopped = h
        .call("ai.session_stop", json!({ "id": info["id"] }))
        .await
        .unwrap();
    assert_eq!(stopped["status"], "stopped");
    let cancelled = wait_for(&mut rx, |e| {
        e.topic == "ai.permission" && e.data["status"] == "cancelled"
    })
    .await;
    assert_eq!(cancelled.data["id"], pending.data["id"]);
    let list = h.call("ai.permissions_list", json!({})).await.unwrap();
    assert!(list["permissions"].as_array().unwrap().is_empty());
    let err = h
        .call(
            "ai.permission_answer",
            json!({ "id": pending.data["id"], "decision": "allow" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

/// Claude Code withdraws a request right after asking (`control_cancel_request` read in the
/// same batch): the card goes away and the CLI gets its answer, instead of a card that waits
/// until the chat stops.
#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawn_request_is_cancelled_at_once() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "withdraw" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    let events = until_done(&mut rx, &id).await;
    let statuses: Vec<&str> = of(&events, "ai.permission")
        .into_iter()
        .filter_map(|e| e.data["status"].as_str())
        .collect();
    assert_eq!(statuses, ["pending", "cancelled"]);
    let list = h.call("ai.permissions_list", json!({})).await.unwrap();
    assert!(list["permissions"].as_array().unwrap().is_empty());
    h.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_chat_continues_its_conversation_also_after_a_restart() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let id = {
        let h = Harness::open(data.path());
        h.use_fake("claude-code", fake.path()).await;
        let mut rx = h.bus.subscribe();
        let info = h
            .call(
                "ai.session_start",
                json!({ "provider": "claude-code", "prompt": "hello" }),
            )
            .await
            .unwrap();
        let id = info["id"].as_str().unwrap().to_string();
        until_done(&mut rx, &id).await;
        h.call("ai.session_stop", json!({ "id": id }))
            .await
            .unwrap();

        // The next message continues the SAME session, resuming Claude Code's conversation.
        let again = h
            .call("ai.session_send", json!({ "id": id, "text": "again" }))
            .await
            .unwrap();
        assert_eq!(again["id"], id.as_str());
        let events = until_done(&mut rx, &id).await;
        let text = of(&events, "ai.message")
            .into_iter()
            .rfind(|e| e.data["text"].is_string())
            .unwrap();
        assert_eq!(text.data["text"], "echo: again");
        let runs = runs(fake.path());
        assert_eq!(runs.len(), 2);
        let resume = argv(&runs[1])
            .into_iter()
            .find(|a| a.starts_with("--resume=sess-"))
            .expect("the second run resumes");
        assert!(!argv(&runs[0]).iter().any(|a| a.starts_with("--resume")));
        let list = h.call("ai.sessions_list", json!({})).await.unwrap();
        assert_eq!(list["sessions"].as_array().unwrap().len(), 1);
        h.stop_all().await;
        assert!(resume.len() > "--resume=sess-".len());
        id
    };

    // A new browser on the same data folder lists the chat (stopped) with its transcript...
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    let sessions = list["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        (sessions[0]["id"].as_str(), sessions[0]["status"].as_str()),
        (Some(id.as_str()), Some("stopped"))
    );
    let transcript = h
        .call("ai.session_transcript", json!({ "id": id }))
        .await
        .unwrap();
    assert!(transcript["entries"].as_array().unwrap().len() >= 4);

    // ...and continues the same Claude Code conversation.
    let mut rx = h.bus.subscribe();
    h.call(
        "ai.session_send",
        json!({ "id": id, "text": "after restart" }),
    )
    .await
    .unwrap();
    until_done(&mut rx, &id).await;
    let all = runs(fake.path());
    assert_eq!(
        argv(&all[2]).last(),
        argv(&all[1]).last(),
        "same --resume id"
    );

    // New sessions are numbered after the old ones (their files never mix).
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "new" }),
        )
        .await
        .unwrap();
    assert_eq!(info["id"], "claude-code-2");
    h.stop_all().await;
}

/// Messages sent while a chat is being stopped (the panel and an MCP client at once) continue
/// it with ONE new Claude Code process: none is lost to a second backend, and the chat ends
/// idle, not stopped.
#[tokio::test(flavor = "multi_thread")]
async fn sends_racing_a_stop_continue_the_chat_once() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "hello" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    until_done(&mut rx, &id).await;

    // The stop is still killing the first process when both messages arrive.
    let (stopped, a, b) = tokio::join!(
        h.call("ai.session_stop", json!({ "id": id })),
        h.call("ai.session_send", json!({ "id": id, "text": "one" })),
        h.call("ai.session_send", json!({ "id": id, "text": "two" })),
    );
    stopped.unwrap();
    a.unwrap();
    b.unwrap();
    let mut answers = Vec::new();
    for _ in 0..2 {
        let events = until_done(&mut rx, &id).await;
        answers.extend(
            of(&events, "ai.message")
                .into_iter()
                .filter_map(|e| e.data["text"].as_str().map(str::to_string))
                .filter(|t| t.starts_with("echo: ")),
        );
    }
    answers.sort();
    assert_eq!(answers, ["echo: one", "echo: two"]);
    assert_eq!(runs(fake.path()).len(), 2, "one restart, not two");
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert_eq!(list["sessions"][0]["status"], "idle");
    h.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_conversation_goes_on_in_a_new_one_and_says_so() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "hello" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    until_done(&mut rx, &id).await;
    h.call("ai.session_stop", json!({ "id": id }))
        .await
        .unwrap();

    std::fs::write(
        fake.path().join("scenario.json"),
        r#"{"reject_resume": true}"#,
    )
    .unwrap();
    h.call("ai.session_send", json!({ "id": id, "text": "again" }))
        .await
        .unwrap();
    let events = until_done(&mut rx, &id).await;
    let notices: Vec<&Event> = of(&events, "ai.message")
        .into_iter()
        .filter(|e| e.data["role"] == "system")
        .collect();
    // A code the UI translates, and English text for other clients.
    assert!(
        notices.iter().any(|n| n.data["code"] == "resume_lost"
            && n.data["params"]["agent"] == "Claude Code"
            && n.data["text"]
                .as_str()
                .is_some_and(|t| t.contains("could not continue"))),
        "{notices:?}"
    );
    let last = of(&events, "ai.message")
        .into_iter()
        .rfind(|e| e.data["text"].is_string())
        .unwrap();
    assert_eq!(
        last.data["text"], "echo: again",
        "the message was sent again"
    );
    let runs = runs(fake.path());
    assert_eq!(runs.len(), 3);
    assert!(!argv(&runs[2]).iter().any(|a| a.starts_with("--resume")));
    h.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_removes_chats_and_keeps_transcripts_unless_asked() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let sessions_dir = data.path().join("ai").join("sessions");
    {
        let h = Harness::open(data.path());
        h.use_fake("claude-code", fake.path()).await;
        let mut rx = h.bus.subscribe();
        let first = h
            .call(
                "ai.session_start",
                json!({ "provider": "claude-code", "prompt": "hello" }),
            )
            .await
            .unwrap();
        until_done(&mut rx, "claude-code-1").await;
        // A running one, waiting for an approval.
        h.call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "bash" }),
        )
        .await
        .unwrap();
        wait_for(&mut rx, pending_permission).await;

        h.call("ai.session_close", json!({ "id": "claude-code-2" }))
            .await
            .unwrap();
        let closed = wait_for(&mut rx, |e| e.topic == "ai.session_closed").await;
        assert_eq!(closed.data["session"], "claude-code-2");
        let list = h.call("ai.sessions_list", json!({})).await.unwrap();
        let ids: Vec<&str> = list["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [first["id"].as_str().unwrap()]);
        assert!(sessions_dir.join("claude-code-2.jsonl").exists());
        assert!(!sessions_dir.join("claude-code-2.meta.json").exists());
        assert!(!sessions_dir.join("claude-code-2.mcp.json").exists());

        h.call(
            "ai.session_close",
            json!({ "id": "claude-code-1", "delete_transcript": true }),
        )
        .await
        .unwrap();
        assert!(!sessions_dir.join("claude-code-1.jsonl").exists());
        let err = h
            .call("ai.session_close", json!({ "id": "claude-code-1" }))
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }
    // Closed chats stay closed after a restart; numbering goes on after the kept transcript.
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert!(list["sessions"].as_array().unwrap().is_empty());
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "new" }),
        )
        .await
        .unwrap();
    assert_eq!(info["id"], "claude-code-3");
    h.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn permission_modes_and_folders_reach_claude_code() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let extra = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;

    let access = h
        .call("ai.agent_access", json!({ "id": "claude-code" }))
        .await
        .unwrap();
    assert_eq!(access["permission_mode"], "ask");
    assert!(access["cwd"].is_null());
    assert!(
        access["default_cwd"]
            .as_str()
            .unwrap()
            .ends_with("workspace")
    );

    let missing = work.path().join("missing").to_string_lossy().into_owned();
    let err = h
        .call(
            "ai.agent_access_set",
            json!({ "id": "claude-code", "cwd": missing }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let set = h
        .call(
            "ai.agent_access_set",
            json!({ "id": "claude-code", "permission_mode": "full_access",
                    "cwd": work.path().to_string_lossy(), "add_dirs": [extra.path().to_string_lossy()] }),
        )
        .await
        .unwrap();
    assert_eq!(set["permission_mode"], "full_access");
    // Saved.
    let saved = std::fs::read_to_string(data.path().join("config").join("ai.json")).unwrap();
    assert!(saved.contains("full_access"), "{saved}");

    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "hello" }),
        )
        .await
        .unwrap();
    until_done(&mut rx, info["id"].as_str().unwrap()).await;
    let run = &runs(fake.path())[0];
    let args = argv(run);
    assert!(same_dir(run["cwd"].as_str().unwrap(), work.path()));
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-mode", "bypassPermissions"]),
        "{args:?}"
    );
    assert!(args.contains(&"--allow-dangerously-skip-permissions".to_string()));
    let add = args.iter().position(|a| a == "--add-dir").unwrap();
    assert!(same_dir(&args[add + 1], extra.path()));

    // A chat may override the defaults; a bad folder starts nothing.
    let err = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "x", "cwd": missing }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 1);

    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "bash", "permission_mode": "browser_only" }),
        )
        .await
        .unwrap();
    assert_eq!(info["access"]["permission_mode"], "browser_only");
    let events = until_done(&mut rx, info["id"].as_str().unwrap()).await;
    assert!(
        of(&events, "ai.permission").is_empty(),
        "browser-only never asks"
    );
    let result = of(&events, "ai.tool_result")[0];
    assert_eq!(result.data["ok"], false);
    assert!(
        result.data["summary"]
            .as_str()
            .unwrap()
            .contains("permission")
    );
    let args = argv(&runs(fake.path())[1]);
    assert!(
        args.windows(2)
            .any(|w| w == ["--permission-mode", "dontAsk"])
    );
    assert!(!args.contains(&"--permission-prompt-tool".to_string()));
    h.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_approvals_use_the_same_cards_and_threads_resume() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("codex", fake.path()).await;
    h.call(
        "ai.agent_access_set",
        json!({ "id": "codex", "sandbox": "read_only", "approval_policy": "untrusted" }),
    )
    .await
    .unwrap();
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "codex", "prompt": "run the cleanup" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    assert_eq!(info["access"]["sandbox"], "read_only");

    let pending = wait_for(&mut rx, pending_permission).await;
    assert_eq!(pending.data["kind"], "command");
    assert_eq!(pending.data["tool"], "shell");
    assert_eq!(pending.data["input"]["command"], "rm -rf build");
    assert_eq!(pending.data["reason"], "clean the build folder");
    h.call(
        "ai.permission_answer",
        json!({ "id": pending.data["id"], "decision": "allow_always" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, &id).await;
    let result = of(&events, "ai.tool_result")[0];
    assert_eq!(
        (result.data["ok"].as_bool(), result.data["summary"].as_str()),
        (Some(true), Some("removed"))
    );
    let sent = jsonl(&fake.path().join("stdin.jsonl"));
    let start = sent.iter().find(|m| m["method"] == "thread/start").unwrap();
    assert_eq!(start["params"]["sandbox"], "read-only");
    assert_eq!(start["params"]["approvalPolicy"], "untrusted");
    let answer = sent
        .iter()
        .find(|m| m["method"].is_null() && m["result"]["decision"].is_string())
        .unwrap();
    assert_eq!(answer["result"]["decision"], "acceptForSession");

    // Stopped, then continued: the same Codex thread; "always" still holds in this chat.
    h.call("ai.session_stop", json!({ "id": id }))
        .await
        .unwrap();
    h.call(
        "ai.session_send",
        json!({ "id": id, "text": "run it again" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, &id).await;
    assert!(of(&events, "ai.permission").is_empty());
    let sent = jsonl(&fake.path().join("stdin.jsonl"));
    let resume = sent
        .iter()
        .find(|m| m["method"] == "thread/resume")
        .expect("continues with thread/resume");
    assert_eq!(resume["params"]["threadId"], "th-1");
    assert_eq!(runs(fake.path()).len(), 2);

    // A denied command says why in the chat.
    h.call(
        "ai.session_start",
        json!({ "provider": "codex", "prompt": "run something" }),
    )
    .await
    .unwrap();
    let pending = wait_for(&mut rx, pending_permission).await;
    let other = pending.data["session"].as_str().unwrap().to_string();
    h.call(
        "ai.permission_answer",
        json!({ "id": pending.data["id"], "decision": "deny" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, &other).await;
    let result = of(&events, "ai.tool_result")[0];
    assert_eq!(result.data["ok"], false);
    assert!(
        result.data["summary"]
            .as_str()
            .unwrap()
            .contains("not approved")
    );
    h.stop_all().await;
}
