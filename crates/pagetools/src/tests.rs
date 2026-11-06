//! Tests against `FakeBackend`: the dialog policy table, dialog events + guard, upload paths
//! and CDP sequences, PDF/MHTML files and find in page.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::command::Empty;
use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{
    BackendError, Caller, CmdError, Command, Ctx, DataDir, ErrorCode, Event, Meta, Registry, TabId,
    Toolset,
};
use serde_json::{Value, json};

use crate::dialog::{Decision, DialogType, Policy, Rule, decide, is_guarded};

const TAB: TabId = TabId(1);
const LOADER: &str = "L1";

fn mcp() -> Caller {
    Caller::Mcp {
        session: "s1".into(),
        agent: None,
    }
}

/// A `page.*` test command that takes 10 s (paused clock: no real time).
struct Probe;

impl Command for Probe {
    const NAME: &'static str = "page.probe";
    type Args = Empty;
    type Output = Empty;

    fn meta() -> Meta {
        Meta::new("Test only.", Toolset::Hidden)
    }

    async fn run(_ctx: Ctx, _args: Empty) -> Result<Empty, CmdError> {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(Empty {})
    }
}

/// Scripted CDP answers: `(method, Ok(result) | Err(message))`, newest wins.
type Answers = Arc<Mutex<Vec<(String, Result<Value, String>)>>>;

struct Env {
    fake: Arc<FakeBackend>,
    registry: Arc<Registry>,
    tmp: tempfile::TempDir,
    /// Scripted answers by CDP method (default `{}`); errors with `Err`.
    answers: Answers,
}

impl Env {
    fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let fake = FakeBackend::with_tabs(&["https://tools.test/"]);
        let answers: Answers = Arc::default();
        let scripted = answers.clone();
        fake.on_cdp(move |_tab, method, _params| {
            let scripted = scripted.lock().unwrap();
            let answer = scripted
                .iter()
                .rev()
                .find(|(m, _)| m == method)
                .map(|(_, a)| a.clone());
            Some(match answer {
                Some(Ok(value)) => Ok(value),
                Some(Err(message)) => Err(BackendError::Cdp(message)),
                None if method == "Page.getFrameTree" => {
                    Ok(json!({ "frameTree": { "frame": { "id": "F1", "loaderId": LOADER } } }))
                }
                None => Ok(json!({})),
            })
        });
        let mut registry = Registry::new();
        bmcp_core::commands::register_builtin(&mut registry);
        crate::register(&mut registry);
        registry.add::<Probe>();
        registry.extensions().insert(DataDir::from_root(tmp.path()));
        Env {
            fake,
            registry: Arc::new(registry),
            tmp,
            answers,
        }
    }

    fn answer(&self, method: &str, answer: Result<Value, &str>) {
        self.answers
            .lock()
            .unwrap()
            .push((method.to_string(), answer.map_err(str::to_string)));
    }

    async fn call_as(&self, caller: Caller, name: &str, args: Value) -> Result<Value, CmdError> {
        let ctx = ctx(caller, self.fake.clone());
        self.registry.call(ctx, name, args).await
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.call_as(mcp(), name, args).await
    }

    /// The CDP calls of one method, in order.
    fn calls_of(&self, method: &str) -> Vec<Value> {
        self.fake
            .cdp_calls()
            .into_iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p)
            .collect()
    }

    fn emit(&self, method: &str, params: Value) {
        self.fake.emit_cdp_event(TAB, method, params);
    }

    fn dialog(&self, kind: &str, message: &str) {
        self.emit(
            "Page.javascriptDialogOpening",
            json!({ "type": kind, "message": message, "defaultPrompt": "Bob",
                    "url": "https://tools.test/", "hasBrowserHandler": true }),
        );
    }
}

/// Let spawned tasks (listeners, auto-answers) run.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(5)).await;
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        out.push(event);
    }
    out
}

// ------------------------------------------------------------------ policy table

