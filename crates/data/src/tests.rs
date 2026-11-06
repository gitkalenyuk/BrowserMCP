//! Shared test helpers and the Chrome-import tests.
//!
//! PRIVACY: these tests never touch a real Chrome profile. Every input (Bookmarks JSON, History
//! SQLite, password CSV, cookie JSON) is a synthetic fixture written to a temp folder.

use std::path::Path;

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{Caller, CmdError, Ctx, Registry, Toolset};
use rusqlite::Connection;
use serde_json::{Value, json};

use crate::DataStore;
use crate::import::webkit_to_unix_ms;

/// A registry with the data commands and an in-memory store, plus a context that shares the
/// registry's extensions (so tests can reach the store with `DataStore::from_ctx`).
pub fn setup() -> (Registry, Ctx) {
    let mut registry = Registry::new();
    registry
        .extensions()
        .insert(DataStore::open_in_memory().unwrap());
    crate::register(&mut registry);
    let mut ctx = ctx(Caller::Rest, FakeBackend::new());
    ctx.extensions = registry.extensions().clone();
    (registry, ctx)
}

pub async fn call(registry: &Registry, ctx: &Ctx, name: &str, args: Value) -> Value {
    registry
        .call(ctx.clone(), name, args)
        .await
        .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
}

pub async fn call_err(registry: &Registry, ctx: &Ctx, name: &str, args: Value) -> CmdError {
    match registry.call(ctx.clone(), name, args).await {
        Ok(out) => panic!("{name} should fail, got {out}"),
        Err(err) => err,
    }
}

/// The `{topic, data}` of every event received so far whose topic starts with `prefix`
/// (skips the registry's own `command.*` events).
pub fn drain_events(
    rx: &mut tokio::sync::broadcast::Receiver<bmcp_core::Event>,
    prefix: &str,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if event.topic.starts_with(prefix) {
            out.push((event.topic, event.data));
        }
    }
    out
}

#[test]
fn every_command_is_in_the_data_toolset() {
    let (registry, _) = setup();
    assert_eq!(registry.len(), 17);
    for info in registry.list(None) {
        assert_eq!(info.toolset, Toolset::Data, "{}", info.name);
    }
}

#[tokio::test]
async fn commands_without_a_store_fail_cleanly() {
    let mut registry = Registry::new();
    crate::register(&mut registry);
    let err = registry
        .call(
            ctx(Caller::Rest, FakeBackend::new()),
            "data.bookmarks",
            json!({}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::Internal);
    assert!(err.message.contains("not open"));
}

// ---------------------------------------------------------------------------------------------
// Chrome import fixtures
// ---------------------------------------------------------------------------------------------

/// 2022-06-18 00:26:40 UTC in Chrome time (µs since 1601) and in Unix ms.
const WEBKIT_T1: i64 = 13_300_000_000_000_000;
const UNIX_T1: i64 = 1_655_526_400_000;

const BOOKMARKS_JSON: &str = r#"{
  "checksum": "0",
  "roots": {
    "bookmark_bar": {
      "type": "folder", "name": "Bookmarks bar",
      "children": [
        {"type": "url", "name": "Rust", "url": "https://www.rust-lang.org/", "date_added": "13300000000000000"},
        {"type": "folder", "name": "Dev", "date_added": "13300000000000000", "children": [
          {"type": "url", "name": "MDN", "url": "https://developer.mozilla.org/", "date_added": "0"}
        ]},
        {"type": "url", "name": "Broken", "url": "not a url"}
      ]
    },
    "other": {"type": "folder", "name": "Other bookmarks", "children": [
      {"type": "url", "name": "Other one", "url": "https://other.test/"}
    ]},
    "synced": {"type": "folder", "name": "Mobile bookmarks", "children": [
      {"type": "url", "name": "Phone", "url": "https://m.test/"}
    ]}
  },
  "version": 1
}"#;

