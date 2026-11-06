//! run.export (all formats, webhook with retry) and cron triggers, with the fake host.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::ErrorCode;
use bmcp_jobs::topics;
use calamine::Reader;
use common::{Env, wait_event};
use serde_json::{Value, json};

/// A finished run: rows a (ok), b (failed: wrong password), c (ok).
async fn finished_run(env: &Env) -> String {
    env.save(
        "login",
        "when password=bad throw Wrong password\nresult {\"welcome\": \"Welcome {{email}}\"}",
    )
    .await;
    let rows = json!([
        {"email": "a@x.test", "password": "hunter2"},
        {"email": "b@x.test", "password": "bad"},
        {"email": "c@x.test", "password": "hunter2"},
    ]);
    let run = env
        .ok(
            "run.start",
            json!({"script": "login", "data": {"rows": rows}, "retries": 0}),
        )
        .await;
    let id = run["id"].as_str().unwrap().to_string();
    env.wait_status(&id, &["finished"]).await;
    id
}

#[tokio::test(flavor = "multi_thread")]
async fn exports_to_files() {
    let env = Env::new();
    let id = finished_run(&env).await;

    // CSV at the default path.
    let out = env.ok("run.export", json!({"id": id, "to": "csv"})).await;
    assert_eq!(out["rows"], 3);
    let path = out["path"].as_str().unwrap();
    assert!(path.ends_with("results.csv"), "{path}");
    assert!(path.contains(&id));
    let mut reader = csv::Reader::from_path(path).unwrap();
    let headers: Vec<String> = reader.headers().unwrap().iter().map(String::from).collect();
    assert_eq!(
        headers,
        [
            "row",
            "status",
            "email",
            "password",
            "welcome",
            "error_code",
            "error",
            "attempts",
            "duration_ms"
        ]
    );
    let records: Vec<csv::StringRecord> = reader.records().map(Result::unwrap).collect();
    assert_eq!(&records[0][4], "Welcome a@x.test");
    assert_eq!(&records[1][1], "failed");
    assert_eq!(&records[1][5], "SCRIPT_ERROR");
    assert_eq!(&records[1][6], "Uncaught Error: Wrong password");

    // XLSX at a relative path (inside data/runs/<id>/).
    let out = env
        .ok(
            "run.export",
            json!({"id": id, "to": "xlsx", "path": "out/report.xlsx"}),
        )
        .await;
    let path = out["path"].as_str().unwrap();
    let mut book = calamine::open_workbook_auto(path).unwrap();
    let sheet = book.worksheet_range("results").unwrap();
    assert_eq!(sheet.height(), 4);
    assert_eq!(sheet.get((0, 4)).unwrap().to_string(), "welcome");
    assert_eq!(sheet.get((3, 4)).unwrap().to_string(), "Welcome c@x.test");
    assert_eq!(sheet.get((2, 0)), Some(&calamine::Data::Float(1.0)));

    // JSON, only the ok rows, at an absolute path.
    let json_path = env.root().join("ok.json");
    let out = env
        .ok(
            "run.export",
            json!({"id": id, "to": "json", "path": json_path, "status": "ok"}),
        )
        .await;
    assert_eq!(out["rows"], 2);
    let rows: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&json_path).unwrap()).unwrap();
    assert_eq!(rows[1]["inputs"]["email"], "c@x.test");
    assert_eq!(rows[1]["result"]["welcome"], "Welcome c@x.test");

    // SQLite (twice: the second export replaces the file).
    for _ in 0..2 {
        let out = env
            .ok("run.export", json!({"id": id, "to": "sqlite"}))
            .await;
        let conn = rusqlite::Connection::open(out["path"].as_str().unwrap()).unwrap();
        let (n, welcome): (i64, String) = conn
            .query_row("SELECT COUNT(*), MAX(welcome) FROM results", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(n, 3);
        assert_eq!(welcome, "Welcome c@x.test");
    }

    // Google Sheets is a clear stub.
    let err = env
        .call("run.export", json!({"id": id, "to": "google_sheets"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotImplemented);
    assert!(err.message.contains("OAuth"));
    let err = env
        .call("run.export", json!({"id": "run-0", "to": "csv"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test(flavor = "multi_thread")]
async fn webhook_export_batches_and_retries() {
    let env = Env::new();
    let id = finished_run(&env).await;

    // A webhook that fails its first request with 500.
    let received: Arc<Mutex<Vec<Value>>> = Arc::default();
    let app = {
        let received = received.clone();
        axum::Router::new().route(
            "/hook",
            axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
                let received = received.clone();
                async move {
                    let mut all = received.lock().unwrap();
                    all.push(body);
                    if all.len() == 1 {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        axum::http::StatusCode::OK
                    }
                }
            }),
        )
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let out = env
        .ok(
            "run.export",
            json!({"id": id, "to": "webhook", "url": url, "batch_size": 2}),
        )
        .await;
    assert_eq!(out["batches"], 2);
    assert_eq!(out["rows"], 3);
    let received = received.lock().unwrap().clone();
    assert_eq!(received.len(), 3, "first batch retried once");
    assert_eq!(received[0], received[1]);
    assert_eq!(received[1]["batch"], 1);
    assert_eq!(received[1]["batches"], 2);
    assert_eq!(received[1]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(received[2]["rows"][0]["row"], 2);
    assert_eq!(received[2]["run"], id.as_str());

    let err = env
        .call("run.export", json!({"id": id, "to": "webhook"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test(flavor = "multi_thread")]
async fn cron_triggers_fire_the_script() {
    let env = Env::new();
    let mut rx = env.events.subscribe();
    env.save("tick", "result {\"k\": \"{{k}}\"}").await;

    let before = chrono::Local::now().timestamp_millis();
    let t = env
        .ok(
            "trigger.set",
            json!({"kind": "cron", "spec": "*/5 * * * * *", "script": "tick", "inputs": {"k": "v"}}),
        )
        .await;
    let after = chrono::Local::now().timestamp_millis();
    let id = t["id"].as_str().unwrap().to_string();
    let next = t["next_fire_ms"].as_i64().unwrap();
    // The next 5-second boundary after the moment of the call (however long the call took).
    assert!(
        next > before && next <= after + 5_000,
        "next {next} call between {before} and {after}"
    );
    assert!(t["next_fire"].as_str().unwrap().contains('T'));

    // Drive time by hand: nothing due in a 1 ms window that ends before the next firing…
    let base = chrono::DateTime::parse_from_rfc3339("2026-11-06T10:00:01+00:00")
        .unwrap()
        .with_timezone(&chrono::Local);
    assert!(
        env.jobs
            .tick_triggers(base, base + chrono::Duration::milliseconds(1))
            .is_empty()
    );
    // …and one firing in a window that contains 10:00:05.
    let fired = env
        .jobs
        .tick_triggers(base, base + chrono::Duration::seconds(5));
    assert_eq!(fired, std::slice::from_ref(&id));
    let event = wait_event(&mut rx, topics::TRIGGER_FIRED, |d| d["id"] == id.as_str()).await;
    assert_eq!(event.data["ok"], true);
    assert_eq!(event.data["result"], json!({"k": "v"}));

    let list = env.ok("trigger.list", json!({})).await;
    assert_eq!(list["triggers"][0]["last_status"], "ok");
    assert_eq!(list["triggers"][0]["inputs"], json!({"k": "v"}));
    assert!(list["triggers"][0]["last_fired_ms"].as_u64().is_some());

    // Replace with a 5-field spec under the same id.
    let t = env
        .ok(
            "trigger.set",
            json!({"kind": "cron", "spec": "0 9 * * Mon-Fri", "script": "tick", "id": id}),
        )
        .await;
    assert_eq!(t["spec"], "0 9 * * Mon-Fri");
    let list = env.ok("trigger.list", json!({})).await;
    assert_eq!(list["triggers"].as_array().unwrap().len(), 1);

    let removed = env.ok("trigger.remove", json!({"id": id})).await;
    assert_eq!(removed["removed"], true);
    let removed = env.ok("trigger.remove", json!({"id": id})).await;
    assert_eq!(removed["removed"], false);

    let err = env
        .call(
            "trigger.set",
            json!({"kind": "cron", "spec": "every day", "script": "tick"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = env
        .call(
            "trigger.set",
            json!({"kind": "cron", "spec": "* * * * *", "script": "missing"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = env
        .call(
            "trigger.set",
            json!({"kind": "webhook", "spec": "x", "script": "tick"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    // Triggers are persisted.
    env.ok(
        "trigger.set",
        json!({"kind": "cron", "spec": "* * * * *", "script": "tick", "id": "keep"}),
    )
    .await;
    let env = env.reopen();
    let list = env.ok("trigger.list", json!({})).await;
    assert_eq!(list["triggers"][0]["id"], "keep");
    tokio::time::sleep(Duration::from_millis(10)).await;
}
