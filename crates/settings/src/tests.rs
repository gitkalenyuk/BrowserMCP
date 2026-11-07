//! The commands through the registry, with a fake browser and a fake cookie store.

use std::sync::{Arc, Mutex};

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{BackendResult, BoxFuture, Caller, Ctx, DataDir, ErrorCode, Registry};
use bmcp_profiles::{
    CookieInfo, CookieTarget, OpenProfile, ProfileBackend, ProfileSettings, ProfilesService,
};
use serde_json::{Value, json};

use super::*;

struct Setup {
    registry: Registry,
    /// One bus for every call, so a test can watch the events.
    events: bmcp_core::EventBus,
    fake: Arc<FakeBackend>,
    cookies: Arc<FakeCookies>,
    tmp: tempfile::TempDir,
}

impl Setup {
    fn new(urls: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let data = DataDir::from_root(tmp.path().join("data"));
        data.ensure().unwrap();
        let mut registry = Registry::new();
        bmcp_core::commands::register_builtin(&mut registry);
        register(&mut registry);
        bmcp_data::register(&mut registry);
        registry.extensions().insert(data.clone());
        registry.extensions().insert(BrowserSettings::load(&data));
        registry
            .extensions()
            .insert(bmcp_data::store::DataStore::open_in_memory().unwrap());
        let profiles = registry
            .extensions()
            .insert(ProfilesService::new(data.root(), "default"));
        let cookies = Arc::new(FakeCookies::default());
        profiles.set_backend(cookies.clone());
        let fake = FakeBackend::with_tabs(urls);
        // Every CDP call succeeds (the clear commands only need an answer).
        fake.on_cdp(|_, _, _| Some(Ok(json!({}))));
        Setup {
            registry,
            events: bmcp_core::EventBus::new(),
            fake,
            cookies,
            tmp,
        }
    }

    fn ctx(&self) -> Ctx {
        let mut ctx = ctx(Caller::Rest, self.fake.clone());
        ctx.extensions = self.registry.extensions().clone();
        ctx.events = self.events.clone();
        ctx
    }

    async fn call(&self, name: &str, args: Value) -> Value {
        self.registry
            .call(self.ctx(), name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    async fn call_err(&self, name: &str, args: Value) -> bmcp_core::CmdError {
        self.registry
            .call(self.ctx(), name, args)
            .await
            .unwrap_err()
    }

    fn config_file(&self, name: &str) -> Value {
        let text = std::fs::read_to_string(self.tmp.path().join("data").join("config").join(name))
            .unwrap();
        serde_json::from_str(&text).unwrap()
    }
}

/// Cookie store: what was deleted, for which profile and domain.
#[derive(Default)]
struct FakeCookies {
    cookies: Mutex<Vec<CookieInfo>>,
    deleted: Mutex<Vec<(Option<String>, Option<String>)>>,
}

impl ProfileBackend for FakeCookies {
    fn configure(&self, _settings: ProfileSettings) -> BoxFuture<'_, BackendResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn forget(&self, _id: String) -> BoxFuture<'_, BackendResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn open_profiles(&self) -> BoxFuture<'_, BackendResult<Vec<OpenProfile>>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn cookies(
        &self,
        _target: CookieTarget,
        _domain: Option<String>,
    ) -> BoxFuture<'_, BackendResult<Vec<CookieInfo>>> {
        let cookies = self.cookies.lock().unwrap().clone();
        Box::pin(async move { Ok(cookies) })
    }
    fn set_cookies(
        &self,
        _target: CookieTarget,
        cookies: Vec<CookieInfo>,
    ) -> BoxFuture<'_, BackendResult<Vec<bool>>> {
        Box::pin(async move { Ok(vec![true; cookies.len()]) })
    }
    fn delete_cookies(
        &self,
        target: CookieTarget,
        domain: Option<String>,
        _name: Option<String>,
    ) -> BoxFuture<'_, BackendResult<usize>> {
        self.deleted
            .lock()
            .unwrap()
            .push((target.profile, domain.clone()));
        let mut cookies = self.cookies.lock().unwrap();
        let before = cookies.len();
        cookies.retain(|c| {
            domain
                .as_deref()
                .is_some_and(|d| c.domain.trim_start_matches('.') != d)
        });
        let n = before - cookies.len();
        Box::pin(async move { Ok(n) })
    }
}

fn cookie(domain: &str) -> CookieInfo {
    CookieInfo {
        name: "sid".into(),
        value: "1".into(),
        domain: domain.into(),
        path: "/".into(),
        secure: true,
        http_only: false,
        expires: None,
        same_site: "lax".into(),
    }
}

