//! The commands with a fake browser: persistence, validation, when the browser is told.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{
    BackendError, BackendResult, BoxFuture, Caller, CmdError, Ctx, ErrorCode, Registry,
};
use serde_json::{Value, json};

use super::*;

/// One `apply_languages` call: (website languages, spellcheck dictionaries).
type Applied = (Vec<String>, Option<Vec<String>>);

/// Records `apply_languages` calls (when they finish); reports one stealth profile with its
/// own language. `fail_next` makes the next call fail; `delays` slow the next calls down.
#[derive(Default)]
struct FakeLanguages {
    applied: Mutex<Vec<Applied>>,
    fail_next: AtomicBool,
    delays: Mutex<VecDeque<Duration>>,
}

impl FakeLanguages {
    fn report() -> BackendLanguages {
        BackendLanguages {
            spellcheck_supported: Some(true),
            spellcheck_current: Some(vec!["en-US".into()]),
            own_language: vec![ProfileLanguage {
                profile: "de-proxy".into(),
                accept_language: "de-DE,de,en-US,en".into(),
            }],
        }
    }
}

impl LanguageBackend for FakeLanguages {
    fn apply_languages(
        &self,
        web: Vec<String>,
        spellcheck: Option<Vec<String>>,
    ) -> BoxFuture<'_, BackendResult<BackendLanguages>> {
        let delay = self.delays.lock().unwrap().pop_front();
        Box::pin(async move {
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(BackendError::Cdp("the browser went away".into()));
            }
            self.applied.lock().unwrap().push((web, spellcheck));
            Ok(FakeLanguages::report())
        })
    }

    fn languages(&self) -> BoxFuture<'_, BackendResult<BackendLanguages>> {
        Box::pin(async { Ok(FakeLanguages::report()) })
    }
}

struct Setup {
    registry: Registry,
    ctx: Ctx,
    browser: Arc<FakeLanguages>,
    tmp: tempfile::TempDir,
}

impl Setup {
    /// A data folder with `locales/{en-US,uk}.pak` and optionally a languages.json.
    fn new(stored: Option<Value>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let locales = tmp.path().join("locales");
        std::fs::create_dir_all(&locales).unwrap();
        for f in ["en-US.pak", "uk.pak", "uk_FEMININE.pak"] {
            std::fs::write(locales.join(f), b"").unwrap();
        }
        let config = tmp.path().join("config");
        if let Some(stored) = stored {
            std::fs::create_dir_all(&config).unwrap();
            std::fs::write(config.join(config::FILE), stored.to_string()).unwrap();
        }
        let mut registry = Registry::new();
        register(&mut registry);
        let service = registry
            .extensions()
            .insert(LanguagesService::new(&config, &locales));
        let browser = Arc::new(FakeLanguages::default());
        service.set_backend(browser.clone());
        let mut ctx = ctx(Caller::Rest, FakeBackend::new());
        ctx.extensions = registry.extensions().clone();
        Setup {
            registry,
            ctx,
            browser,
            tmp,
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.registry.call(self.ctx.clone(), name, args).await
    }

    fn stored(&self) -> Value {
        let text = std::fs::read_to_string(self.tmp.path().join("config").join(config::FILE))
            .unwrap_or_else(|_| "null".into());
        serde_json::from_str(&text).unwrap()
    }

    fn applied(&self) -> Vec<Applied> {
        self.browser.applied.lock().unwrap().clone()
    }
}

#[tokio::test]
async fn get_reports_the_automatic_list_and_the_locales() {
    let s = Setup::new(Some(json!({"ui_language": "uk"})));
    let out = s.call("browser.languages_get", json!({})).await.unwrap();
    assert_eq!(out["web"], json!(["uk", "en"]));
    assert_eq!(out["web_automatic"], true);
    assert_eq!(out["browser_locale"], Value::Null);
    assert_eq!(out["browser_locales"], json!(["en-US", "uk"]));
    assert_eq!(out["restart_required"], false);
    assert_eq!(out["spellcheck_supported"], true);
    // Never set: Chromium's own dictionaries are shown.
    assert_eq!(out["spellcheck"], json!(["en-US"]));
    assert_eq!(out["spellcheck_default"], true);
    assert_eq!(out["own_language"][0]["profile"], "de-proxy");
    assert!(s.applied().is_empty(), "reading changes nothing");
}

#[tokio::test]
async fn set_web_saves_normalizes_and_applies_live() {
    let s = Setup::new(None);
    let out = s
        .call(
            "browser.languages_set",
            json!({"web": ["UK", "en-us", "uk"]}),
        )
        .await
        .unwrap();
    assert_eq!(out["web"], json!(["uk", "en-US"]));
    assert_eq!(out["web_automatic"], false);
    assert_eq!(s.stored()["web"], json!(["uk", "en-US"]));
    assert_eq!(
        s.applied(),
        [(vec!["uk".to_string(), "en-US".to_string()], None)]
    );

    // The same again: nothing to apply.
    s.call("browser.languages_set", json!({"web": ["uk", "en-US"]}))
        .await
        .unwrap();
    assert_eq!(s.applied().len(), 1);

    // [] = back to automatic (the interface language, then English).
    s.call("browser.languages_set", json!({"ui_language": "en"}))
        .await
        .unwrap();
    assert_eq!(
        s.applied().len(),
        1,
        "a stored list hides the interface language"
    );
    let out = s
        .call("browser.languages_set", json!({"web": []}))
        .await
        .unwrap();
    assert_eq!(out["web"], json!(["en-US", "en"]));
    assert_eq!(out["web_automatic"], true);
    assert_eq!(s.applied().len(), 2);
}

#[tokio::test]
async fn the_interface_language_drives_the_automatic_list() {
    let s = Setup::new(None);
    let out = s
        .call("browser.languages_set", json!({"ui_language": "uk"}))
        .await
        .unwrap();
    assert_eq!(out["web"], json!(["uk", "en"]));
    assert_eq!(s.stored()["ui_language"], "uk");
    assert_eq!(s.applied().last().unwrap().0, ["uk", "en"]);
}

#[tokio::test]
async fn bad_tags_are_refused_and_nothing_is_saved() {
    let s = Setup::new(None);
    for args in [
        json!({"web": ["uk", "de;q=0.9"]}),
        json!({"spellcheck": ["english"]}),
        json!({"ui_language": "?"}),
        json!({"browser_locale": "de"}),
        json!({"unknown": 1}),
    ] {
        let err = s
            .call("browser.languages_set", args.clone())
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams, "{args}");
    }
    assert_eq!(s.stored(), Value::Null);
    assert!(s.applied().is_empty());
}