/// A tiny `History` database with Chrome's column names.
fn write_history_fixture(path: &Path) {
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE urls (id INTEGER PRIMARY KEY AUTOINCREMENT, url LONGVARCHAR, title LONGVARCHAR,
             visit_count INTEGER DEFAULT 0 NOT NULL, typed_count INTEGER DEFAULT 0 NOT NULL,
             last_visit_time INTEGER NOT NULL, hidden INTEGER DEFAULT 0 NOT NULL);
         CREATE TABLE visits (id INTEGER PRIMARY KEY, url INTEGER NOT NULL, visit_time INTEGER NOT NULL,
             from_visit INTEGER, transition INTEGER DEFAULT 0 NOT NULL, segment_id INTEGER,
             visit_duration INTEGER DEFAULT 0 NOT NULL);",
    )
    .unwrap();
    let t2 = WEBKIT_T1 + 60_000_000; // one minute later
    db.execute_batch(&format!(
        "INSERT INTO urls (id, url, title, visit_count, last_visit_time) VALUES
             (1, 'https://a.test/', 'Page A', 2, {t2}),
             (2, 'https://frame.test/ad', '', 1, {t2}),
             (3, 'chrome://settings/', 'Settings', 1, {t2});
         INSERT INTO visits (url, visit_time, transition) VALUES
             (1, {WEBKIT_T1}, 805306369), -- TYPED | CHAIN_START | CHAIN_END
             (1, {t2}, 0),                -- LINK
             (2, {t2}, 3),                -- AUTO_SUBFRAME: skipped
             (3, {t2}, 1);                -- chrome:// page: skipped"
    ))
    .unwrap();
}

const PASSWORDS_CSV: &str = "name,url,username,password,note\n\
github.com,https://github.com/session,me,\"pa,ss\"\"word\",\n\
example.org,https://example.org/login,,secret,a note\n\
bad,,x,y,\n\
nopass,https://nopass.test/,u,,\n";

const COOKIES_JSON: &str = r#"[
  {"domain": ".github.com", "name": "logged_in", "value": "yes", "path": "/",
   "expirationDate": 1893456000.5, "secure": true, "httpOnly": true, "sameSite": "lax",
   "hostOnly": false, "session": false, "storeId": "0"},
  {"domain": "example.org", "name": "sid", "value": "abc", "session": true,
   "sameSite": "no_restriction"},
  {"name": "broken"}
]"#;

/// `<tmp>/profile/{Bookmarks,History}`, `<tmp>/passwords.csv`, `<tmp>/cookies.json`.
fn write_fixtures(root: &Path) {
    let profile = root.join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("Bookmarks"), BOOKMARKS_JSON).unwrap();
    write_history_fixture(&profile.join("History"));
    std::fs::write(root.join("passwords.csv"), PASSWORDS_CSV).unwrap();
    std::fs::write(root.join("cookies.json"), COOKIES_JSON).unwrap();
}

fn import_args(root: &Path) -> Value {
    json!({
        "parts": ["bookmarks", "history", "passwords_csv", "cookies_json"],
        "profile_dir": root.join("profile"),
        "passwords_csv_path": root.join("passwords.csv"),
        "cookies_json_path": root.join("cookies.json"),
    })
}

#[test]
fn webkit_timestamps_convert_to_unix_ms() {
    assert_eq!(webkit_to_unix_ms(WEBKIT_T1), UNIX_T1);
    // The Unix epoch itself.
    assert_eq!(webkit_to_unix_ms(11_644_473_600_000_000), 0);
    assert_eq!(webkit_to_unix_ms(0), 0);
}