#[tokio::test]
async fn startup_config_reads_saves_and_announces() {
    let s = Setup::new(&[]);
    let mut events = s.events.subscribe();
    let out = s.call("startup.config", json!({})).await;
    assert_eq!(out["config"]["on_startup"], "restore");
    assert_eq!(out["new_tab_page"], "bmcp://ui/newtab.html");

    let out = s
        .call(
            "startup.config",
            json!({ "on_startup": "pages", "pages": ["a.test"], "new_tab_position": "after_active",
                    "show_home_button": true, "home_page": "https://home.test" }),
        )
        .await;
    assert_eq!(out["config"]["pages"], json!(["https://a.test/"]));
    assert_eq!(
        s.config_file("startup.json")["new_tab_position"],
        "after_active"
    );
    let event = std::iter::from_fn(|| events.try_recv().ok())
        .find(|e| e.topic == SETTINGS_CHANGED)
        .expect("settings.changed");
    assert_eq!(event.data["area"], "startup");

    let err = s.call_err("startup.config", json!({ "pages": [] })).await;
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let settings = s.registry.extensions().get::<BrowserSettings>().unwrap();
    assert_eq!(
        settings.startup().pages,
        vec!["https://a.test/"],
        "a refused change keeps the old value"
    );
    assert_eq!(
        settings.startup().new_tab_position,
        NewTabPosition::AfterActive
    );
}