#[test]
fn policy_table() {
    let table = [
        (Rule::Accept, false, Decision::Accept),
        (Rule::Accept, true, Decision::Accept),
        (Rule::Dismiss, false, Decision::Dismiss),
        (Rule::Dismiss, true, Decision::Dismiss),
        (Rule::Ask, false, Decision::Leave),
        (Rule::Ask, true, Decision::Leave),
        (Rule::Auto, false, Decision::Leave),
        (Rule::Auto, true, Decision::Accept),
    ];
    for (rule, automation, expected) in table {
        assert_eq!(decide(rule, automation), expected, "{rule:?} {automation}");
    }

    // Defaults: everything is left to the agent/human, except beforeunload in automation.
    let defaults = Policy::defaults();
    for kind in [DialogType::Alert, DialogType::Confirm, DialogType::Prompt] {
        assert_eq!(defaults.rule(kind), Rule::Ask);
    }
    assert_eq!(defaults.rule(DialogType::Beforeunload), Rule::Auto);

    // Levels: tab over global over defaults, field by field.
    let global = Policy {
        confirm: Some(Rule::Accept),
        prompt_text: Some("x".into()),
        ..Policy::default()
    };
    let tab = Policy {
        confirm: Some(Rule::Dismiss),
        alert: Some(Rule::Accept),
        ..Policy::default()
    };
    let effective = tab.over(&global.over(&defaults));
    assert_eq!(effective.rule(DialogType::Alert), Rule::Accept);
    assert_eq!(effective.rule(DialogType::Confirm), Rule::Dismiss);
    assert_eq!(effective.rule(DialogType::Prompt), Rule::Ask);
    assert_eq!(effective.rule(DialogType::Beforeunload), Rule::Auto);
    assert_eq!(effective.prompt_text.as_deref(), Some("x"));
}

#[test]
fn guarded_commands() {
    for name in [
        "page.snapshot",
        "page.act",
        "input.keys",
        "nav.goto",
        "page.upload",
    ] {
        assert!(is_guarded(name), "{name}");
    }
    for name in [
        "page.dialog",
        "page.dialog_policy",
        "tabs.close",
        "net.list",
    ] {
        assert!(!is_guarded(name), "{name}");
    }
}

// ------------------------------------------------------------------ dialogs

