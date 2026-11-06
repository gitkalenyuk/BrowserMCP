//! Self-healing end to end against the fake script host: heal_needed → snapshot hook →
//! proposal (canned AI answers) → accept / reject / auto → the run finishes with the patch.

mod common;

use bmcp_heal::topics;
use common::{BROKEN, Env, FIX_DIFF, FakeLlm, answer, wait_event};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn ask_mode_proposes_and_accept_heals_the_rest_of_the_run() {
    let llm = FakeLlm::new(&[answer(FIX_DIFF, 0.9)]);
    let env = Env::new(Some(llm.clone()));
    let mut rx = env.events.subscribe();
    let id = env.start(BROKEN, &["a", "b", "c"], "ask").await;

    // The watcher proposes on its own and announces the proposal.
    let proposed = wait_event(&mut rx, topics::HEAL_PROPOSED, |d| d["id"] == id.as_str()).await;
    let d = &proposed.data;
    assert_eq!(d["row"], 0);
    assert_eq!(d["mode"], "ask");
    assert_eq!(d["attempt"], 1);
    assert_eq!(d["max_attempts"], 3);
    assert!(
        d["diff"]
            .as_str()
            .unwrap()
            .contains("+await act(\"button\", \"Log in\"")
    );
    assert_eq!(d["explanation"], "the button is now called Log in");
    assert!(
        d["patched_code"]
            .as_str()
            .unwrap()
            .contains("\"Log in\", \"click\"")
    );
    assert_eq!(d["checks"], json!(["applies", "region 1-20", "transpiles"]));

    // The AI saw the error, the marked failing line, the locators and the page snapshot that
    // the jobs hook stored when the row broke.
    let prompt = &llm.prompts()[0];
    for part in [
        "code: NOT_FOUND",
        "command: page.find",
        ">>   5 | fail NOT_FOUND",
        "Element locators",
        "- button \"Log in\" @1:e2",
    ] {
        assert!(
            prompt.contains(part),
            "missing {part:?} in prompt:\n{prompt}"
        );
    }
    let snapshot = env.jobs.heal_snapshot(&id, Some(0)).unwrap().unwrap();
    assert!(snapshot.contains("@1:e2"), "{snapshot}");

    // Still paused until a person accepts.
    let status = env.wait_status(&id, &["paused"]).await;
    assert_eq!(status["counts"]["needs_heal"], 1);

    let proposal_id = d["proposal_id"].as_str().unwrap();
    let applied = env
        .ok("run.heal_accept", json!({"proposal_id": proposal_id}))
        .await;
    assert_eq!(applied["script"], "login.v2");
    assert_eq!(applied["requeued"], 1);
    let event = wait_event(&mut rx, topics::HEAL_APPLIED, |d| d["id"] == id.as_str()).await;
    assert_eq!(event.data["auto"], false);

    let status = env.wait_status(&id, &["finished"]).await;
    assert_eq!(status["counts"]["ok"], 3, "{status:#}");
    let code = env.ok("script.get", json!({"name": "login.v2"})).await;
    assert!(code["code"].as_str().unwrap().contains("\"Log in\""));

    // Accepting twice is refused; the list shows the status and the patched code.
    let again = env
        .call("run.heal_accept", json!({"proposal_id": proposal_id}))
        .await
        .unwrap_err();
    assert!(again.message.contains("not pending"), "{again:?}");
    let list = env.ok("run.heal_proposals", json!({"id": id})).await;
    assert_eq!(list["proposals"][0]["status"], "accepted");
    assert!(
        list["proposals"][0]["patched_code"]
            .as_str()
            .unwrap()
            .contains("Log in")
    );
    assert_eq!(llm.prompts().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn reject_with_feedback_proposes_again() {
    let second = FIX_DIFF.replace("\"click\"", "\"click\", undefined, 1");
    let llm = FakeLlm::new(&[answer(FIX_DIFF, 0.7), answer(&second, 0.8)]);
    let env = Env::new(Some(llm.clone()));
    let mut rx = env.events.subscribe();
    let id = env.start(BROKEN, &["a", "b"], "ask").await;

    let first = wait_event(&mut rx, topics::HEAL_PROPOSED, |d| d["id"] == id.as_str()).await;
    let first_id = first.data["proposal_id"].as_str().unwrap().to_string();
    let next = env
        .ok(
            "run.heal_reject",
            json!({"proposal_id": first_id, "feedback": "click the second Log in button"}),
        )
        .await;
    assert_eq!(next["attempt"], 2);
    assert_ne!(next["proposal_id"], first_id.as_str());
    let announced = wait_event(&mut rx, topics::HEAL_PROPOSED, |d| d["attempt"] == 2).await;
    assert_eq!(announced.data["proposal_id"], next["proposal_id"]);

    // The second prompt carries the reviewer's words.
    let prompts = llm.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].contains("click the second Log in button"),
        "{}",
        prompts[1]
    );
    assert!(prompts[1].contains("rejected attempt 1"));

    let list = env.ok("run.heal_proposals", json!({"id": id})).await;
    let statuses: Vec<&Value> = list["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["status"])
        .collect();
    assert_eq!(statuses, vec![&json!("pending"), &json!("rejected")]);

    env.ok(
        "run.heal_accept",
        json!({"proposal_id": next["proposal_id"]}),
    )
    .await;
    let status = env.wait_status(&id, &["finished"]).await;
    assert_eq!(status["counts"]["ok"], 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_answers_are_retried_then_the_heal_fails() {
    let llm = FakeLlm::new(&[
        "Sorry, I cannot help with that.".into(),
        // Does not transpile (the fake host's check rejects SYNTAX_ERROR).
        json!({"patched_code": BROKEN.replace("fail NOT_FOUND", "SYNTAX_ERROR"), "confidence": 0.9})
            .to_string(),
        // Changes nothing.
        json!({"patched_code": BROKEN}).to_string(),
    ]);
    let env = Env::new(Some(llm.clone()));
    let mut rx = env.events.subscribe();
    let id = env.start(BROKEN, &["a"], "ask").await;

    let failed = wait_event(&mut rx, topics::HEAL_FAILED, |d| d["id"] == id.as_str()).await;
    assert_eq!(failed.data["attempts"], 3);
    assert!(
        failed.data["error"]
            .as_str()
            .unwrap()
            .contains("changes nothing")
    );
    let prompts = llm.prompts();
    assert_eq!(prompts.len(), 3);
    assert!(prompts[1].contains("attempt 1 was invalid: the answer was not a JSON object"));
    assert!(prompts[2].contains("does not compile"), "{}", prompts[2]);

    // The run stays paused; a manual propose has no attempts left either.
    env.wait_status(&id, &["paused"]).await;
    let err = env
        .call("run.heal_propose", json!({"id": id}))
        .await
        .unwrap_err();
    assert!(err.message.contains("no usable patch"), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn patches_outside_the_failing_region_need_a_justification() {
    // 40 filler lines, then the broken step at line 43: changing line 2 is far away.
    let mut script = String::from("// header\nconst url = inputs.url;\n");
    for i in 0..40 {
        script.push_str(&format!("// filler {i}\n"));
    }
    script.push_str("fail NOT_FOUND page.find no Sign in\nresult {\"ok\": true}\n");
    let far_change = script
        .replace("const url = inputs.url;", "const url = \"https://x.test\";")
        .replace("fail NOT_FOUND page.find no Sign in", "// fixed");
    let llm = FakeLlm::new(&[
        json!({"patched_code": far_change, "confidence": 0.9}).to_string(),
        json!({"patched_code": far_change, "confidence": 0.9,
               "justification": "the start URL moved"})
        .to_string(),
    ]);
    let env = Env::new(Some(llm.clone()));
    let mut rx = env.events.subscribe();
    let id = env.start(&script, &["a"], "ask").await;
    let proposed = wait_event(&mut rx, topics::HEAL_PROPOSED, |d| d["id"] == id.as_str()).await;
    assert_eq!(proposed.data["attempt"], 2);
    let checks = proposed.data["checks"].to_string();
    assert!(
        checks.contains("justified: the start URL moved"),
        "{checks}"
    );
    assert!(llm.prompts()[1].contains("outside the failing step's region (lines 28-58)"));
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_mode_applies_confident_patches_after_a_dry_run() {
    let llm = FakeLlm::new(&[answer(FIX_DIFF, 0.95)]);
    let env = Env::new(Some(llm.clone()));
    let mut rx = env.events.subscribe();
    let id = env.start(BROKEN, &["a", "b"], "auto").await;

    let applied = wait_event(&mut rx, topics::HEAL_APPLIED, |d| d["id"] == id.as_str()).await;
    assert_eq!(applied.data["auto"], true);
    assert_eq!(applied.data["script"], "login.v2");
    let status = env.wait_status(&id, &["finished"]).await;
    assert_eq!(status["counts"]["ok"], 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_mode_asks_below_the_threshold_or_when_the_dry_run_fails() {
    // Low confidence: announced for a person instead.
    let llm = FakeLlm::new(&[answer(FIX_DIFF, 0.5)]);
    let env = Env::new(Some(llm));
    let mut rx = env.events.subscribe();
    let id = env.start(BROKEN, &["a"], "auto").await;
    let proposed = wait_event(&mut rx, topics::HEAL_PROPOSED, |d| d["id"] == id.as_str()).await;
    assert_eq!(proposed.data["mode"], "auto");
    let note = proposed.data["note"].as_str().unwrap();
    assert!(note.contains("confidence 0.50 is below 0.80"), "{note}");
    env.wait_status(&id, &["paused"]).await;

    // Confident, but the patched script still fails in the dry run.
    let still_broken = FIX_DIFF.replace(
        "+await act(\"button\", \"Log in\", \"click\");",
        "+fail NOT_FOUND page.find still no button",
    );
    let llm = FakeLlm::new(&[answer(&still_broken, 0.9)]);
    let env = Env::new(Some(llm));
    let mut rx = env.events.subscribe();
    let id = env.start(BROKEN, &["a"], "auto").await;
    let proposed = wait_event(&mut rx, topics::HEAL_PROPOSED, |d| d["id"] == id.as_str()).await;
    let note = proposed.data["note"].as_str().unwrap();
    assert!(
        note.contains("dry run of row 0 failed: NOT_FOUND"),
        "{note}"
    );
    let status = env.wait_status(&id, &["paused"]).await;
    assert_eq!(status["script"], "login");
}

#[tokio::test(flavor = "multi_thread")]
async fn propose_needs_an_open_heal_request() {
    let env = Env::new(Some(FakeLlm::new(&[])));
    env.ok("script.save", json!({"name": "ok", "code": "result {}"}))
        .await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "ok", "data": {"rows": [{"a": "1"}]}}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    env.wait_status(id, &["finished"]).await;
    let err = env
        .call("run.heal_propose", json!({"id": id}))
        .await
        .unwrap_err();
    assert!(err.message.contains("no open heal request"), "{err:?}");
    let err = env
        .call("run.heal_accept", json!({"proposal_id": "heal-x"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NotFound);
}

/// The real provider path (bmcp_ai::AiService → OpenAI-compatible HTTP) against a mock server.
#[tokio::test(flavor = "multi_thread")]
async fn proposals_through_a_mocked_openai_compatible_provider() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let chunk = json!({"choices": [{"index": 0,
        "delta": {"content": answer(FIX_DIFF, 0.9)}, "finish_reason": "stop"}]});
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {chunk}\n\ndata: [DONE]\n\n")),
        )
        .mount(&server)
        .await;

    let env = Env::new(None);
    let provider = env
        .ok(
            "ai.provider_set",
            json!({"id": "custom", "base_url": format!("{}/v1", server.uri()),
                   "model": "test-model", "default": true}),
        )
        .await;
    assert_eq!(provider["is_default"], true);

    let mut rx = env.events.subscribe();
    let id = env.start(BROKEN, &["a"], "ask").await;
    let proposed = wait_event(&mut rx, topics::HEAL_PROPOSED, |d| d["id"] == id.as_str()).await;
    assert!(proposed.data["diff"].as_str().unwrap().contains("Log in"));
    let list = env.ok("run.heal_proposals", json!({"id": id})).await;
    assert_eq!(list["proposals"][0]["provider"], "custom");
    assert_eq!(list["proposals"][0]["model"], "test-model");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("You repair BrowserMCP automation scripts"));
    assert!(body.contains("NOT_FOUND"));
}
