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

fn ids(list: &Value) -> Vec<String> {
    list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect()
}

/// Closing archives a chat (files kept, listed as archived, also after a restart), reopening
/// brings it back and continues its conversation, deleting removes it and its files.
#[tokio::test(flavor = "multi_thread")]
async fn closed_chats_are_archived_reopened_and_deleted() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let sessions_dir = data.path().join("ai").join("sessions");
    {
        let h = Harness::open(data.path());
        h.use_fake("claude-code", fake.path()).await;
        let mut rx = h.bus.subscribe();
        h.call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "hello there" }),
        )
        .await
        .unwrap();
        until_done(&mut rx, "claude-code-1").await;
        // A running one, waiting for an approval.
        let second = h
            .call(
                "ai.session_start",
                json!({ "provider": "claude-code", "prompt": "bash" }),
            )
            .await
            .unwrap();
        assert_eq!(second["title"], "bash");
        wait_for(&mut rx, pending_permission).await;

        let closed = h
            .call("ai.session_close", json!({ "id": "claude-code-2" }))
            .await
            .unwrap();
        assert_eq!(closed, json!({ "closed": "claude-code-2" }));
        let event = wait_for(&mut rx, |e| e.topic == "ai.session_closed").await;
        assert_eq!(event.data["session"], "claude-code-2");
        let list = h.call("ai.sessions_list", json!({})).await.unwrap();
        assert_eq!(ids(&list), ["claude-code-1"]);
        let archived = h
            .call("ai.sessions_list", json!({ "archived": true }))
            .await
            .unwrap();
        assert_eq!(ids(&archived), ["claude-code-2"]);
        let s = &archived["sessions"][0];
        assert_eq!(
            (s["archived"].as_bool(), s["status"].as_str()),
            (Some(true), Some("stopped"))
        );
        // Every file stays (but the MCP config of the stopped CLI).
        assert!(sessions_dir.join("claude-code-2.jsonl").exists());
        assert!(sessions_dir.join("claude-code-2.meta.json").exists());
        assert!(!sessions_dir.join("claude-code-2.mcp.json").exists());
        // The history can be read; writing needs a reopen first.
        let transcript = h
            .call("ai.session_transcript", json!({ "id": "claude-code-2" }))
            .await
            .unwrap();
        assert!(!transcript["entries"].as_array().unwrap().is_empty());
        let err = h
            .call(
                "ai.session_send",
                json!({ "id": "claude-code-2", "text": "x" }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        assert!(err.hint.as_deref().unwrap_or_default().contains("reopen"));

        // Closing the first one too: the most recently active archived chat comes first.
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.call("ai.session_close", json!({ "id": "claude-code-1" }))
            .await
            .unwrap();
        let archived = h
            .call("ai.sessions_list", json!({ "archived": true }))
            .await
            .unwrap();
        assert_eq!(ids(&archived), ["claude-code-1", "claude-code-2"]);
    }

    // After a restart: still archived, with title and transcript.
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert!(ids(&list).is_empty());
    let archived = h
        .call("ai.sessions_list", json!({ "archived": true }))
        .await
        .unwrap();
    assert_eq!(ids(&archived).len(), 2);
    let first = archived["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "claude-code-1")
        .unwrap();
    assert_eq!(first["title"], "hello there");

    // Reopened: open again, and the next message continues the same conversation.
    let mut rx = h.bus.subscribe();
    let reopened = h
        .call("ai.session_reopen", json!({ "id": "claude-code-1" }))
        .await
        .unwrap();
    assert_eq!(
        (reopened["archived"].as_bool(), reopened["status"].as_str()),
        (Some(false), Some("stopped"))
    );
    let update = wait_for(&mut rx, |e| {
        e.topic == "ai.session" && e.data["archived"] == false
    })
    .await;
    assert_eq!(update.data["session"], "claude-code-1");
    assert_eq!(
        ids(&h.call("ai.sessions_list", json!({})).await.unwrap()),
        ["claude-code-1"]
    );
    h.call(
        "ai.session_send",
        json!({ "id": "claude-code-1", "text": "again" }),
    )
    .await
    .unwrap();
    until_done(&mut rx, "claude-code-1").await;
    let last = runs(fake.path()).pop().unwrap();
    assert!(
        argv(&last).iter().any(|a| a.starts_with("--resume=sess-")),
        "{last}"
    );

    // Deleted: gone with every file, now and after a restart.
    let deleted = h
        .call("ai.session_delete", json!({ "id": "claude-code-1" }))
        .await
        .unwrap();
    assert_eq!(deleted, json!({ "deleted": true }));
    wait_for(&mut rx, |e| e.topic == "ai.session_deleted").await;
    for ext in ["jsonl", "meta.json", "history.json", "mcp.json"] {
        assert!(
            !sessions_dir.join(format!("claude-code-1.{ext}")).exists(),
            "{ext}"
        );
    }
    let err = h
        .call("ai.session_reopen", json!({ "id": "claude-code-1" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // The older form: close with delete_transcript deletes.
    h.call(
        "ai.session_close",
        json!({ "id": "claude-code-2", "delete_transcript": true }),
    )
    .await
    .unwrap();
    assert!(!sessions_dir.join("claude-code-2.jsonl").exists());
    assert!(!sessions_dir.join("claude-code-2.meta.json").exists());
    drop(h);
    let h = Harness::open(data.path());
    assert!(
        ids(&h
            .call("ai.sessions_list", json!({ "archived": true }))
            .await
            .unwrap())
        .is_empty()
    );
    assert!(ids(&h.call("ai.sessions_list", json!({})).await.unwrap()).is_empty());
}

/// The browser's agents may rename or delete their own chat, never another agent's.
#[tokio::test(flavor = "multi_thread")]
async fn agents_cannot_rename_or_delete_other_agents_chats() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    for prompt in ["one", "two"] {
        let info = h
            .call(
                "ai.session_start",
                json!({ "provider": "claude-code", "prompt": prompt }),
            )
            .await
            .unwrap();
        until_done(&mut rx, info["id"].as_str().unwrap()).await;
    }
    let as_agent = |id: &str| Caller::Mcp {
        session: "m1".into(),
        agent: Some(id.to_string()),
    };
    for (name, args) in [
        (
            "ai.session_rename",
            json!({ "id": "claude-code-2", "title": "mine" }),
        ),
        ("ai.session_delete", json!({ "id": "claude-code-2" })),
        (
            "ai.session_close",
            json!({ "id": "claude-code-2", "delete_transcript": true }),
        ),
        ("ai.session_close", json!({ "id": "claude-code-2" })),
        (
            "ai.session_edit",
            json!({ "id": "claude-code-2", "index": 0, "text": "rewritten" }),
        ),
    ] {
        let err = h
            .call_as(as_agent("claude-code-1"), name, args.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NeedsApproval, "{name}");
        let err = h
            .call_as(
                Caller::Agent {
                    id: "anthropic-9".into(),
                },
                name,
                args,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NeedsApproval, "{name}");
    }
    // None of it happened: the other chat is open, its message unchanged.
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert_eq!(list["sessions"].as_array().unwrap().len(), 2);
    let transcript = h
        .call("ai.session_transcript", json!({ "id": "claude-code-2" }))
        .await
        .unwrap();
    assert_eq!(transcript["entries"][0]["text"], "two");
    // Its own chat: yes. The person: any chat.
    let renamed = h
        .call_as(
            as_agent("claude-code-1"),
            "ai.session_rename",
            json!({ "id": "claude-code-1", "title": "Agent's own" }),
        )
        .await
        .unwrap();
    assert_eq!(
        (renamed["title"].as_str(), renamed["title_set"].as_bool()),
        (Some("Agent's own"), Some(true))
    );
    let renamed = h
        .call(
            "ai.session_rename",
            json!({ "id": "claude-code-2", "title": "  Shopping   list " }),
        )
        .await
        .unwrap();
    assert_eq!(renamed["title"], "Shopping list");
    let back = h
        .call(
            "ai.session_rename",
            json!({ "id": "claude-code-2", "title": "" }),
        )
        .await
        .unwrap();
    assert_eq!(
        (back["title"].as_str(), back["title_set"].as_bool()),
        (Some("two"), Some(false))
    );
    h.call("ai.session_delete", json!({ "id": "claude-code-2" }))
        .await
        .unwrap();
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

/// Texts of the `ai.message` events of `role` with a whole `text`.
fn texts(events: &[Event], role: &str) -> Vec<String> {
    of(events, "ai.message")
        .into_iter()
        .filter(|e| e.data["role"] == role)
        .filter_map(|e| e.data["text"].as_str().map(str::to_string))
        .collect()
}

/// Wait until `session` is idle with nothing queued.
async fn until_idle(h: &Harness, session: &str) {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let list = h.call("ai.sessions_list", json!({})).await.unwrap();
            let s = list["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["id"] == session)
                .cloned()
                .unwrap();
            if s["status"] == "idle" && s["queued"] == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the chat did not become idle");
}

/// A message sent while Claude Code works goes to it at once (with a uuid), is queued until
/// claude reports it took it (`command_lifecycle started`), and lands in the running turn.
#[tokio::test(flavor = "multi_thread")]
async fn claude_code_takes_messages_sent_while_it_works() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "slow task" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    wait_for(&mut rx, |e| {
        e.topic == "ai.message" && e.data["text"] == "working on it"
    })
    .await;

    let sent = h
        .call("ai.session_send", json!({ "id": id, "text": "also this" }))
        .await
        .unwrap();
    assert_eq!(sent["status"], "running");
    let events = until_done(&mut rx, &id).await;
    let user = of(&events, "ai.message")
        .into_iter()
        .find(|e| e.data["role"] == "user")
        .unwrap();
    assert_eq!(
        (user.data["queued"].as_bool(), user.data["index"].as_u64()),
        (Some(true), Some(1))
    );
    let taken = of(&events, "ai.message_taken");
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].data["index"], 1);
    assert!(
        of(&events, "ai.session")
            .iter()
            .any(|e| e.data["queued"] == 1),
        "an update counts the queued message"
    );
    assert_eq!(
        texts(&events, "assistant").last().unwrap(),
        "echo: slow task + also this"
    );
    assert_eq!(events.last().unwrap().data["reason"], "end_turn");
    until_idle(&h, &id).await;
    // One process; the message carried our uuid.
    assert_eq!(runs(fake.path()).len(), 1);
    let users: Vec<Value> = jsonl(&fake.path().join("stdin.jsonl"))
        .into_iter()
        .filter(|l| l["type"] == "user")
        .collect();
    assert_eq!(users.len(), 2);
    assert!(users[1]["uuid"].as_str().is_some_and(|u| u.len() == 36));
    let transcript = h
        .call("ai.session_transcript", json!({ "id": id }))
        .await
        .unwrap();
    assert!(
        transcript["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e.get("queued").is_none()),
        "nothing is left queued"
    );

    // Two messages right after each other while idle: the second waits for the next turn,
    // the chat stays running until both are answered.
    let (a, b) = tokio::join!(
        h.call("ai.session_send", json!({ "id": id, "text": "first" })),
        h.call("ai.session_send", json!({ "id": id, "text": "second" })),
    );
    a.unwrap();
    b.unwrap();
    let mut answers = Vec::new();
    for _ in 0..2 {
        answers.extend(texts(&until_done(&mut rx, &id).await, "assistant"));
    }
    answers.sort();
    assert_eq!(answers, ["echo: first", "echo: second"]);
    until_idle(&h, &id).await;
    h.stop_all().await;
}

/// ai.session_interrupt ends the turn but not the process; ai.session_set_model reaches the
/// running process at once and the next one through --model.
#[tokio::test(flavor = "multi_thread")]
async fn claude_code_interrupts_and_switches_models_without_restarting() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "slow job" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    wait_for(&mut rx, |e| {
        e.topic == "ai.message" && e.data["text"] == "working on it"
    })
    .await;
    let interrupted = h
        .call("ai.session_interrupt", json!({ "id": id }))
        .await
        .unwrap();
    assert_eq!(interrupted["status"], "idle");
    let events = until_done(&mut rx, &id).await;
    assert_eq!(events.last().unwrap().data["reason"], "stopped");
    let control: Vec<Value> = jsonl(&fake.path().join("stdin.jsonl"))
        .into_iter()
        .filter(|l| l["type"] == "control_request")
        .collect();
    assert!(
        control
            .iter()
            .any(|c| c["request"]["subtype"] == "interrupt"),
        "{control:?}"
    );
    // Nothing to interrupt now: a no-op.
    let idle = h
        .call("ai.session_interrupt", json!({ "id": id }))
        .await
        .unwrap();
    assert_eq!(idle["status"], "idle");

    let changed = h
        .call(
            "ai.session_set_model",
            json!({ "id": id, "model": "haiku" }),
        )
        .await
        .unwrap();
    assert_eq!(changed["model"], "haiku");
    h.call("ai.session_send", json!({ "id": id, "text": "model?" }))
        .await
        .unwrap();
    let events = until_done(&mut rx, &id).await;
    assert!(texts(&events, "assistant").contains(&"model: haiku".to_string()));
    assert_eq!(runs(fake.path()).len(), 1, "the same process went on");
    let set = jsonl(&fake.path().join("stdin.jsonl"))
        .into_iter()
        .find(|l| l["request"]["subtype"] == "set_model")
        .unwrap();
    assert_eq!(set["request"]["model"], "haiku");

    // Saved: a stopped chat starts its next process with it; "default" goes back.
    h.call("ai.session_stop", json!({ "id": id }))
        .await
        .unwrap();
    h.call("ai.session_send", json!({ "id": id, "text": "hi" }))
        .await
        .unwrap();
    until_done(&mut rx, &id).await;
    let args = argv(&runs(fake.path())[1]);
    assert!(
        args.windows(2).any(|w| w == ["--model", "haiku"]),
        "{args:?}"
    );
    let reset = h
        .call(
            "ai.session_set_model",
            json!({ "id": id, "model": "default" }),
        )
        .await
        .unwrap();
    assert!(reset["model"].is_null());
    h.stop_all().await;
}

/// Editing a message: Claude Code resumes at the end of the turn before it
/// (`--resume-session-at`), the transcript is cut there and the new text follows.
#[tokio::test(flavor = "multi_thread")]
async fn editing_a_message_rewinds_claude_code() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "one" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    until_done(&mut rx, &id).await;
    for text in ["two", "three"] {
        h.call("ai.session_send", json!({ "id": id, "text": text }))
            .await
            .unwrap();
        until_done(&mut rx, &id).await;
    }
    let transcript = h
        .call("ai.session_transcript", json!({ "id": id }))
        .await
        .unwrap();
    let dones: Vec<String> = transcript["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "done")
        .map(|e| e["at"].as_str().expect("a checkpoint").to_string())
        .collect();
    assert_eq!(dones.len(), 3);

    let err = h
        .call(
            "ai.session_edit",
            json!({ "id": id, "index": 7, "text": "x" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let edited = h
        .call(
            "ai.session_edit",
            json!({ "id": id, "index": 1, "text": "TWO" }),
        )
        .await
        .unwrap();
    assert_eq!(edited["id"], id.as_str());
    let events = until_done(&mut rx, &id).await;
    let rewound = of(&events, "ai.session_rewound");
    assert_eq!(rewound[0].data["index"], 1);
    assert_eq!(texts(&events, "user"), ["TWO"]);
    assert_eq!(texts(&events, "assistant").last().unwrap(), "echo: TWO");

    // A new process resumes the same conversation at the end of turn 1.
    let all = runs(fake.path());
    assert_eq!(all.len(), 2);
    let args = argv(&all[1]);
    assert!(
        args.iter().any(|a| a.starts_with("--resume=sess-")),
        "{args:?}"
    );
    assert!(
        args.contains(&format!("--resume-session-at={}", dones[0])),
        "{args:?}"
    );
    let transcript = h
        .call("ai.session_transcript", json!({ "id": id }))
        .await
        .unwrap();
    let users: Vec<(Value, Value)> = transcript["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "user")
        .map(|e| (e["text"].clone(), e["index"].clone()))
        .collect();
    assert_eq!(users, [(json!("one"), json!(0)), (json!("TWO"), json!(1))]);
    // The file holds the same (a restart shows the edited chat).
    let file = jsonl(&data.path().join("ai/sessions").join(format!("{id}.jsonl")));
    assert_eq!(file.iter().filter(|e| e["type"] == "user").count(), 2);

    // Editing the first message: a new conversation (nothing before it to keep), new title.
    h.call(
        "ai.session_edit",
        json!({ "id": id, "index": 0, "text": "fresh start" }),
    )
    .await
    .unwrap();
    until_done(&mut rx, &id).await;
    let args = argv(&runs(fake.path())[2]);
    assert!(!args.iter().any(|a| a.contains("resume")), "{args:?}");
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert_eq!(list["sessions"][0]["title"], "fresh start");
    h.stop_all().await;
}

/// Codex: a message sent during a turn is steered into it; an older Codex without
/// turn/steer gets it as the next turn. Interrupt, the model per turn, ai.models.
#[tokio::test(flavor = "multi_thread")]
async fn codex_steers_holds_interrupts_and_lists_models() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("codex", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "codex", "prompt": "slow work" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    // The turn runs (turn/started seen by the adapter once a steer is accepted).
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.call("ai.session_send", json!({ "id": id, "text": "and this" }))
        .await
        .unwrap();
    let events = until_done(&mut rx, &id).await;
    assert_eq!(
        texts(&events, "assistant").last().unwrap(),
        "echo: slow work + and this"
    );
    assert_eq!(of(&events, "ai.message_taken")[0].data["index"], 1);
    let sent = jsonl(&fake.path().join("stdin.jsonl"));
    let steer = sent
        .iter()
        .find(|m| m["method"] == "turn/steer")
        .expect("turn/steer");
    assert_eq!(steer["params"]["expectedTurnId"], "u1");
    until_idle(&h, &id).await;

    // Interrupt: the turn ends as stopped, the thread and process stay.
    h.call("ai.session_send", json!({ "id": id, "text": "slow again" }))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.call("ai.session_interrupt", json!({ "id": id }))
        .await
        .unwrap();
    let events = until_done(&mut rx, &id).await;
    assert_eq!(events.last().unwrap().data["reason"], "stopped");
    assert!(
        jsonl(&fake.path().join("stdin.jsonl"))
            .iter()
            .any(|m| m["method"] == "turn/interrupt")
    );

    // The model goes with every turn/start from now on.
    h.call(
        "ai.session_set_model",
        json!({ "id": id, "model": "fake-small" }),
    )
    .await
    .unwrap();
    h.call("ai.session_send", json!({ "id": id, "text": "model?" }))
        .await
        .unwrap();
    let events = until_done(&mut rx, &id).await;
    assert!(texts(&events, "assistant").contains(&"model: fake-small".to_string()));
    assert_eq!(runs(fake.path()).len(), 1);
    h.stop_all().await;

    // An older Codex without turn/steer: held, then the next turn.
    std::fs::write(fake.path().join("scenario.json"), r#"{"no_steer": true}"#).unwrap();
    h.call("ai.session_send", json!({ "id": id, "text": "slow three" }))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    h.call("ai.session_send", json!({ "id": id, "text": "held one" }))
        .await
        .unwrap();
    let first = until_done(&mut rx, &id).await;
    assert_eq!(
        texts(&first, "assistant").last().unwrap(),
        "echo: slow three"
    );
    let second = until_done(&mut rx, &id).await;
    assert_eq!(
        texts(&second, "assistant").last().unwrap(),
        "echo: held one"
    );
    until_idle(&h, &id).await;

    // ai.models: Codex's model/list (no thread started), else the static list.
    let models = h
        .call("ai.models", json!({ "provider": "codex" }))
        .await
        .unwrap();
    let names: Vec<&str> = models["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["default", "fake-large", "fake-small"]);
    assert_eq!(models["default"], "fake-large");
    assert_eq!(models["models"][1]["hint"], "the big one");
    std::fs::write(
        fake.path().join("scenario.json"),
        r#"{"no_model_list": true}"#,
    )
    .unwrap();
    let models = h
        .call("ai.models", json!({ "provider": "codex" }))
        .await
        .unwrap();
    assert!(models["models"].as_array().unwrap().len() > 1);
    assert!(
        models["models"][1]["hint"]
            .as_str()
            .unwrap()
            .contains("static")
    );
    h.stop_all().await;
}

/// Editing a Codex message rolls the thread back by the turns after it.
#[tokio::test(flavor = "multi_thread")]
async fn editing_a_codex_message_rolls_the_thread_back() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("codex", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "codex", "prompt": "one" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    until_done(&mut rx, &id).await;
    for text in ["two", "three"] {
        h.call("ai.session_send", json!({ "id": id, "text": text }))
            .await
            .unwrap();
        until_done(&mut rx, &id).await;
    }
    h.call(
        "ai.session_edit",
        json!({ "id": id, "index": 1, "text": "TWO" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, &id).await;
    assert_eq!(texts(&events, "assistant").last().unwrap(), "echo: TWO");
    let sent = jsonl(&fake.path().join("stdin.jsonl"));
    let rollback = sent
        .iter()
        .find(|m| m["method"] == "thread/rollback")
        .expect("thread/rollback");
    assert_eq!(
        rollback["params"],
        json!({ "threadId": "th-1", "numTurns": 2 })
    );
    assert!(sent.iter().any(|m| m["method"] == "thread/resume"));
    // Still the same thread: no new one was started after the first.
    assert_eq!(
        sent.iter()
            .filter(|m| m["method"] == "thread/start")
            .count(),
        1
    );
    h.stop_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn claude_code_models_are_its_aliases_and_ids() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let models = h
        .call("ai.models", json!({ "provider": "claude-code" }))
        .await
        .unwrap();
    let ids: Vec<&str> = models["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    for id in [
        "default",
        "opus",
        "sonnet",
        "haiku",
        "fable",
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "claude-fable-5-1",
    ] {
        assert!(ids.contains(&id), "{id} in {ids:?}");
    }
    assert!(models["default"].is_string());
    let err = h
        .call("ai.models", json!({ "provider": "nope" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

/// Codex keeps the model a turn named: back to Default, the next turn names Codex's default
/// model (model/list `isDefault`), so the switch takes effect.
#[tokio::test(flavor = "multi_thread")]
async fn codex_goes_back_to_its_default_model() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("codex", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "codex", "prompt": "model?" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    let events = until_done(&mut rx, &id).await;
    assert!(texts(&events, "assistant").contains(&"model: default".to_string()));
    let ask = async |rx: &mut broadcast::Receiver<Event>, model: &str| {
        h.call("ai.session_set_model", json!({ "id": id, "model": model }))
            .await
            .unwrap();
        h.call("ai.session_send", json!({ "id": id, "text": "model?" }))
            .await
            .unwrap();
        texts(&until_done(rx, &id).await, "assistant")
    };
    assert!(
        ask(&mut rx, "fake-small")
            .await
            .contains(&"model: fake-small".to_string())
    );
    let back = ask(&mut rx, "default").await;
    assert!(back.contains(&"model: fake-large".to_string()), "{back:?}");
    // One process the whole time; nothing was named before an override was.
    assert_eq!(runs(fake.path()).len(), 1);
    let starts: Vec<Value> = jsonl(&fake.path().join("stdin.jsonl"))
        .into_iter()
        .filter(|m| m["method"] == "turn/start")
        .map(|m| m["params"]["model"].clone())
        .collect();
    assert_eq!(
        starts,
        [Value::Null, json!("fake-small"), json!("fake-large")]
    );
    h.stop_all().await;
}

/// A message held while Codex crashed mid-turn stays queued while the chat has failed; the
/// next message continues the chat and takes it along (both texts in the first turn of the
/// new run, `ai.message_taken` for the held one).
#[tokio::test(flavor = "multi_thread")]
async fn messages_queued_when_codex_crashed_go_with_the_next_one() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("codex", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "codex", "prompt": "crash now" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let sent = h
        .call("ai.session_send", json!({ "id": id, "text": "held one" }))
        .await
        .unwrap();
    assert_eq!(sent["queued"], 1);
    let events = until_done(&mut rx, &id).await;
    assert!(
        events.last().unwrap().data["reason"]
            .as_str()
            .unwrap()
            .starts_with("error:")
    );
    wait_for(&mut rx, |e| {
        e.topic == "ai.session" && e.data["session"] == id.as_str() && e.data["status"] == "failed"
    })
    .await;
    let list = h.call("ai.sessions_list", json!({})).await.unwrap();
    assert_eq!(
        (
            list["sessions"][0]["status"].as_str(),
            list["sessions"][0]["queued"].as_u64()
        ),
        (Some("failed"), Some(1))
    );

    h.call("ai.session_send", json!({ "id": id, "text": "next" }))
        .await
        .unwrap();
    let events = until_done(&mut rx, &id).await;
    let taken: Vec<&Value> = of(&events, "ai.message_taken")
        .iter()
        .map(|e| &e.data["index"])
        .collect();
    assert_eq!(taken, [&json!(1)]);
    assert_eq!(
        texts(&events, "assistant").last().unwrap(),
        "echo: held one\n\nnext"
    );
    until_idle(&h, &id).await;
    assert_eq!(runs(fake.path()).len(), 2);
    let transcript = h
        .call("ai.session_transcript", json!({ "id": id }))
        .await
        .unwrap();
    assert!(
        transcript["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e.get("queued").is_none()),
        "nothing is left queued"
    );
    h.stop_all().await;
}

/// An edit that cannot rewind Claude Code's conversation (a chat from before checkpoints)
/// says so AFTER `ai.session_rewound`: a window applying the cut keeps the notice.
#[tokio::test(flavor = "multi_thread")]
async fn the_lost_conversation_notice_follows_the_rewind() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let sessions = data.path().join("ai").join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(
        sessions.join("claude-code-1.meta.json"),
        r##"{"id":"claude-code-1","provider":"claude-code","model":null,"color":"#3b82f6",
            "created_ms":5,"launch":{"toolsets":["core"],"max_steps":30,"access":null},
            "conversation":"sess-old","title":"one","title_set":false,"archived":false,
            "updated_ms":60}"##,
    )
    .unwrap();
    std::fs::write(
        sessions.join("claude-code-1.jsonl"),
        "{\"type\":\"user\",\"text\":\"one\",\"index\":0,\"ts_ms\":10}\n\
         {\"type\":\"assistant\",\"text\":\"a1\",\"ts_ms\":20}\n\
         {\"type\":\"done\",\"reason\":\"end_turn\",\"ts_ms\":30}\n\
         {\"type\":\"user\",\"text\":\"two\",\"index\":1,\"ts_ms\":40}\n\
         {\"type\":\"assistant\",\"text\":\"a2\",\"ts_ms\":50}\n\
         {\"type\":\"done\",\"reason\":\"end_turn\",\"ts_ms\":60}\n",
    )
    .unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    h.call(
        "ai.session_edit",
        json!({ "id": "claude-code-1", "index": 1, "text": "TWO" }),
    )
    .await
    .unwrap();
    let events = until_done(&mut rx, "claude-code-1").await;
    let rewound = events
        .iter()
        .position(|e| e.topic == "ai.session_rewound")
        .expect("ai.session_rewound");
    let notice = events
        .iter()
        .position(|e| e.topic == "ai.message" && e.data["code"] == "resume_lost")
        .expect("the resume_lost notice");
    assert!(rewound < notice, "rewound at {rewound}, notice at {notice}");
    h.stop_all().await;
}

/// A close racing a delete never brings the deleted chat back (its meta file stays gone).
#[tokio::test(flavor = "multi_thread")]
async fn a_close_racing_a_delete_does_not_restore_the_chat() {
    let data = tempfile::tempdir().unwrap();
    let fake = tempfile::tempdir().unwrap();
    let h = Harness::open(data.path());
    h.use_fake("claude-code", fake.path()).await;
    let mut rx = h.bus.subscribe();
    let info = h
        .call(
            "ai.session_start",
            json!({ "provider": "claude-code", "prompt": "one" }),
        )
        .await
        .unwrap();
    let id = info["id"].as_str().unwrap().to_string();
    until_done(&mut rx, &id).await;
    let (deleted, closed) = tokio::join!(
        h.call("ai.session_delete", json!({ "id": id })),
        h.call("ai.session_close", json!({ "id": id })),
    );
    deleted.unwrap();
    if let Err(err) = closed {
        assert_eq!(err.code, ErrorCode::NotFound);
    }
    let meta = data
        .path()
        .join("ai/sessions")
        .join(format!("{id}.meta.json"));
    assert!(!meta.exists(), "the deleted chat's meta file came back");
    drop(h);
    let h = Harness::open(data.path());
    for archived in [false, true] {
        let list = h
            .call("ai.sessions_list", json!({ "archived": archived }))
            .await
            .unwrap();
        assert_eq!(list["sessions"], json!([]));
    }
}