#[tokio::test(start_paused = true)]
async fn open_dialog_blocks_page_commands_until_answered() {
    let env = Env::new();
    // Reading the policy starts watching the tab: Page.enable + event listener.
    let policy = env.call("page.dialog_policy", json!({})).await.unwrap();
    assert_eq!(policy["scope"], "tab");
    assert_eq!(policy["policy"]["alert"], "ask");
    assert_eq!(policy["policy"]["beforeunload"], "auto");
    assert_eq!(env.calls_of("Page.enable").len(), 1);

    env.dialog("alert", "Saved!");
    settle().await;

    // Another page command is refused at once, with the dialog in candidates[0].
    let err = env.call("page.probe", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::DialogOpen);
    assert!(
        err.message.contains("page.probe is blocked"),
        "{}",
        err.message
    );
    assert!(
        err.message
            .contains("an alert dialog is open on tab 1: \"Saved!\"")
    );
    assert_eq!(err.candidates[0]["type"], "alert");
    assert_eq!(err.candidates[0]["message"], "Saved!");
    assert_eq!(err.candidates[0]["tab"], 1);
    assert!(err.hint.unwrap().contains("page_dialog"));
    // Same with an explicit tab.
    let err = env
        .call("page.find_text", json!({"tab": 1, "query": "x"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::DialogOpen);
    // Non-page commands still work.
    env.call("tabs.list", json!({})).await.unwrap();

    // Answer it.
    let out = env
        .call("page.dialog", json!({"action": "accept"}))
        .await
        .unwrap();
    assert_eq!(out["dialog"]["type"], "alert");
    assert_eq!(
        env.calls_of("Page.handleJavaScriptDialog"),
        vec![json!({"accept": true})]
    );
    env.emit(
        "Page.javascriptDialogClosed",
        json!({"result": true, "userInput": ""}),
    );
    settle().await;
    env.call("page.probe", json!({})).await.unwrap();
    // Still one Page.enable: the tab is watched once.
    assert_eq!(env.calls_of("Page.enable").len(), 1);
}

#[tokio::test(start_paused = true)]
async fn dialog_opening_while_a_command_runs_stops_it() {
    let env = Env::new();
    env.call("page.dialog_policy", json!({})).await.unwrap();
    let registry = env.registry.clone();
    let fake = env.fake.clone();
    let running = tokio::spawn(async move {
        registry
            .call(ctx(mcp(), fake), "page.probe", json!({}))
            .await
    });
    settle().await;
    env.dialog("confirm", "Delete?");
    let err = running.await.unwrap().unwrap_err();
    assert_eq!(err.code, ErrorCode::DialogOpen);
    assert!(
        err.message
            .starts_with("page.probe stopped: a confirm dialog opened on tab 1: \"Delete?\""),
        "{}",
        err.message
    );
    // The close event clears it even if someone else (the human) answered.
    env.emit(
        "Page.javascriptDialogClosed",
        json!({"result": false, "userInput": ""}),
    );
    settle().await;
    env.call("page.probe", json!({})).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn page_dialog_without_a_dialog_is_not_found() {
    let env = Env::new();
    env.answer("Page.handleJavaScriptDialog", Err("No dialog is showing"));
    let err = env
        .call("page.dialog", json!({"action": "dismiss"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(err.message, "no JavaScript dialog is open on tab 1");
}

#[tokio::test(start_paused = true)]
async fn prompt_text_is_sent() {
    let env = Env::new();
    env.call("page.dialog_policy", json!({})).await.unwrap();
    env.dialog("prompt", "Name?");
    settle().await;
    env.call("page.dialog", json!({"action": "accept", "text": "Alice"}))
        .await
        .unwrap();
    assert_eq!(
        env.calls_of("Page.handleJavaScriptDialog"),
        vec![json!({"accept": true, "promptText": "Alice"})]
    );
}

#[tokio::test(start_paused = true)]
async fn policies_answer_dialogs_automatically() {
    let env = Env::new();
    let out = env
        .call(
            "page.dialog_policy",
            json!({"confirm": "dismiss", "prompt": "accept", "prompt_text": "auto"}),
        )
        .await
        .unwrap();
    assert_eq!(out["policy"]["confirm"], "dismiss");
    // The MCP call itself counts as automation.
    assert_eq!(out["automation_active"], true);

    env.dialog("confirm", "Sure?");
    settle().await;
    env.dialog("prompt", "Name?");
    settle().await;
    // beforeunload: `auto` + automation active → accepted.
    env.dialog("beforeunload", "");
    settle().await;
    assert_eq!(
        env.calls_of("Page.handleJavaScriptDialog"),
        vec![
            json!({"accept": false}),
            json!({"accept": true, "promptText": "auto"}),
            json!({"accept": true}),
        ]
    );
    // Nothing was left open: page commands run.
    env.call("page.probe", json!({})).await.unwrap();

    // Global rules apply to tabs without their own rule; tab rules win.
    env.call(
        "page.dialog_policy",
        json!({"global": true, "confirm": "accept", "alert": "accept"}),
    )
    .await
    .unwrap();
    let out = env.call("page.dialog_policy", json!({})).await.unwrap();
    assert_eq!(out["policy"]["alert"], "accept");
    assert_eq!(out["policy"]["confirm"], "dismiss");
    let out = env
        .call("page.dialog_policy", json!({"reset": true}))
        .await
        .unwrap();
    assert_eq!(out["policy"]["confirm"], "accept");
    assert_eq!(out["policy"]["prompt"], "ask");

    let err = env
        .call("page.dialog_policy", json!({"global": true, "tab": 1}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test(start_paused = true)]
async fn beforeunload_is_left_to_the_human_without_automation() {
    let env = Env::new();
    // Only the UI has called anything: no automation.
    env.call_as(Caller::Ui, "page.dialog_policy", json!({}))
        .await
        .unwrap();
    env.dialog("beforeunload", "");
    settle().await;
    assert!(env.calls_of("Page.handleJavaScriptDialog").is_empty());
    let err = env
        .call_as(Caller::Ui, "page.probe", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.candidates[0]["type"], "beforeunload");

    // A failed auto-answer leaves the dialog open (so commands report it).
    let env = Env::new();
    env.answer("Page.handleJavaScriptDialog", Err("boom"));
    env.call("page.dialog_policy", json!({"alert": "accept"}))
        .await
        .unwrap();
    env.dialog("alert", "x");
    settle().await;
    let err = env.call("page.probe", json!({})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::DialogOpen);
}

#[tokio::test(start_paused = true)]
async fn dialog_events_are_emitted() {
    let env = Env::new();
    // The watcher emits on the bus it was started with: start it like the app does.
    let bus = bmcp_core::EventBus::new();
    let mut rx = bus.subscribe();
    crate::start(&env.registry, env.fake.clone(), bus);
    settle().await;
    assert_eq!(env.calls_of("Page.enable").len(), 1);
    env.dialog("confirm", "Delete?");
    settle().await;
    env.emit(
        "Page.javascriptDialogClosed",
        json!({"result": true, "userInput": ""}),
    );
    settle().await;
    let events: Vec<(String, Value)> = drain(&mut rx)
        .into_iter()
        .map(|e| (e.topic, e.data))
        .collect();
    assert_eq!(
        events,
        vec![
            (
                "page.dialog_opened".to_string(),
                json!({"tab": 1, "type": "confirm", "message": "Delete?", "default_prompt": "Bob",
                       "url": "https://tools.test/", "handling": "ask"})
            ),
            (
                "page.dialog_closed".to_string(),
                json!({"tab": 1, "accepted": true, "user_input": ""})
            ),
        ]
    );
}

// ------------------------------------------------------------------ upload

/// Give the tab a snapshot with one ref, `@1:e1` = DOM node 42.
async fn with_ref(env: &Env) -> String {
    use bmcp_perception::{AxTree, PageHeader, SnapshotOptions, snapshot};
    let tree = AxTree::from_cdp(&json!({"nodes": [
        {"nodeId": "1", "role": {"type": "internalRole", "value": "RootWebArea"},
         "name": {"type": "computedString", "value": "Tools"}, "childIds": ["2"]},
        {"nodeId": "2", "parentId": "1", "role": {"type": "role", "value": "button"},
         "name": {"type": "computedString", "value": "Choose file"}, "backendDOMNodeId": 42}
    ]}))
    .unwrap();
    let state = bmcp_browser::PerceptionState::default();
    let perception = env.registry.extensions().get_or_insert_with(|| state);
    let tab = perception.tab(TAB);
    let mut tab = tab.lock().await;
    tab.loader_id = Some(LOADER.to_string());
    let header = PageHeader {
        tab: 1,
        ..Default::default()
    };
    let snap = snapshot(
        &tree,
        &mut tab.registry,
        &SnapshotOptions::default(),
        header,
    )
    .unwrap();
    snap.refs[0].ref_str.clone()
}

/// A run folder with two files; returns their absolute plain paths.
fn run_files(env: &Env) -> (String, String) {
    let dir = env.tmp.path().join("runs").join("r1");
    std::fs::create_dir_all(dir.join("in")).unwrap();
    std::fs::write(dir.join("in").join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("b.txt"), "b").unwrap();
    std::fs::write(env.tmp.path().join("secret.txt"), "s").unwrap();
    let plain = |p: std::path::PathBuf| {
        crate::util::plain_path(std::fs::canonicalize(p).unwrap())
            .to_string_lossy()
            .into_owned()
    };
    (
        plain(dir.join("in").join("a.txt")),
        plain(dir.join("b.txt")),
    )
}

#[test]
fn upload_paths_are_checked() {
    let env = Env::new();
    let (a, b) = run_files(&env);
    let root = env.tmp.path();
    let files = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let ok =
        crate::upload::resolve_files(root, Some("r1"), &files(&["in/a.txt", "./b.txt"])).unwrap();
    let ok: Vec<String> = ok
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    assert_eq!(ok, vec![a.clone(), b]);
    assert!(!ok[0].starts_with(r"\\?\"));
    // Absolute paths are fine with or without a run.
    crate::upload::resolve_files(root, None, &files(&[&a])).unwrap();

    let cases: &[(Option<&str>, &str, ErrorCode, &str)] = &[
        (
            Some("r1"),
            "../../secret.txt",
            ErrorCode::InvalidParams,
            "leaves the run folder",
        ),
        (
            Some("r1"),
            "in/../../r1/../../secret.txt",
            ErrorCode::InvalidParams,
            "leaves",
        ),
        (
            Some("r1"),
            "missing.txt",
            ErrorCode::NotFound,
            "file not found",
        ),
        (Some("r1"), "in", ErrorCode::InvalidParams, "is not a file"),
        (None, "b.txt", ErrorCode::InvalidParams, "is relative"),
        (Some("nope"), "b.txt", ErrorCode::NotFound, "does not exist"),
        (Some(".."), "b.txt", ErrorCode::InvalidParams, "bad run_id"),
        (
            Some("r1/../x"),
            "b.txt",
            ErrorCode::InvalidParams,
            "bad run_id",
        ),
    ];
    for (run, file, code, text) in cases {
        let err = crate::upload::resolve_files(root, *run, &files(&[file])).unwrap_err();
        assert_eq!(err.code, *code, "{file}: {}", err.message);
        assert!(err.message.contains(text), "{file}: {}", err.message);
    }
    let err = crate::upload::resolve_files(root, None, &[]).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test(start_paused = true)]
async fn upload_into_a_file_input() {
    let env = Env::new();
    let (a, b) = run_files(&env);
    let target = with_ref(&env).await;
    env.answer(
        "DOM.describeNode",
        Ok(json!({"node": {"nodeName": "INPUT", "attributes": ["type", "FILE", "multiple", ""]}})),
    );
    let out = env
        .call(
            "page.upload",
            json!({"ref": target, "files": ["in/a.txt", "b.txt"], "run_id": "r1"}),
        )
        .await
        .unwrap();
    assert_eq!(out["via"], "input");
    assert_eq!(out["files"], json!([a, b]));
    assert_eq!(
        env.calls_of("DOM.setFileInputFiles"),
        vec![json!({"files": [a, b], "backendNodeId": 42})]
    );
    assert!(
        env.calls_of("Page.setInterceptFileChooserDialog")
            .is_empty()
    );

    // A single-file input refuses two files, before setting anything.
    env.answer(
        "DOM.describeNode",
        Ok(json!({"node": {"nodeName": "INPUT", "attributes": ["type", "file"]}})),
    );
    let err = env
        .call(
            "page.upload",
            json!({"ref": target, "files": ["in/a.txt", "b.txt"], "run_id": "r1"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert_eq!(env.calls_of("DOM.setFileInputFiles").len(), 1);

    // A ref from another document is stale.
    env.answer(
        "Page.getFrameTree",
        Ok(json!({"frameTree": {"frame": {"id": "F1", "loaderId": "L2"}}})),
    );
    let err = env
        .call("page.upload", json!({"ref": target, "files": [a]}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::StaleRef);
}

#[tokio::test(start_paused = true)]
async fn upload_through_a_file_chooser() {
    let env = Env::new();
    let (a, _) = run_files(&env);
    let target = with_ref(&env).await;
    env.answer(
        "DOM.describeNode",
        Ok(json!({"node": {"nodeName": "BUTTON", "attributes": []}})),
    );
    env.answer(
        "DOM.getContentQuads",
        Ok(json!({"quads": [[10, 20, 30, 20, 30, 40, 10, 40]]})),
    );
    let registry = env.registry.clone();
    let fake = env.fake.clone();
    let args = json!({"ref": target, "files": ["in/a.txt"], "run_id": "r1"});
    let call =
        tokio::spawn(async move { registry.call(ctx(mcp(), fake), "page.upload", args).await });
    // Wait for the click, then open the chooser like Chromium does.
    while !env
        .calls_of("Input.dispatchMouseEvent")
        .iter()
        .any(|p| p["type"] == "mouseReleased")
    {
        settle().await;
    }
    env.emit(
        "Page.fileChooserOpened",
        json!({"frameId": "F1", "mode": "selectSingle", "backendNodeId": 77}),
    );
    let out = call.await.unwrap().unwrap();
    assert_eq!(out["via"], "chooser");
    settle().await; // interception is switched off from a task

    let sequence: Vec<String> = env
        .fake
        .cdp_calls()
        .into_iter()
        .filter(|(m, _)| m != "Page.getFrameTree")
        .map(|(m, p)| match m.as_str() {
            "Input.dispatchMouseEvent" => format!("{m} {} {},{}", p["type"], p["x"], p["y"]),
            "Page.setInterceptFileChooserDialog" => format!("{m} {}", p["enabled"]),
            "DOM.setFileInputFiles" => format!("{m} {}", p["backendNodeId"]),
            _ => m,
        })
        .collect();
    assert_eq!(
        sequence,
        vec![
            "DOM.describeNode",
            "Page.enable",
            "Page.setInterceptFileChooserDialog true",
            "DOM.scrollIntoViewIfNeeded",
            "DOM.getContentQuads",
            "Input.dispatchMouseEvent \"mouseMoved\" 20.0,30.0",
            "Input.dispatchMouseEvent \"mousePressed\" 20.0,30.0",
            "Input.dispatchMouseEvent \"mouseReleased\" 20.0,30.0",
            "DOM.setFileInputFiles 77",
            "Page.setInterceptFileChooserDialog false",
        ]
    );
    assert_eq!(
        env.calls_of("DOM.setFileInputFiles")[0]["files"],
        json!([a])
    );
}

#[tokio::test(start_paused = true)]
async fn upload_chooser_timeout_switches_interception_off() {
    let env = Env::new();
    let (a, _) = run_files(&env);
    let target = with_ref(&env).await;
    env.answer(
        "DOM.describeNode",
        Ok(json!({"node": {"nodeName": "DIV", "attributes": []}})),
    );
    env.answer(
        "DOM.getContentQuads",
        Ok(json!({"quads": [[0, 0, 4, 0, 4, 4, 0, 4]]})),
    );
    let err = env
        .call(
            "page.upload",
            json!({"ref": target, "files": [a], "timeout_ms": 200}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotInteractable);
    assert!(
        err.message
            .contains("did not open a file chooser within 200 ms")
    );
    settle().await;
    assert_eq!(
        env.calls_of("Page.setInterceptFileChooserDialog"),
        vec![json!({"enabled": true}), json!({"enabled": false})]
    );
}

#[tokio::test(start_paused = true)]
async fn upload_cancelled_by_a_dialog_still_switches_interception_off() {
    let env = Env::new();
    env.call("page.dialog_policy", json!({})).await.unwrap(); // watch dialogs
    let (a, _) = run_files(&env);
    let target = with_ref(&env).await;
    env.answer(
        "DOM.describeNode",
        Ok(json!({"node": {"nodeName": "BUTTON", "attributes": []}})),
    );
    env.answer(
        "DOM.getContentQuads",
        Ok(json!({"quads": [[0, 0, 4, 0, 4, 4, 0, 4]]})),
    );
    let registry = env.registry.clone();
    let fake = env.fake.clone();
    let args = json!({"ref": target, "files": [a]});
    let call =
        tokio::spawn(async move { registry.call(ctx(mcp(), fake), "page.upload", args).await });
    while env.calls_of("Input.dispatchMouseEvent").len() < 3 {
        settle().await;
    }
    // The "button" opened an alert instead of a chooser.
    env.dialog("alert", "No uploads today");
    let err = call.await.unwrap().unwrap_err();
    assert_eq!(err.code, ErrorCode::DialogOpen);
    settle().await;
    assert_eq!(
        env.calls_of("Page.setInterceptFileChooserDialog"),
        vec![json!({"enabled": true}), json!({"enabled": false})]
    );
}

// ------------------------------------------------------------------ pdf / mhtml

#[tokio::test(start_paused = true)]
async fn pdf_and_mhtml_files() {
    use base64::Engine;
    let env = Env::new();
    let pdf = b"%PDF-1.7 test";
    let data = base64::engine::general_purpose::STANDARD.encode(pdf);
    env.answer("Page.printToPDF", Ok(json!({ "data": data })));
    let out = env
        .call(
            "page.pdf",
            json!({"landscape": true, "print_background": true, "scale": 0.8, "paper": "a4"}),
        )
        .await
        .unwrap();
    let path = out["path"].as_str().unwrap();
    assert!(path.contains("pdf"), "{path}");
    assert_eq!(out["bytes"], pdf.len());
    assert_eq!(std::fs::read(path).unwrap(), pdf);
    let params = &env.calls_of("Page.printToPDF")[0];
    assert_eq!(params["landscape"], true);
    assert_eq!(params["printBackground"], true);
    assert_eq!(params["scale"], 0.8);
    assert!((params["paperWidth"].as_f64().unwrap() - 8.2677).abs() < 0.001);
    assert!((params["paperHeight"].as_f64().unwrap() - 11.6929).abs() < 0.001);

    // A relative path lands in data/pdf/, `..` is refused, scale is checked.
    let out = env
        .call("page.pdf", json!({"path": "sub/x.pdf"}))
        .await
        .unwrap();
    let expected = env.tmp.path().join("pdf").join("sub").join("x.pdf");
    assert_eq!(out["path"], expected.to_string_lossy().as_ref());
    assert!(expected.is_file());
    for args in [json!({"path": "../x.pdf"}), json!({"scale": 3})] {
        let err = env.call("page.pdf", args).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    env.answer(
        "Page.captureSnapshot",
        Ok(json!({ "data": "From: <Saved by Blink>\r\n" })),
    );
    let out = env.call("page.mhtml", json!({})).await.unwrap();
    let path = out["path"].as_str().unwrap();
    assert!(path.ends_with(".mhtml"), "{path}");
    assert_eq!(out["bytes"], 24);
    assert_eq!(
        env.calls_of("Page.captureSnapshot"),
        vec![json!({"format": "mhtml"})]
    );
}

// ------------------------------------------------------------------ find in page

#[tokio::test(start_paused = true)]
async fn find_steps_through_matches_in_an_isolated_world() {
    let env = Env::new();
    env.answer(
        "Page.createIsolatedWorld",
        Ok(json!({ "executionContextId": 7 })),
    );
    env.answer(
        "Runtime.evaluate",
        Ok(json!({"result": {"type": "object", "value": {"count": 3, "index": 0, "context": "a foo b"}}})),
    );
    let out = env
        .call("page.find_text", json!({"query": "foo"}))
        .await
        .unwrap();
    assert_eq!(
        out,
        json!({"tab": 1, "count": 3, "index": 1, "context": "a foo b"})
    );

    env.answer(
        "Runtime.evaluate",
        Ok(json!({"result": {"type": "object", "value": {"count": 3, "index": 1, "context": "c foo"}}})),
    );
    let out = env
        .call("page.find_text", json!({"query": "foo", "next": true}))
        .await
        .unwrap();
    assert_eq!(out["index"], 2);
    // New query → first match again; prev on a new query → last match (-1 wraps in JS).
    env.call("page.find_text", json!({"query": "bar", "case": true}))
        .await
        .unwrap();
    env.call("page.find_text", json!({"query": "baz", "prev": true}))
        .await
        .unwrap();
    env.call("page.find_clear", json!({})).await.unwrap();

    let evals = env.calls_of("Runtime.evaluate");
    let tails: Vec<String> = evals
        .iter()
        .map(|p| {
            assert_eq!(p["contextId"], 7);
            let expr = p["expression"].as_str().unwrap();
            assert!(expr.contains("CSS.highlights"));
            expr.rsplit(")(").next().unwrap().to_string()
        })
        .collect();
    assert_eq!(
        tails,
        vec![
            "\"foo\", false, 0)",
            "\"foo\", false, 1)",
            "\"bar\", true, 0)",
            "\"baz\", false, -1)",
            "\"\", false, 0)",
        ]
    );
    assert_eq!(
        env.calls_of("Page.createIsolatedWorld")[0],
        json!({"frameId": "F1", "worldName": "bmcp-find"})
    );
    let err = env
        .call(
            "page.find_text",
            json!({"query": "x", "next": true, "prev": true}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}
