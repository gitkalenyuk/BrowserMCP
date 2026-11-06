//! page.extract: AI plan on a cache miss, no AI on a hit, one AI rebuild when required fields
//! come back empty. The "page" is a FakeBackend answering the in-page scripts from a
//! selector → value table.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use common::{Env, FakeLlm};
use serde_json::{Value, json};

/// Selector → what `querySelectorAll` + read would give on the fake page.
type Dom = Arc<Mutex<HashMap<String, Value>>>;

/// Answer the extraction script from `dom`, the outline script with a fixed outline.
fn fake_page(env: &Env) -> Dom {
    let dom: Dom = Arc::default();
    let table = dom.clone();
    env.backend.on_cdp(move |_tab, method, params| {
        if method != "Runtime.evaluate" {
            return None;
        }
        let expr = params["expression"].as_str().unwrap_or_default();
        if expr.contains("__bmcp_outline") {
            return Some(Ok(
                json!({"result": {"value": "<h1> Phone X\n<span.price-new> $1,299.00"}}),
            ));
        }
        if !expr.contains("__bmcp_extract") {
            return None;
        }
        // The plan's JSON sits between `const plan = ` and the next `;\n`.
        let start = expr.find("const plan = ").unwrap() + "const plan = ".len();
        let end = start + expr[start..].find(";\n").unwrap();
        let fields: Vec<Value> = serde_json::from_str(&expr[start..end]).unwrap();
        let table = table.lock().unwrap();
        let mut out = serde_json::Map::new();
        for f in fields {
            if let Some(css) = f["css"].as_str() {
                let v = table.get(css).cloned().unwrap_or(Value::Null);
                out.insert(f["field"].as_str().unwrap().to_string(), v);
            }
        }
        Some(Ok(json!({"result": {"value": out}})))
    });
    dom
}

fn plan(fields: Value) -> String {
    format!(
        "Here is the plan:\n```json\n{}\n```",
        json!({ "fields": fields })
    )
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string"},
            "price": {"type": "number"},
            "account": {"type": "string"}
        },
        "required": ["title", "price"]
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn plan_is_built_once_cached_and_rebuilt_when_the_page_changes() {
    let first = plan(json!([
        {"field": "title", "css": "h1"},
        {"field": "price", "css": ".price"},
        {"field": "account", "role": "button", "name": "Sign out"}
    ]));
    let second = plan(json!([
        {"field": "title", "css": "h1"},
        {"field": "price", "css": ".price-new", "post": {"regex": "\\$([0-9,.]+)"}},
        {"field": "account", "role": "button", "name": "Sign out"}
    ]));
    let llm = FakeLlm::new(&[first, second]);
    let env = Env::new(Some(llm.clone()));
    let dom = fake_page(&env);
    dom.lock()
        .unwrap()
        .insert("h1".into(), json!("  Phone   X "));
    dom.lock()
        .unwrap()
        .insert(".price".into(), json!("$1,299.00"));
    *env.page.find.lock().unwrap() =
        "find 1: 1 of 1 matches\n- button \"Sign out\" @1:e3 in navigation".into();
    *env.page.read.lock().unwrap() = "Phone X\n$1,299.00".into();

    // 1. Miss: the AI builds the plan.
    let out = env.ok("page.extract", json!({"schema": schema()})).await;
    assert_eq!(out["from_cache"], false);
    assert_eq!(out["rebuilt"], false);
    assert_eq!(
        out["data"],
        json!({"title": "Phone X", "price": 1299.0, "account": "Sign out"})
    );
    let key = out["cache_key"].as_str().unwrap().to_string();
    assert!(key.starts_with("shop.test-"), "{key}");
    let file = env
        .tmp
        .path()
        .join("extract-cache")
        .join(format!("{key}.json"));
    let cached: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(cached["plan_id"], out["plan_id"]);
    assert_eq!(cached["provider"], "fake");
    let prompt = &llm.prompts()[0];
    for part in [
        "\"price\"",
        "- button \"Log in\" @1:e2",
        "$1,299.00",
        "<span.price-new>",
    ] {
        assert!(prompt.contains(part), "missing {part:?} in\n{prompt}");
    }

    // 2. Hit: no AI call.
    let out = env.ok("page.extract", json!({"schema": schema()})).await;
    assert_eq!(out["from_cache"], true);
    assert_eq!(out["data"]["price"], 1299.0);
    assert_eq!(llm.prompts().len(), 1);

    // 3. The page changed: the cached plan misses `price`, the AI rebuilds it once.
    {
        let mut dom = dom.lock().unwrap();
        dom.remove(".price");
        dom.insert(".price-new".into(), json!("now $999.50!"));
    }
    let old_plan = out["plan_id"].clone();
    let out = env.ok("page.extract", json!({"schema": schema()})).await;
    assert_eq!(out["from_cache"], false);
    assert_eq!(out["rebuilt"], true);
    assert_eq!(out["data"]["price"], 999.5);
    assert!(out.get("missing").is_none());
    assert_ne!(out["plan_id"], old_plan);
    let prompts = llm.prompts();
    assert_eq!(prompts.len(), 2);
    assert!(
        prompts[1].contains("missed required fields [\"price\"]"),
        "{}",
        prompts[1]
    );
    let cached: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(cached["plan_id"], out["plan_id"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_that_keeps_failing_is_reported_and_not_cached() {
    let bad = plan(json!([{"field": "title", "css": "h2"}, {"field": "price", "css": ".nope"}]));
    let llm = FakeLlm::new(&[bad.clone(), bad]);
    let env = Env::new(Some(llm.clone()));
    let dom = fake_page(&env);
    dom.lock().unwrap().insert("h2".into(), json!("Phone X"));

    let out = env
        .ok(
            "page.extract",
            json!({"schema": schema(), "cache_key": "my/shop"}),
        )
        .await;
    assert_eq!(out["cache_key"], "my_shop");
    assert_eq!(out["rebuilt"], true);
    assert_eq!(out["missing"], json!(["price"]));
    assert_eq!(out["data"]["title"], "Phone X");
    assert_eq!(llm.prompts().len(), 2);
    assert!(!env.tmp.path().join("extract-cache/my_shop.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_plans_and_schemas_are_errors() {
    let llm = FakeLlm::new(&["I think the price is $5.".into()]);
    let env = Env::new(Some(llm));
    fake_page(&env);
    let err = env
        .call("page.extract", json!({"schema": schema()}))
        .await
        .unwrap_err();
    assert!(err.message.contains("usable extraction plan"), "{err:?}");
    let err = env
        .call("page.extract", json!({"schema": {"type": "string"}}))
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
}