#[tokio::test]
async fn browser_locale_needs_a_restart() {
    let s = Setup::new(None);
    let out = s
        .call("browser.languages_set", json!({"browser_locale": "UK"}))
        .await
        .unwrap();
    assert_eq!(out["browser_locale"], "uk");
    assert_eq!(out["browser_locale_active"], Value::Null);
    assert_eq!(out["restart_required"], true);
    assert_eq!(s.stored()["browser_locale"], "uk");
    assert!(s.applied().is_empty(), "the locale is not applied live");
    let out = s
        .call("browser.languages_set", json!({"browser_locale": "system"}))
        .await
        .unwrap();
    assert_eq!(out["browser_locale"], Value::Null);
    assert_eq!(out["restart_required"], false);
}

#[tokio::test]
async fn a_locale_saved_before_startup_is_active() {
    let s = Setup::new(Some(json!({"browser_locale": "uk"})));
    let out = s.call("browser.languages_get", json!({})).await.unwrap();
    assert_eq!(out["browser_locale_active"], "uk");
    assert_eq!(out["restart_required"], false);
}

#[tokio::test]
async fn spellcheck_is_applied_with_the_web_list() {
    let s = Setup::new(Some(json!({"web": ["uk", "en"]})));
    s.call(
        "browser.languages_set",
        json!({"spellcheck": ["uk", "en-US"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        s.applied(),
        [(
            vec!["uk".to_string(), "en".to_string()],
            Some(vec!["uk".to_string(), "en-US".to_string()])
        )]
    );
    // [] turns spellcheck off (and is not "default" any more).
    let out = s
        .call("browser.languages_set", json!({"spellcheck": []}))
        .await
        .unwrap();
    assert_eq!(out["spellcheck"], json!([]));
    assert_eq!(out["spellcheck_default"], false);
    assert_eq!(s.applied().last().unwrap().1, Some(vec![]));
}

#[tokio::test]
async fn works_without_a_browser() {
    let mut registry = Registry::new();
    register(&mut registry);
    let tmp = tempfile::tempdir().unwrap();
    registry
        .extensions()
        .insert(bmcp_core::DataDir::from_root(tmp.path()));
    let mut c = ctx(Caller::Rest, FakeBackend::new());
    c.extensions = registry.extensions().clone();
    let out = registry
        .call(c, "browser.languages_set", json!({"web": ["pl"]}))
        .await
        .unwrap();
    assert_eq!(out["web"], json!(["pl"]));
    assert_eq!(out["spellcheck_supported"], Value::Null);
    assert!(tmp.path().join("config").join(config::FILE).exists());
}

#[tokio::test]
async fn a_failed_apply_is_retried_by_the_same_call() {
    let s = Setup::new(None);
    s.browser.fail_next.store(true, Ordering::SeqCst);
    let err = s
        .call("browser.languages_set", json!({"web": ["fr"]}))
        .await
        .unwrap_err();
    assert!(err.message.contains("went away"), "{err:?}");
    // Saved already, but the browser does not have it yet.
    assert_eq!(s.stored()["web"], json!(["fr"]));
    assert!(s.applied().is_empty());

    // The retry changes nothing in the file, yet must still reach the browser.
    s.call("browser.languages_set", json!({"web": ["fr"]}))
        .await
        .unwrap();
    assert_eq!(s.applied(), [(vec!["fr".to_string()], None)]);
    // In sync again: the same call is a no-op once more.
    s.call("browser.languages_set", json!({"web": ["fr"]}))
        .await
        .unwrap();
    assert_eq!(s.applied().len(), 1);
}

#[tokio::test]
async fn concurrent_sets_reach_the_browser_in_order() {
    let s = Setup::new(None);
    // The first call is slow in the browser; without the write lock the second one would be
    // applied first and then overwritten by the older list.
    s.browser
        .delays
        .lock()
        .unwrap()
        .extend([Duration::from_millis(50), Duration::ZERO]);
    let (a, b) = tokio::join!(
        s.call("browser.languages_set", json!({"web": ["de"]})),
        s.call(
            "browser.languages_set",
            json!({"web": ["pl"], "spellcheck": ["pl"]})
        ),
    );
    a.unwrap();
    b.unwrap();
    let applied = s.applied();
    assert_eq!(applied.len(), 2);
    assert_eq!(
        applied[1],
        (vec!["pl".to_string()], Some(vec!["pl".to_string()]))
    );
    assert_eq!(s.stored()["web"], json!(["pl"]));
    assert_eq!(s.stored()["spellcheck"], json!(["pl"]));
}
