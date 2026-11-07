//! Built-in commands. One file per group (`tabs.rs` holds `tabs.*`, …).
//!
//! Adding a command: write a unit struct + `impl Command` in the right file, then add one line
//! to [`register_builtin`]. Every API surface picks it up from the registry automatically.

pub mod agents;
pub mod nav;
pub mod page;
pub mod system;
pub mod tabs;
pub mod ui;

use crate::command::Registry;

/// Register every built-in command, and install [`crate::agents::Agents`] as the registry's
/// lock policy (agent identity, tab groups, locks, human takeover).
pub fn register_builtin(registry: &mut Registry) {
    let agents = registry.extensions().insert(crate::agents::Agents::new());
    registry.set_lock_policy(agents);
    registry
        .add::<agents::List>()
        .add::<agents::Forget>()
        .add::<agents::Lock>()
        .add::<agents::Unlock>()
        .add::<agents::Groups>()
        .add::<agents::Handoff>();
    registry
        .add::<system::Ping>()
        .add::<system::Quit>()
        .add::<tabs::List>()
        .add::<tabs::Open>()
        .add::<tabs::Close>()
        .add::<tabs::Activate>()
        .add::<tabs::Split>()
        .add::<tabs::Unsplit>()
        .add::<tabs::RestoreClosed>()
        .add::<nav::Goto>()
        .add::<nav::Stop>()
        .add::<page::Text>()
        .add::<page::Zoom>()
        .add::<ui::Layout>();
    // page.snapshot / page.find / page.act, nav.back/forward/reload/wait ... live in the
    // `bmcp-browser` crate.
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::backend::{BackendError, TabId};
    use crate::command::{Caller, ErrorCode};
    use crate::testing::{FakeBackend, ctx};

    fn registry() -> Registry {
        let mut registry = Registry::new();
        register_builtin(&mut registry);
        registry
    }

    async fn call(fake: &std::sync::Arc<FakeBackend>, name: &str, args: Value) -> Value {
        registry()
            .call(ctx(Caller::Rest, fake.clone()), name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    #[tokio::test]
    async fn ping_returns_version() {
        let out = call(&FakeBackend::new(), "system_ping", json!({})).await;
        assert_eq!(out["name"], "BrowserMCP");
        assert_eq!(out["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn quit_answers_then_quits() {
        let fake = FakeBackend::new();
        let out = call(&fake, "system.quit", json!({})).await;
        assert_eq!(out, json!({ "quitting": true }));
        // The backend is asked after a short delay (the answer goes out first).
        assert!(!fake.calls().contains(&"quit".to_string()));
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
        assert!(fake.calls().contains(&"quit".to_string()));
    }

    /// What the fake reports for a fresh tab, as JSON (camelCase, like the UI expects).
    fn tab_json(id: i32, url: &str, active: bool) -> Value {
        json!({
            "id": id, "title": url, "url": url, "loading": false, "favicon": null,
            "canGoBack": false, "canGoForward": false, "active": active,
            "groupId": null, "lockedBy": null, "lock": null,
        })
    }

    #[tokio::test]
    async fn tabs_list_and_open() {
        let fake = FakeBackend::with_tabs(&["https://a.test/"]);
        let out = call(&fake, "tabs.open", json!({ "url": "https://b.test/" })).await;
        // Old shape `{tab}` plus the whole TabInfo.
        let mut expected = tab_json(2, "https://b.test/", true);
        expected["tab"] = json!(2);
        expected.as_object_mut().unwrap().remove("lock");
        assert_eq!(out, expected);

        let out = call(&fake, "tabs.list", Value::Null).await;
        assert_eq!(
            out,
            json!({
                "tabs": [
                    tab_json(1, "https://a.test/", false),
                    tab_json(2, "https://b.test/", true),
                ],
                "groups": [],
                "activeId": 2,
            })
        );
    }

    #[tokio::test]
    async fn tabs_open_defaults_and_background() {
        let fake = FakeBackend::with_tabs(&["https://a.test/"]);
        let out = call(&fake, "tabs.open", json!({})).await;
        assert_eq!(out["url"], crate::NEW_TAB_URL);
        let out = call(
            &fake,
            "tabs.open",
            json!({ "url": "https://c.test/", "background": true }),
        )
        .await;
        assert_eq!(out["active"], false);
        // The new tab page opened first is still the one shown.
        let list = call(&fake, "tabs.list", json!({})).await;
        assert_eq!(list["activeId"], 2);
        assert!(
            fake.calls()
                .contains(&"open_tab https://c.test/ background".to_string())
        );
    }

    #[tokio::test]
    async fn tabs_open_uses_the_profile_incognito_default() {
        struct Private;
        impl tabs::TabDefaults for Private {
            fn incognito_default(&self, profile: &str) -> bool {
                profile == "private"
            }
        }
        let fake = FakeBackend::new();
        let registry = registry();
        registry
            .extensions()
            .insert(tabs::ProfileTabDefaults(std::sync::Arc::new(Private)));
        let open = |args: Value| {
            let registry = &registry;
            let fake = fake.clone();
            async move {
                registry
                    .call(ctx(Caller::Rest, fake), "tabs.open", args)
                    .await
                    .unwrap();
            }
        };
        open(json!({ "url": "https://a.test/", "profile": "private" })).await;
        open(json!({ "url": "https://b.test/", "profile": "private", "incognito": false })).await;
        open(json!({ "url": "https://c.test/", "profile": "work" })).await;
        let calls = fake.calls();
        let call = |url: &str| {
            calls
                .iter()
                .find(|c| c.contains(url))
                .cloned()
                .unwrap_or_default()
        };
        assert!(
            call("https://a.test/").contains("incognito: true"),
            "{calls:?}"
        );
        assert!(
            call("https://b.test/").contains("incognito: false"),
            "{calls:?}"
        );
        assert!(
            call("https://c.test/").contains("incognito: false"),
            "{calls:?}"
        );
    }

    #[tokio::test]
    async fn tabs_activate_close_and_stop() {
        let fake =
            FakeBackend::with_tabs(&["https://a.test/", "https://b.test/", "https://c.test/"]);
        call(&fake, "tabs.activate", json!({ "tab": 2 })).await;
        assert_eq!(call(&fake, "tabs.list", json!({})).await["activeId"], 2);

        // Closing the active tab activates its right neighbour.
        call(&fake, "tabs.close", json!({ "tab": 2 })).await;
        let list = call(&fake, "tabs.list", json!({})).await;
        assert_eq!(list["activeId"], 3);
        assert_eq!(list["tabs"].as_array().unwrap().len(), 2);

        let out = call(&fake, "nav.stop", json!({})).await;
        assert_eq!(out, json!({}));
        assert!(fake.calls().contains(&"stop_loading 3".to_string()));

        let err = registry()
            .call(
                ctx(Caller::Rest, fake.clone()),
                "tabs.close",
                json!({ "tab": 9 }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::TabNotFound);
    }

    #[tokio::test]
    async fn split_unsplit_and_errors() {
        let fake = FakeBackend::with_tabs(&["https://a.test/", "https://b.test/"]);
        let out = call(
            &fake,
            "tabs.split",
            json!({ "left": 1, "right": 2, "ratio": 0.95 }),
        )
        .await;
        assert_eq!(out, json!({ "left": 1, "right": 2, "ratio": 0.85 }));
        let list = call(&fake, "tabs.list", json!({})).await;
        assert_eq!(
            list["tabs"][0]["split"],
            json!({ "side": "left", "other": 2, "ratio": 0.85 })
        );
        assert_eq!(list["tabs"][1]["split"]["side"], "right");
        let out = call(&fake, "tabs.unsplit", json!({})).await;
        assert_eq!(out, json!({ "tabs": [1, 2] }));
        assert!(
            call(&fake, "tabs.list", json!({})).await["tabs"][0]
                .get("split")
                .is_none()
        );
        // Same tab twice, unknown tab.
        let err = registry()
            .call(
                ctx(Caller::Rest, fake.clone()),
                "tabs.split",
                json!({ "left": 1, "right": 1 }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
        let err = registry()
            .call(
                ctx(Caller::Rest, fake.clone()),
                "tabs.split",
                json!({ "left": 1, "right": 9 }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::TabNotFound);
    }

    #[tokio::test]
    async fn restore_closed_reopens_the_last_one() {
        let fake = FakeBackend::with_tabs(&["https://a.test/", "https://b.test/"]);
        call(&fake, "tabs.close", json!({ "tab": 2 })).await;
        let out = call(&fake, "tabs.restore_closed", json!({})).await;
        assert_eq!(out, json!({ "tab": 3 }));
        let list = call(&fake, "tabs.list", json!({})).await;
        assert_eq!(list["tabs"][1]["url"], "https://b.test/");
        // Nothing left.
        let out = call(&fake, "tabs.restore_closed", json!({})).await;
        assert_eq!(out, json!({ "tab": null }));
    }

    #[tokio::test]
    async fn page_zoom_levels_and_steps() {
        let fake = FakeBackend::with_tabs(&["https://a.test/"]);
        let out = call(&fake, "page.zoom", json!({ "tab": 1 })).await;
        assert_eq!(out, json!({ "tab": 1, "level": 100.0 }));
        let out = call(&fake, "page.zoom", json!({ "tab": 1, "delta": 1 })).await;
        assert_eq!(out["level"], 110.0);
        let out = call(&fake, "page.zoom", json!({ "tab": 1, "level": 150 })).await;
        assert_eq!(out["level"], 150.0);
        assert_eq!(
            call(&fake, "tabs.list", json!({})).await["tabs"][0]["zoom"],
            150
        );
        let out = call(&fake, "page.zoom", json!({ "tab": 1, "delta": 0 })).await;
        assert_eq!(out["level"], 100.0);
        assert!(
            call(&fake, "tabs.list", json!({})).await["tabs"][0]
                .get("zoom")
                .is_none()
        );
        let err = registry()
            .call(
                ctx(Caller::Rest, fake.clone()),
                "page.zoom",
                json!({ "tab": 1, "level": 5 }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn ui_layout_is_accepted() {
        let out = call(
            &FakeBackend::new(),
            "ui.layout",
            json!({ "x": 0, "y": 84, "w": 1280, "h": 716, "dpr": 1.5 }),
        )
        .await;
        assert_eq!(out, json!({}));
    }

    #[tokio::test]
    async fn goto_defaults_to_highest_tab_id() {
        let fake = FakeBackend::with_tabs(&["https://a.test/", "https://b.test/"]);
        let out = call(&fake, "nav_goto", json!({ "url": "https://c.test/" })).await;
        assert_eq!(out["id"], 2);
        assert_eq!(out["url"], "https://c.test/");
        assert!(
            fake.calls()
                .contains(&"navigate 2 https://c.test/".to_string())
        );
    }

    #[tokio::test]
    async fn goto_explicit_tab_and_errors() {
        let fake = FakeBackend::with_tabs(&["https://a.test/", "https://b.test/"]);
        let out = call(
            &fake,
            "nav.goto",
            json!({ "url": "https://c.test/", "tab": 1 }),
        )
        .await;
        assert_eq!(out["id"], 1);

        let registry = registry();
        let err = registry
            .call(
                ctx(Caller::Rest, fake.clone()),
                "nav.goto",
                json!({ "url": "https://c.test/", "tab": 9 }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::TabNotFound);
        assert!(err.hint.unwrap().contains("tabs_list"));

        let err = registry
            .call(
                ctx(Caller::Rest, fake.clone()),
                "nav.goto",
                json!({ "url": "https://c.test/", "timeout_secs": 0 }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }

    #[tokio::test]
    async fn goto_without_tabs_is_no_tabs() {
        let err = registry()
            .call(
                ctx(Caller::Rest, FakeBackend::new()),
                "nav.goto",
                json!({ "url": "https://a.test/" }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NoTabs);
    }

    #[tokio::test]
    async fn goto_timeout_maps_to_timeout_error() {
        let fake = FakeBackend::with_tabs(&["https://a.test/"]);
        // `nav.goto` with an explicit tab makes exactly one backend call: navigate.
        fake.fail_next(BackendError::Timeout(std::time::Duration::from_secs(30)));
        let err = registry()
            .call(
                ctx(Caller::Rest, fake.clone()),
                "nav.goto",
                json!({ "url": "https://slow.test/", "tab": 1 }),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Timeout);
        assert_eq!(err.retry, crate::command::Retry::AfterWait);
    }

    #[tokio::test]
    async fn page_text_with_limit() {
        let fake = FakeBackend::with_tabs(&["https://a.test/"]);
        fake.set_text(TabId(1), "Привіт, світ!");
        let out = call(&fake, "page_text", json!({ "limit": 6 })).await;
        assert_eq!(
            out,
            json!({ "tab": 1, "text": "Привіт", "truncated": true, "total_chars": 13 })
        );

        let out = call(&fake, "page.text", json!({ "tab": 1 })).await;
        assert_eq!(out["text"], "Привіт, світ!");
        assert_eq!(out["truncated"], false);
    }
}