#[tokio::test]
async fn import_chrome_from_fixtures() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixtures(tmp.path());
    let (registry, ctx) = setup();
    let mut rx = ctx.events.subscribe();

    let out = call(&registry, &ctx, "import.chrome", import_args(tmp.path())).await;
    // The UI is told to re-read bookmarks and history.
    let events: Vec<(String, Value)> = drain_events(&mut rx, "")
        .into_iter()
        .filter(|(topic, _)| !topic.starts_with("command."))
        .collect();
    assert_eq!(
        events,
        [
            (
                "bookmarks.changed".to_string(),
                json!({"op": "import", "id": null})
            ),
            ("history.changed".to_string(), json!({"op": "import"})),
        ]
    );
    let counts: Vec<(Value, Value, Value, Value)> = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["part"].clone(),
                r["ok"].clone(),
                r["imported"].clone(),
                r["skipped"].clone(),
            )
        })
        .collect();
    assert_eq!(
        counts,
        [
            (json!("bookmarks"), json!(true), json!(4), json!(1)),
            (json!("history"), json!(true), json!(2), json!(2)),
            (json!("passwords_csv"), json!(true), json!(2), json!(2)),
            (json!("cookies_json"), json!(true), json!(2), json!(1)),
        ],
        "{out:#}"
    );

    // Bookmarks: bar keeps Chrome's order and dates; folders are recreated.
    let bar = call(&registry, &ctx, "data.bookmarks", json!({})).await;
    assert_eq!(bar["bookmarks"][0]["title"], "Rust");
    assert_eq!(bar["bookmarks"][0]["created"], UNIX_T1);
    assert_eq!(bar["folders"][0]["title"], "Dev");
    let dev = bar["folders"][0]["id"].clone();
    let inside = call(&registry, &ctx, "data.bookmarks", json!({"folder": dev})).await;
    assert_eq!(
        inside["bookmarks"][0]["url"],
        "https://developer.mozilla.org/"
    );
    let other = call(&registry, &ctx, "data.bookmarks", json!({"folder": 2})).await;
    assert_eq!(other["bookmarks"][0]["title"], "Other one");
    assert_eq!(other["folders"][0]["title"], "Mobile bookmarks");

    // History: two visits of a.test with converted times; subframes and chrome:// skipped.
    let hist = call(&registry, &ctx, "history.search", json!({})).await;
    assert_eq!(
        hist["entries"],
        json!([{"url": "https://a.test/", "title": "Page A", "visit_count": 2, "last_visit": UNIX_T1 + 60_000}])
    );

    // Passwords: CSV quoting survives; not revealed by default.
    let pw = call(
        &registry,
        &ctx,
        "passwords.get",
        json!({"origin": "https://github.com"}),
    )
    .await;
    assert_eq!(pw["entries"][0]["username"], "me");
    assert_eq!(pw["entries"][0]["password"], "pa,ss\"word");
    let pw = call(
        &registry,
        &ctx,
        "passwords.list",
        json!({"origin": "example.org"}),
    )
    .await;
    assert_eq!(pw["entries"][0]["note"], "a note");
    assert!(pw["entries"][0]["password"].is_null());

    // Cookies wait in pending_cookies for the CEF step.
    let store = DataStore::from_ctx(&ctx).unwrap();
    let cookies = store
        .with(|c| crate::import::list_pending_cookies(c))
        .unwrap();
    assert_eq!(cookies.len(), 2);
    assert_eq!(cookies[0].domain, ".github.com");
    assert_eq!(cookies[0].expires, Some(1_893_456_000_500));
    assert!(cookies[0].secure && cookies[0].http_only);
    assert_eq!(cookies[0].same_site, "lax");
    assert_eq!(cookies[1].expires, None, "session cookie");
    assert_eq!(cookies[1].same_site, "no_restriction");

    // Importing again adds nothing.
    let out = call(&registry, &ctx, "import.chrome", import_args(tmp.path())).await;
    let imported: Vec<Value> = out["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["imported"].clone())
        .collect();
    // Passwords and cookies are upserts (they count as imported again but do not duplicate).
    assert_eq!(imported, [json!(0), json!(0), json!(2), json!(2)]);
    let bar = call(&registry, &ctx, "data.bookmarks", json!({})).await;
    assert_eq!(bar["bookmarks"].as_array().unwrap().len(), 1);
    assert_eq!(bar["folders"].as_array().unwrap().len(), 1);
    let hist = call(&registry, &ctx, "history.search", json!({})).await;
    assert_eq!(hist["entries"][0]["visit_count"], 2);
    let pw = call(&registry, &ctx, "passwords.list", json!({})).await;
    assert_eq!(pw["entries"].as_array().unwrap().len(), 2);
    assert_eq!(
        store
            .with(|c| crate::import::list_pending_cookies(c))
            .unwrap()
            .len(),
        2
    );

    // Applied cookies can be removed by id.
    let ids: Vec<i64> = cookies.iter().map(|c| c.id).collect();
    assert_eq!(
        store
            .with(|c| crate::import::delete_pending_cookies(c, &ids))
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn import_reports_each_part_separately() {
    let tmp = tempfile::tempdir().unwrap();
    write_fixtures(tmp.path());
    let (registry, ctx) = setup();
    let empty_profile = tmp.path().join("empty");
    std::fs::create_dir_all(&empty_profile).unwrap();

    let out = call(
        &registry,
        &ctx,
        "import.chrome",
        json!({
            "parts": ["bookmarks", "history", "cookies_json"],
            "profile_dir": empty_profile,
            "cookies_json_path": tmp.path().join("cookies.json"),
        }),
    )
    .await;
    let results = out["results"].as_array().unwrap();
    assert_eq!(results[0]["ok"], false);
    assert!(
        results[0]["error"]
            .as_str()
            .unwrap()
            .contains("cannot read")
    );
    assert_eq!(results[1]["ok"], false);
    assert!(results[1]["error"].as_str().unwrap().contains("not found"));
    assert_eq!(
        results[2]["ok"], true,
        "a failed part does not stop the others"
    );

    let err = call_err(
        &registry,
        &ctx,
        "import.chrome",
        json!({"parts": ["passwords_csv"]}),
    )
    .await;
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
    let err = call_err(&registry, &ctx, "import.chrome", json!({"parts": []})).await;
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
    let err = call_err(
        &registry,
        &ctx,
        "import.chrome",
        json!({"parts": ["cookies"]}),
    )
    .await;
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
}

#[tokio::test]
async fn bad_import_files_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let profile = tmp.path().join("p");
    std::fs::create_dir_all(&profile).unwrap();
    std::fs::write(profile.join("Bookmarks"), "{not json").unwrap();
    std::fs::write(tmp.path().join("pw.csv"), "a,b\n1,2\n").unwrap();
    std::fs::write(tmp.path().join("c.json"), "{\"not\": \"an array\"}").unwrap();
    let (registry, ctx) = setup();
    let out = call(
        &registry,
        &ctx,
        "import.chrome",
        json!({
            "parts": ["bookmarks", "passwords_csv", "cookies_json"],
            "profile_dir": profile,
            "passwords_csv_path": tmp.path().join("pw.csv"),
            "cookies_json_path": tmp.path().join("c.json"),
        }),
    )
    .await;
    for result in out["results"].as_array().unwrap() {
        assert_eq!(result["ok"], false, "{result}");
    }
}

#[tokio::test]
async fn detect_lists_profile_folders_only() {
    let tmp = tempfile::tempdir().unwrap();
    for dir in [
        "Default",
        "Profile 10",
        "Profile 2",
        "System Profile",
        "Guest Profile",
        "Crashpad",
    ] {
        std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
    }
    std::fs::write(tmp.path().join("Local State"), "{}").unwrap();
    std::fs::write(tmp.path().join("Default").join("Bookmarks"), "{}").unwrap();
    let (registry, ctx) = setup();
    let out = call(
        &registry,
        &ctx,
        "import.chrome_detect",
        json!({"user_data_dir": tmp.path()}),
    )
    .await;
    let names: Vec<_> = out["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].clone())
        .collect();
    assert_eq!(
        names,
        [json!("Default"), json!("Profile 2"), json!("Profile 10")]
    );
    assert_eq!(out["profiles"][0]["has_bookmarks"], true);
    assert_eq!(out["profiles"][0]["has_history"], false);

    let out = call(
        &registry,
        &ctx,
        "import.chrome_detect",
        json!({"user_data_dir": tmp.path().join("missing")}),
    )
    .await;
    assert_eq!(out["profiles"], json!([]));
}
