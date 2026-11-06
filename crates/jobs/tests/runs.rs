//! Script library, script.run and mass runs against the fake script host
//! (`src/bin/fake_script_host.rs`; its "scripts" are a tiny line DSL, not TypeScript).

mod common;

use std::time::Duration;

use bmcp_core::ErrorCode;
use bmcp_jobs::topics;
use common::{Env, WS_URL, id_rows, wait_event};
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn script_library_commands() {
    let env = Env::new();
    env.save("login", "out hi").await;
    let list = env.ok("script.list", json!({})).await;
    assert_eq!(list["scripts"][0]["name"], "login");
    let got = env.ok("script.get", json!({"name": "login"})).await;
    assert_eq!(got["code"], "out hi");
    let err = env
        .call("script.save", json!({"name": "../evil", "code": "x"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let deleted = env.ok("script.delete", json!({"name": "login"})).await;
    assert_eq!(deleted["deleted"], true);
    let err = env
        .call("script.get", json!({"name": "login"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn script_run_passes_inputs_and_reads_results() {
    let env = Env::new();
    env.save("hello", "args\nout hello {{name}}\nresult {\"x\": 1}")
        .await;
    let out = env
        .ok(
            "script.run",
            json!({"name": "hello", "inputs": {"name": "Ada", "n": 3}, "timeout_secs": 7}),
        )
        .await;
    assert_eq!(out["ok"], true, "{out:#}");
    assert_eq!(out["exit_code"], 0);
    assert_eq!(out["result"]["ws"], WS_URL);
    assert_eq!(out["result"]["inputs"], json!({"name": "Ada", "n": "3"}));
    assert_eq!(out["result"]["timeout_secs"].as_f64(), Some(7.0));
    assert_eq!(out["result"]["x"], 1);
    assert_eq!(out["stdout"], "hello Ada\n");

    // Regression: a value far past the Windows 32K command-line limit, with a NUL byte, still
    // reaches the host (inputs travel in a temp JSON file, not as `--input` arguments).
    let long = format!("{}{}tail", "y".repeat(40_000), char::from(0));
    let out = env
        .ok(
            "script.run",
            json!({"name": "hello", "inputs": {"name": "Ada", "body": long}}),
        )
        .await;
    assert_eq!(out["ok"], true, "{out:#}");
    assert_eq!(out["result"]["inputs"]["body"], long.as_str());

    // Inline code; temp file is removed afterwards and not listed.
    let out = env
        .ok("script.run", json!({"code": "result {\"inline\": true}"}))
        .await;
    assert_eq!(out["result"], json!({"inline": true}));
    let list = env.ok("script.list", json!({})).await;
    assert_eq!(list["scripts"].as_array().unwrap().len(), 1);
    let leftovers = std::fs::read_dir(env.root().join("scripts"))
        .unwrap()
        .count();
    assert_eq!(leftovers, 1, "temp script not cleaned up");

    // A failed browser call is parsed.
    let out = env
        .ok(
            "script.run",
            json!({"code": "out step 1\nfail NOT_FOUND page.find no button \"Save\""}),
        )
        .await;
    assert_eq!(out["ok"], false);
    assert_eq!(out["exit_code"], 1);
    assert_eq!(out["error"]["code"], "NOT_FOUND");
    assert_eq!(out["error"]["command"], "page.find");
    assert_eq!(out["error"]["line"], 2);

    // The host's own timeout (exit 124).
    let out = env
        .ok(
            "script.run",
            json!({"code": "sleep 5000", "timeout_secs": 1}),
        )
        .await;
    assert_eq!(out["exit_code"], 124);
    assert_eq!(out["error"]["code"], "SCRIPT_TIMEOUT");

    let err = env
        .call("script.run", json!({"name": "missing"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = env
        .call("script.run", json!({"name": "hello", "code": "x"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_host_binary_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let mut config = common::config(tmp.path());
    config.host_exe = Some(tmp.path().join("nope.exe"));
    let env = Env::with_config(tmp, config);
    env.save("a", "out x").await;
    let err = env
        .call("script.run", json!({"name": "a"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.message.contains("script host"), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn rows_run_in_parallel_slots() {
    let env = Env::new();
    let mut rx = env.events.subscribe();
    env.save(
        "job",
        "time start\nsleep 400\ntime end\nresult {\"id\": \"{{id}}\"}",
    )
    .await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "job", "data": {"rows": id_rows(6)}, "parallel": 3}),
        )
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    assert_eq!(run["options"]["parallel"], 3);
    assert_eq!(run["total"], 6);

    let finished = wait_event(&mut rx, topics::RUN_FINISHED, |d| d["id"] == id.as_str()).await;
    assert_eq!(finished.data["status"], "finished");
    assert_eq!(finished.data["ok"], 6);

    let rows = env.rows(&id).await;
    assert_eq!(rows.len(), 6);
    let mut spans: Vec<(u64, u64)> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(row["status"], "ok");
        assert_eq!(row["result"]["id"], i.to_string());
        assert_eq!(row["inputs"]["id"], i.to_string());
        assert_eq!(row["attempts"], 1);
        spans.push((
            row["result"]["start"].as_u64().unwrap(),
            row["result"]["end"].as_u64().unwrap(),
        ));
    }
    // At most 3 at once, and more than 1 (they really overlapped).
    let max_overlap = spans
        .iter()
        .map(|(s, _)| spans.iter().filter(|(s2, e2)| s2 <= s && s < e2).count())
        .max()
        .unwrap();
    assert!(
        (2..=3).contains(&max_overlap),
        "overlap {max_overlap}: {spans:?}"
    );

    let status = env.ok("run.status", json!({"id": id})).await;
    assert_eq!(status["done"], 6);
    assert_eq!(status["counts"]["ok"], 6);
    let list = env.ok("run.list", json!({})).await;
    assert_eq!(list["runs"][0]["id"], id.as_str());
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_rows_are_retried_with_backoff() {
    let env = Env::new();
    let mut rx = env.events.subscribe();
    env.save(
        "flaky",
        "flaky {{id}} 1 fail TIMEOUT nav.goto slow page\nresult {\"done\": \"{{id}}\"}",
    )
    .await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "flaky", "data": {"rows": [{"id": "a"}, {"id": "b"}]}, "retries": 1}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    env.wait_status(id, &["finished"]).await;
    for row in env.rows(id).await {
        assert_eq!(row["status"], "ok", "{row:#}");
        assert_eq!(row["attempts"], 2);
    }

    // Without retries the first failure is final and reported.
    let run = env
        .ok(
            "run.start",
            json!({"script": "flaky", "data": {"rows": [{"id": "c"}]}, "retries": 0}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    let failed = wait_event(&mut rx, topics::RUN_ROW_FAILED, |d| d["id"] == id).await;
    assert_eq!(failed.data["row"], 0);
    assert_eq!(failed.data["error"]["code"], "TIMEOUT");
    assert_eq!(failed.data["error"]["command"], "nav.goto");
    let status = env.wait_status(id, &["finished"]).await;
    assert_eq!(status["counts"]["failed"], 1);
    let rows = env.rows(id).await;
    assert_eq!(rows[0]["attempts"], 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_on_error_cancels_the_rest() {
    let env = Env::new();
    env.save("s", "when id=1 throw boom\nresult {\"ok\": 1}")
        .await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(5)}, "parallel": 1,
                   "retries": 0, "stop_on_error": true}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    let status = env.wait_status(id, &["failed"]).await;
    assert_eq!(status["counts"]["ok"], 1);
    assert_eq!(status["counts"]["failed"], 1);
    assert_eq!(status["counts"]["cancelled"], 3);
    assert!(
        status["note"].as_str().unwrap().contains("boom"),
        "{status:#}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_kills_running_scripts() {
    let env = Env::new();
    let mut rx = env.events.subscribe();
    env.save("slow", "sleep 20000").await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "slow", "data": {"rows": id_rows(4)}, "parallel": 2}),
        )
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = std::time::Instant::now();
    env.ok("run.cancel", json!({"id": id})).await;
    let finished = wait_event(&mut rx, topics::RUN_FINISHED, |d| d["id"] == id.as_str()).await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(finished.data["status"], "cancelled");
    assert_eq!(finished.data["cancelled"], 4);
    // Cancelling again is a no-op.
    let again = env.ok("run.cancel", json!({"id": id})).await;
    assert_eq!(again["status"], "cancelled");
    // A finished run can't be resumed.
    let err = env.call("run.resume", json!({"id": id})).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_and_resume() {
    let env = Env::new();
    env.save("s", "sleep 150\nresult {\"r\": \"{{id}}\"}").await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(4)}, "parallel": 1}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    env.ok("run.pause", json!({"id": id})).await;
    // The row already running finishes…
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let status = loop {
        let status = env.ok("run.status", json!({"id": id})).await;
        if status["counts"]["running"] == 0 {
            break status;
        }
        assert!(std::time::Instant::now() < deadline, "{status:#}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(status["status"], "paused");
    assert!(status["done"].as_u64().unwrap() <= 1, "{status:#}");
    // …and nothing new starts.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let later = env.ok("run.status", json!({"id": id})).await;
    assert_eq!(later["done"], status["done"]);
    assert_eq!(later["counts"]["running"], 0);

    env.ok("run.resume", json!({"id": id})).await;
    let status = env.wait_status(id, &["finished"]).await;
    assert_eq!(status["counts"]["ok"], 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn runs_survive_a_restart() {
    let env = Env::new();
    env.save("s", "sleep 300\nresult {\"r\": \"{{id}}\"}").await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(3)}, "parallel": 1}),
        )
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    tokio::time::sleep(Duration::from_millis(100)).await;
    // "Quit the browser" in the middle of row 0.
    env.jobs.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let env = env.reopen();
    let status = env.ok("run.status", json!({"id": id})).await;
    assert_eq!(status["status"], "paused", "{status:#}");
    assert!(status["note"].as_str().unwrap().contains("restart"));
    assert_eq!(status["counts"]["running"], 0);
    let list = env.ok("run.list", json!({})).await;
    assert_eq!(list["runs"][0]["id"], id.as_str());

    env.ok("run.resume", json!({"id": id})).await;
    let status = env.wait_status(&id, &["finished"]).await;
    assert_eq!(status["counts"]["ok"], 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn heal_pauses_and_patch_resumes() {
    let env = Env::new();
    let mut rx = env.events.subscribe();
    env.save(
        "login",
        "when user=b fail NOT_FOUND page.find no element matches name \"Sign in\"\n\
         result {\"user\": \"{{user}}\"}",
    )
    .await;
    let rows = json!([{"user": "a"}, {"user": "b"}, {"user": "c"}]);
    let run = env
        .ok(
            "run.start",
            json!({"script": "login", "data": {"rows": rows}, "parallel": 1, "retries": 0}),
        )
        .await;
    let id = run["id"].as_str().unwrap().to_string();

    let heal = wait_event(&mut rx, topics::RUN_HEAL_NEEDED, |d| d["id"] == id.as_str()).await;
    assert_eq!(heal.data["row"], 1);
    assert_eq!(heal.data["script"], "login");
    assert_eq!(heal.data["step"], "page.find");
    assert_eq!(heal.data["line"], 1);
    assert_eq!(heal.data["error"]["code"], "NOT_FOUND");

    let status = env.wait_status(&id, &["paused"]).await;
    assert_eq!(status["heal"]["row"], 1);
    assert_eq!(status["counts"]["needs_heal"], 1);
    assert_eq!(status["counts"]["pending"], 1);

    let applied = env
        .ok(
            "run.heal_apply",
            json!({"id": id, "patched_code": "result {\"user\": \"{{user}}\", \"healed\": true}"}),
        )
        .await;
    assert_eq!(applied["script"], "login.v2");
    assert_eq!(applied["requeued"], 1);

    let status = env.wait_status(&id, &["finished"]).await;
    assert_eq!(status["script"], "login.v2");
    assert_eq!(status["counts"]["ok"], 3);
    assert!(status.get("heal").is_none());
    let rows = env.rows(&id).await;
    assert_eq!(rows[0]["result"], json!({"user": "a"}));
    assert_eq!(rows[0]["script"], "login");
    assert_eq!(rows[1]["result"], json!({"user": "b", "healed": true}));
    assert_eq!(rows[1]["script"], "login.v2");
    assert_eq!(rows[2]["result"]["healed"], true);
    let code = env.ok("script.get", json!({"name": "login.v2"})).await;
    assert!(code["code"].as_str().unwrap().contains("healed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn heal_can_be_turned_off() {
    let env = Env::new();
    env.save(
        "s",
        "fail STALE_REF page.act ref @3:e1 is from an old snapshot",
    )
    .await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(2)}, "retries": 0, "heal": false}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    let status = env.wait_status(id, &["finished"]).await;
    assert_eq!(status["counts"]["failed"], 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn heal_modes_and_the_snapshot_hook() {
    let env = Env::new();
    env.save("s", "fail NOT_FOUND page.find nothing matches")
        .await;
    // "off" as a string works like `false`.
    let run = env
        .ok(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(1)}, "retries": 0, "heal": "off"}),
        )
        .await;
    assert_eq!(run["options"]["heal"], false);
    env.wait_status(run["id"].as_str().unwrap(), &["finished"])
        .await;

    // The hook runs before run.heal_needed; its text is stored for the row.
    let hooked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = hooked.clone();
    env.jobs.set_heal_hook(std::sync::Arc::new(move |t| {
        seen.lock().unwrap().push((t.row, t.mode));
        Box::pin(async { Some("- button \"Log in\" @1:e2".to_string()) })
    }));
    let mut rx = env.events.subscribe();
    let run = env
        .ok(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(1)}, "retries": 0, "heal": "auto"}),
        )
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    assert_eq!(run["options"]["heal_mode"], "auto");
    let heal = wait_event(&mut rx, topics::RUN_HEAL_NEEDED, |d| d["id"] == id.as_str()).await;
    assert_eq!(heal.data["mode"], "auto");
    assert_eq!(
        *hooked.lock().unwrap(),
        vec![(0, bmcp_jobs::HealMode::Auto)]
    );
    let snapshot = env.jobs.heal_snapshot(&id, Some(0)).unwrap();
    assert_eq!(snapshot.as_deref(), Some("- button \"Log in\" @1:e2"));
    assert_eq!(env.jobs.heal_snapshot(&id, None).unwrap(), snapshot);
    assert_eq!(env.jobs.heal_snapshot(&id, Some(5)).unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn results_are_paged_by_cursor() {
    let env = Env::new();
    env.save("s", "when id=3 throw nope\nresult {\"n\": \"{{id}}\"}")
        .await;
    let run = env
        .ok(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(5)}, "retries": 0}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    env.wait_status(id, &["finished"]).await;

    let mut seen = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = env
            .ok(
                "run.results",
                json!({"id": id, "limit": 2, "cursor": cursor}),
            )
            .await;
        for row in page["rows"].as_array().unwrap() {
            seen.push(row["row"].as_u64().unwrap());
        }
        cursor = page.get("next_cursor").cloned().unwrap_or(Value::Null);
        if cursor.is_null() {
            break;
        }
    }
    assert_eq!(seen, [0, 1, 2, 3, 4]);
    let failed = env
        .ok("run.results", json!({"id": id, "status": "failed"}))
        .await;
    assert_eq!(failed["rows"].as_array().unwrap().len(), 1);
    assert_eq!(failed["rows"][0]["row"], 3);
    assert_eq!(
        failed["rows"][0]["error"]["message"],
        "Uncaught Error: nope"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn csv_and_xlsx_data_reach_the_script() {
    let env = Env::new();
    env.save(
        "echo",
        "result {\"email\": \"{{email}}\", \"pw\": \"{{password}}\"}",
    )
    .await;
    let csv = env.root().join("in.csv");
    std::fs::write(&csv, "email,password\na@x.test,hunter2\nb@x.test,bad\n").unwrap();
    let run = env
        .ok(
            "run.start",
            json!({"script": "echo", "data": {"csv_path": csv}}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    env.wait_status(id, &["finished"]).await;
    let rows = env.rows(id).await;
    assert_eq!(rows[1]["result"], json!({"email": "b@x.test", "pw": "bad"}));

    let xlsx = env.root().join("in.xlsx");
    let mut book = rust_xlsxwriter::Workbook::new();
    let sheet = book.add_worksheet();
    sheet.write_string(0, 0, "email").unwrap();
    sheet.write_string(0, 1, "password").unwrap();
    sheet.write_string(1, 0, "x@x.test").unwrap();
    sheet.write_number(1, 1, 1234.0).unwrap();
    book.save(&xlsx).unwrap();
    let run = env
        .ok(
            "run.start",
            json!({"script": "echo", "data": {"xlsx_path": xlsx}}),
        )
        .await;
    let id = run["id"].as_str().unwrap();
    env.wait_status(id, &["finished"]).await;
    let rows = env.rows(id).await;
    assert_eq!(
        rows[0]["result"],
        json!({"email": "x@x.test", "pw": "1234"})
    );

    let err = env
        .call(
            "run.start",
            json!({"script": "echo", "data": {"csv_path": env.root().join("missing.csv")}}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_default_profile_for_now() {
    let env = Env::new();
    env.save("s", "out x").await;
    let err = env
        .call(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(1)}, "profile": "isolated"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotImplemented);
    let run = env
        .ok(
            "run.start",
            json!({"script": "s", "data": {"rows": id_rows(1)}, "profile": "default"}),
        )
        .await;
    assert_eq!(run["options"]["profile"], "default");
}
