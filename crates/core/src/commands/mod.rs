//! Built-in commands. One file per group (`tabs.rs` holds `tabs.*`, …).
//!
//! Adding a command: write a unit struct + `impl Command` in the right file, then add one line
//! to [`register_builtin`]. Every API surface picks it up from the registry automatically.

pub mod nav;
pub mod page;
pub mod system;
pub mod tabs;
pub mod ui;

use crate::command::Registry;

/// Register every built-in command.
pub fn register_builtin(registry: &mut Registry) {
    registry
        .add::<system::Ping>()
        .add::<tabs::List>()
        .add::<tabs::Open>()
        .add::<tabs::Close>()
        .add::<tabs::Activate>()
        .add::<nav::Goto>()
        .add::<nav::Stop>()
        .add::<page::Text>()
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

    /// What the fake reports for a fresh tab, as JSON (camelCase, like the UI expects).
    fn tab_json(id: i32, url: &str, active: bool) -> Value {
        json!({
            "id": id, "title": url, "url": url, "loading": false, "favicon": null,
            "canGoBack": false, "canGoForward": false, "active": active,
            "groupId": null, "lockedBy": null,
        })
    }

    #[tokio::test]
    async fn tabs_list_and_open() {
        let fake = FakeBackend::with_tabs(&["https://a.test/"]);
        let out = call(&fake, "tabs.open", json!({ "url": "https://b.test/" })).await;
        // Old shape `{tab}` plus the whole TabInfo.
        let mut expected = tab_json(2, "https://b.test/", true);
        expected["tab"] = json!(2);
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
