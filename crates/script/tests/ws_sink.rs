//! WsSink against a tiny in-process fake JSON-RPC server. This checks our own
//! client side only; the real `/v1/ws` server is tested in `crates/api/tests`.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use bmcp_script::{RunOptions, RunOutcome, WsSink, run_script};
use deno_core::futures::{SinkExt, StreamExt};
use deno_core::serde_json::{self, Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

/// Answer every request with `{echo: method, params}`; "bad.cmd" gets an error.
async fn fake_browser(listener: TcpListener) {
    let (tcp, _) = listener.accept().await.expect("accept");
    let mut ws = tokio_tungstenite::accept_async(tcp)
        .await
        .expect("handshake");
    while let Some(Ok(msg)) = ws.next().await {
        let Message::Text(text) = msg else { continue };
        let req: Value = serde_json::from_str(text.as_str()).expect("json request");
        assert_eq!(req["jsonrpc"], "2.0");
        let method = req["method"].as_str().unwrap_or_default().to_string();
        let reply = if method == "bad.cmd" {
            json!({ "jsonrpc": "2.0", "id": req["id"],
                    "error": { "code": -32601, "message": "no such command" } })
        } else {
            json!({ "jsonrpc": "2.0", "id": req["id"],
                    "result": { "echo": method, "params": req["params"] } })
        };
        ws.send(Message::text(reply.to_string()))
            .await
            .expect("send");
    }
}

#[test]
fn ws_sink_round_trip_against_fake_server() {
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio");
    tokio.block_on(async {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("ws://{}", listener.local_addr().expect("addr"));
        tokio::spawn(fake_browser(listener));

        let sink = WsSink::connect(&url).await.expect("connect");
        let script =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ws_roundtrip.ts");
        let opts = RunOptions {
            script,
            inputs: Vec::new(),
            timeout: Some(Duration::from_secs(20)),
        };
        let (outcome, _timings) = run_script(opts, Rc::new(sink)).await.expect("run");
        assert!(matches!(outcome, RunOutcome::Completed), "{outcome:?}");
    });
}

#[test]
fn ws_sink_reports_unreachable_browser() {
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio");
    tokio.block_on(async {
        // Port 9 ("discard") on loopback: nothing listens there.
        let err = WsSink::connect("ws://127.0.0.1:9/v1/ws")
            .await
            .err()
            .expect("connect must fail");
        assert!(format!("{err:#}").contains("cannot connect to browser"));
    });
}
