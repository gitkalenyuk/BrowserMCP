//! Built-in commands. One file per group (`tabs.rs` holds `tabs.*`, …).
//!
//! Adding a command: write a unit struct + `impl Command` in the right file, then add one line
//! to [`register_builtin`]. Every API surface picks it up from the registry automatically.

pub mod nav;
pub mod page;
pub mod system;
pub mod tabs;

use crate::command::Registry;

/// Register every built-in command.
pub fn register_builtin(registry: &mut Registry) {
    registry
        .add::<system::Ping>()
        .add::<tabs::List>()
        .add::<tabs::Open>()
        .add::<nav::Goto>()
        .add::<page::Text>();
    // page.snapshot / page.find / page.act ... live in the `bmcp-browser` crate.
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
    async fn tabs_list_and_open() {
        let fake = FakeBackend::with_tabs(&["https://a.test/"]);
        let out = call(&fake, "tabs.open", json!({ "url": "https://b.test/" })).await;
        assert_eq!(out, json!({ "tab": 2 }));

        let out = call(&fake, "tabs.list", Value::Null).await;
        assert_eq!(
            out,
            json!({ "tabs": [
                { "id": 1, "title": "https://a.test/", "url": "https://a.test/", "loading": false },
                { "id": 2, "title": "https://b.test/", "url": "https://b.test/", "loading": false },
            ]})
        );
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