#[tokio::test]
async fn search_engines_end_to_end() {
    let s = Setup::new(&[]);
    let out = s.call("search.config", json!({})).await;
    assert_eq!(out["default"], "google");
    assert_eq!(out["engines"].as_array().unwrap().len(), 5);

    let out = s
        .call(
            "search.config",
            json!({ "add": { "name": "Docs", "url": "https://docs.test/?q=%s", "keyword": "dx" } }),
        )
        .await;
    let id = out["added"].as_str().unwrap().to_string();
    s.call("search.config", json!({ "default": id })).await;
    assert_eq!(s.config_file("search.json")["default"], "custom-docs");

    let out = s
        .call("search.url", json!({ "query": "hello world" }))
        .await;
    assert_eq!(
        out,
        json!({ "url": "https://docs.test/?q=hello%20world", "engine": "custom-docs" })
    );
    let out = s
        .call("search.url", json!({ "query": "@bing hello" }))
        .await;
    assert_eq!(out["engine"], "bing");
    let out = s
        .call("search.url", json!({ "query": "x", "engine": "brave" }))
        .await;
    assert_eq!(out["url"], "https://search.brave.com/search?q=x");

    let out = s
        .call("search.config", json!({ "remove": "custom-docs" }))
        .await;
    assert_eq!(out["default"], "google");
    let err = s
        .call_err("search.config", json!({ "remove": "google" }))
        .await;
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn downloads_folder_and_ask() {
    let s = Setup::new(&[]);
    let out = s.call("downloads.config", json!({})).await;
    assert_eq!(out["custom"], false);
    assert!(out["dir"].as_str().unwrap().ends_with("downloads"));

    let chosen = s.tmp.path().join("My Downloads");
    let out = s
        .call(
            "downloads.config",
            json!({ "dir": chosen.display().to_string(), "ask": true }),
        )
        .await;
    assert_eq!(out["custom"], true);
    assert_eq!(out["ask"], true);
    assert!(chosen.is_dir());
    let settings = s.registry.extensions().get::<BrowserSettings>().unwrap();
    assert_eq!(settings.download_dir(None), chosen);
    assert!(
        settings
            .download_dir(Some("r1"))
            .ends_with("runs\\r1\\downloads")
            || settings
                .download_dir(Some("r1"))
                .ends_with("runs/r1/downloads")
    );

    let out = s.call("downloads.config", json!({ "dir": "" })).await;
    assert_eq!(out["custom"], false);
    let err = s
        .call_err("downloads.config", json!({ "dir": "relative" }))
        .await;
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn zoom_list_and_reset() {
    let s = Setup::new(&["https://a.test/", "https://b.test/"]);
    s.call("page.zoom", json!({ "tab": 1, "level": 150 })).await;
    s.call("page.zoom", json!({ "tab": 2, "level": 80 })).await;
    let out = s.call("zoom.list", json!({})).await;
    assert_eq!(
        out["sites"],
        json!([{ "host": "a.test", "percent": 150.0 }, { "host": "b.test", "percent": 80.0 }])
    );
    let out = s.call("zoom.reset", json!({ "host": "A.test" })).await;
    assert_eq!(out["reset"], json!(["a.test"]));
    assert_eq!(s.fake.tabs()[0].zoom, None, "the open tab is back at 100 %");
    let out = s.call("zoom.reset", json!({})).await;
    assert_eq!(out["reset"], json!(["b.test"]));
    assert_eq!(s.call("zoom.list", json!({})).await["sites"], json!([]));
}

#[tokio::test]
async fn clear_all_time_uses_the_cookie_store_and_cdp() {
    let s = Setup::new(&["https://a.test/"]);
    *s.cookies.cookies.lock().unwrap() = vec![cookie(".a.test"), cookie("b.test")];
    let store = s
        .registry
        .extensions()
        .get::<bmcp_data::store::DataStore>()
        .unwrap();
    store
        .with(|c| {
            bmcp_data::history::record_visit(c, "https://v.test/page", "", "link", 1_000)
                .map(|_| ())
        })
        .unwrap();

    let out = s
        .call(
            "browsing_data.clear",
            json!({ "history": true, "downloads": true, "cookies": true, "cache": true, "site_data": true }),
        )
        .await;
    assert_eq!(out["profile"], "default");
    assert_eq!(out["history_visits"], 1);
    assert_eq!(out["downloads"], 0);
    assert_eq!(out["cookies"], 2);
    assert_eq!(out["cache"], true);
    assert_eq!(out["errors"], json!([]));
    // The visited site plus both cookie sites, each over https and http.
    assert_eq!(out["site_data_origins"], 5);
    assert_eq!(
        *s.cookies.deleted.lock().unwrap(),
        vec![(Some("default".to_string()), None)]
    );

    let calls = s.fake.cdp_calls();
    assert_eq!(calls[0].0, "Network.clearBrowserCache");
    let origins: Vec<&str> = calls[1..]
        .iter()
        .map(|(_, p)| p["origin"].as_str().unwrap())
        .collect();
    assert!(origins.contains(&"https://v.test"), "{origins:?}");
    assert!(origins.contains(&"http://a.test"), "{origins:?}");
    assert_eq!(calls[1].1["storageTypes"], clear::STORAGE_TYPES);
    // An open tab of the profile was used: nothing extra was opened.
    assert_eq!(s.fake.tabs().len(), 1);
}

#[tokio::test]
async fn clear_a_time_range_only_touches_sites_visited_in_it() {
    let s = Setup::new(&[]);
    *s.cookies.cookies.lock().unwrap() = vec![cookie("old.test"), cookie("new.test")];
    let store = s
        .registry
        .extensions()
        .get::<bmcp_data::store::DataStore>()
        .unwrap();
    store
        .with(|c| {
            bmcp_data::history::record_visit(c, "https://old.test/", "", "link", 1_000)?;
            bmcp_data::history::record_visit(c, "https://new.test/", "", "link", 9_000)?;
            Ok(())
        })
        .unwrap();
    let out = s
        .call(
            "browsing_data.clear",
            json!({ "since": 5_000, "history": true, "cookies": true, "site_data": true }),
        )
        .await;
    assert_eq!(out["history_visits"], 1);
    assert_eq!(out["cookies"], 1);
    assert_eq!(out["site_data_origins"], 1);
    assert_eq!(s.cookies.cookies.lock().unwrap()[0].domain, "old.test");
    // No tab was open: a temporary off-screen one did the CDP work and is gone again.
    assert!(s.fake.tabs().is_empty());
    assert!(
        s.fake.calls().iter().any(|c| c.starts_with("close_tab")),
        "{:?}",
        s.fake.calls()
    );

    let err = s.call_err("browsing_data.clear", json!({})).await;
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn usage_and_about() {
    let s = Setup::new(&[]);
    std::fs::write(
        s.tmp.path().join("data").join("db").join("x.sqlite"),
        vec![0u8; 64],
    )
    .unwrap();
    let out = s.call("browsing_data.usage", json!({})).await;
    assert_eq!(out["total_bytes"], 64);
    assert_eq!(out["areas"][0]["id"], "db");

    let exe_dir = s.tmp.path().to_path_buf();
    std::fs::write(exe_dir.join("CREDITS.html"), "x").unwrap();
    s.registry.extensions().insert(AppInfo {
        cef_version: "156.0.15+g1+chromium-156.0.8078.80".into(),
        chromium_version: "156.0.8078.80".into(),
        exe_dir,
        api_port: 47800,
        mcp_url: "http://127.0.0.1:47800/mcp".into(),
        ..Default::default()
    });
    let out = s.call("system.about", json!({})).await;
    assert_eq!(out["name"], "BrowserMCP");
    assert_eq!(out["chromium_version"], "156.0.8078.80");
    assert_eq!(out["api"]["port"], 47800);
    assert_eq!(out["licenses"][0]["name"], "CREDITS.html");
    assert!(out["logs_dir"].as_str().unwrap().ends_with("logs"));
}
