//! Command tests with a mock [`ProfileBackend`] (no browser).

use std::sync::{Arc, Mutex};

use bmcp_core::testing::FakeBackend;
use bmcp_core::{BackendError, BackendResult, BoxFuture, Caller, CmdError, Ctx, Registry, TabId};
use bmcp_data::DataStore;
use serde_json::{Value, json};

use crate::backend::{
    CookieTarget, OpenProfile, PermissionPrompt, ProfileBackend, ProfileSettings,
};
use crate::cookies::{CookieInfo, domain_matches};
use crate::permissions::PermissionStore;

/// Remembers what it was told; one cookie jar for every profile.
#[derive(Default)]
struct MockBackend {
    configured: Mutex<Vec<ProfileSettings>>,
    jar: Mutex<Vec<CookieInfo>>,
    /// Profiles that still have tabs (forget fails).
    busy: Mutex<Vec<String>>,
    /// Waiting permission requests; answered ones are removed (with the answer logged).
    prompts: Mutex<Vec<PermissionPrompt>>,
    answers: Mutex<Vec<(u64, bool)>>,
}

impl ProfileBackend for MockBackend {
    fn configure(&self, settings: ProfileSettings) -> BoxFuture<'_, BackendResult<()>> {
        self.configured.lock().unwrap().push(settings);
        Box::pin(async { Ok(()) })
    }

    fn forget(&self, id: String) -> BoxFuture<'_, BackendResult<()>> {
        let busy = self.busy.lock().unwrap().contains(&id);
        Box::pin(async move {
            if busy {
                Err(BackendError::Invalid(format!("profile {id} has open tabs")))
            } else {
                Ok(())
            }
        })
    }

    fn open_profiles(&self) -> BoxFuture<'_, BackendResult<Vec<OpenProfile>>> {
        let busy = self.busy.lock().unwrap().clone();
        Box::pin(async move {
            Ok(busy
                .into_iter()
                .map(|id| OpenProfile {
                    id,
                    incognito: false,
                    tabs: vec![TabId(1)],
                })
                .collect())
        })
    }

    fn cookies(
        &self,
        _target: CookieTarget,
        domain: Option<String>,
    ) -> BoxFuture<'_, BackendResult<Vec<CookieInfo>>> {
        let jar = self.jar.lock().unwrap().clone();
        Box::pin(async move {
            Ok(jar
                .into_iter()
                .filter(|c| domain_matches(&c.domain, domain.as_deref().unwrap_or("")))
                .collect())
        })
    }

    fn set_cookies(
        &self,
        _target: CookieTarget,
        cookies: Vec<CookieInfo>,
    ) -> BoxFuture<'_, BackendResult<Vec<bool>>> {
        // Stands in for the cookie store refusing a cookie.
        let ok = |c: &CookieInfo| !c.domain.contains("refused");
        let results = cookies.iter().map(ok).collect();
        self.jar
            .lock()
            .unwrap()
            .extend(cookies.into_iter().filter(|c| ok(c)));
        Box::pin(async move { Ok(results) })
    }

    fn delete_cookies(
        &self,
        _target: CookieTarget,
        domain: Option<String>,
        name: Option<String>,
    ) -> BoxFuture<'_, BackendResult<usize>> {
        let mut jar = self.jar.lock().unwrap();
        let before = jar.len();
        jar.retain(|c| {
            !(domain_matches(&c.domain, domain.as_deref().unwrap_or(""))
                && name.as_ref().is_none_or(|n| *n == c.name))
        });
        let deleted = before - jar.len();
        Box::pin(async move { Ok(deleted) })
    }

    fn pending_permissions(&self) -> BoxFuture<'_, BackendResult<Vec<PermissionPrompt>>> {
        let prompts = self.prompts.lock().unwrap().clone();
        Box::pin(async move { Ok(prompts) })
    }

    fn answer_permission(
        &self,
        id: u64,
        allow: bool,
    ) -> BoxFuture<'_, BackendResult<Option<PermissionPrompt>>> {
        let mut prompts = self.prompts.lock().unwrap();
        let found = prompts
            .iter()
            .position(|p| p.id == id)
            .map(|i| prompts.remove(i));
        if found.is_some() {
            self.answers.lock().unwrap().push((id, allow));
        }
        Box::pin(async move { Ok(found) })
    }
}

struct Setup {
    registry: Registry,
    ctx: Ctx,
    backend: Arc<MockBackend>,
    dir: tempfile::TempDir,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = Registry::new();
    crate::register(&mut registry);
    bmcp_identity::register(&mut registry);
    bmcp_identity::install(&registry, dir.path().join("config"));
    let permissions = PermissionStore::load(dir.path().join("config/permissions.json"));
    let service = crate::install(&registry, dir.path(), "default", permissions);
    let backend = Arc::new(MockBackend::default());
    service.set_backend(backend.clone());
    let ctx = bmcp_core::testing::ctx(Caller::Rest, FakeBackend::new());
    Setup {
        registry,
        ctx,
        backend,
        dir,
    }
}

async fn call(s: &Setup, name: &str, args: Value) -> Result<Value, CmdError> {
    s.registry.call(s.ctx.clone(), name, args).await
}

#[tokio::test]
async fn create_with_proxy_uses_a_local_forwarder() {
    let s = setup();
    let out = call(
        &s,
        "profile.create",
        json!({"id": "work", "proxy": "socks5://user:secret@10.0.0.1:1080", "accept_language": "de-DE,de;q=0.9"}),
    )
    .await
    .unwrap();
    assert_eq!(out["created"], true);
    assert_eq!(out["proxy"], "socks5://user:***@10.0.0.1:1080");
    let server = out["proxy_server"].as_str().unwrap().to_string();
    assert!(server.starts_with("socks5://127.0.0.1:"), "{server}");

    let configured = s.backend.configured.lock().unwrap().clone();
    let last = configured.last().unwrap();
    assert_eq!(last.id, "work");
    assert_eq!(last.prefs.proxy_server.as_deref(), Some(server.as_str()));
    assert_eq!(
        last.prefs.accept_language.as_deref(),
        Some("de-DE,de;q=0.9")
    );

    // Stored with the password (masked only in output).
    let text = std::fs::read_to_string(s.dir.path().join("config/profiles.json")).unwrap();
    assert!(text.contains("secret"));

    // Updating keeps the same forwarder; removing the proxy goes direct.
    let again = call(&s, "profile.create", json!({"id": "work"}))
        .await
        .unwrap();
    assert_eq!(again["created"], false);
    assert_eq!(again["proxy_server"], server.as_str());
    let direct = call(&s, "profile.create", json!({"id": "work", "proxy": ""}))
        .await
        .unwrap();
    assert!(direct["proxy_server"].is_null());
    let last = s.backend.configured.lock().unwrap().last().unwrap().clone();
    assert!(last.prefs.proxy_server.is_none());
}

#[tokio::test]
async fn create_rejects_bad_input() {
    let s = setup();
    let err = call(&s, "profile.create", json!({"id": "Bad Id"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
    let err = call(
        &s,
        "profile.create",
        json!({"id": "x", "identity": "missing"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NotFound);
    let err = call(
        &s,
        "profile.create",
        json!({"id": "x", "proxy": "socks5://u:p@h:1", "forward": false}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
}

#[tokio::test]
async fn http_proxy_without_forwarder_only_without_password() {
    let s = setup();
    let err = call(
        &s,
        "profile.create",
        json!({"id": "h", "proxy": "http://bob:pw@proxy.test:3128", "forward": false}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
    call(
        &s,
        "profile.create",
        json!({"id": "h", "proxy": "http://proxy.test:3128", "forward": false}),
    )
    .await
    .unwrap();
    let last = s.backend.configured.lock().unwrap().last().unwrap().clone();
    assert_eq!(
        last.prefs.proxy_server.as_deref(),
        Some("http://proxy.test:3128")
    );
}

#[tokio::test]
async fn identity_assignment_and_list() {
    let s = setup();
    call(
        &s,
        "identity.generate",
        json!({"preset": "windows-1080p", "seed": 1, "id": "w1", "save": true, "locale": "fr-FR"}),
    )
    .await
    .unwrap();
    call(&s, "profile.create", json!({"id": "fr", "identity": "w1"}))
        .await
        .unwrap();
    // The identity's language becomes the profile's Accept-Language.
    let last = s.backend.configured.lock().unwrap().last().unwrap().clone();
    assert!(last.prefs.accept_language.unwrap().starts_with("fr"));

    s.backend.busy.lock().unwrap().push("fr".into());
    let list = call(&s, "profile.list", json!({})).await.unwrap();
    let profiles = list["profiles"].as_array().unwrap();
    let fr = profiles.iter().find(|p| p["id"] == "fr").unwrap();
    assert_eq!(fr["identity"], "w1");
    assert_eq!(fr["open"], true);
    assert_eq!(fr["tabs"], json!([1]));
    assert!(
        profiles
            .iter()
            .any(|p| p["id"] == "default" && p["launch"] == true)
    );

    // Busy profiles cannot be deleted; the launch profile never.
    let err = call(&s, "profile.delete", json!({"id": "fr"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams);
    s.backend.busy.lock().unwrap().clear();
    let out = call(&s, "profile.delete", json!({"id": "fr"}))
        .await
        .unwrap();
    assert_eq!(out["removed"], true);
    let list = call(&s, "profile.list", json!({})).await.unwrap();
    assert!(
        !list["profiles"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "fr")
    );
    assert!(
        call(&s, "profile.delete", json!({"id": "default"}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cookies_commands_and_pending_import() {
    let s = setup();
    let store = DataStore::open_in_memory().unwrap();
    store
        .with(|c| {
            bmcp_data::import::import_cookies_json(
                c,
                r#"[{"domain":".a.test","name":"sid","value":"1","path":"/","secure":true,"httpOnly":true,"sameSite":"lax","expirationDate":1900000000},
                    {"domain":"refused.test","name":"bad","value":"2"}]"#,
            )
        })
        .unwrap();
    s.registry.extensions().insert(store.clone());
    let out = call(&s, "cookies.apply_pending", json!({})).await.unwrap();
    assert_eq!(out, json!({"applied": 1, "failed": 1}));
    // The accepted one left pending_cookies, the refused one stays.
    let left = store
        .with(|c| bmcp_data::import::list_pending_cookies(c))
        .unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].name, "bad");

    call(
        &s,
        "cookies.set",
        json!({"profile": "work", "cookies": [{"name": "x", "value": "y", "domain": "b.test"}]}),
    )
    .await
    .unwrap();
    let got = call(&s, "cookies.get", json!({"domain": "a.test"}))
        .await
        .unwrap();
    assert_eq!(got["cookies"][0]["name"], "sid");
    assert_eq!(got["cookies"][0]["same_site"], "lax");
    let deleted = call(&s, "cookies.delete", json!({"domain": "b.test"}))
        .await
        .unwrap();
    assert_eq!(deleted["deleted"], 1);
    assert!(
        call(&s, "cookies.get", json!({"profile": "../x"}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn permission_rules() {
    let s = setup();
    let out = call(
        &s,
        "permissions.set",
        json!({"origin": "https://meet.test/room", "kind": "camera", "allow": true}),
    )
    .await
    .unwrap();
    assert_eq!(
        out["rules"],
        json!([{"origin": "https://meet.test", "kind": "camera", "allow": true}])
    );
    let store = s.registry.extensions().get::<PermissionStore>().unwrap();
    assert!(store.decide_all("https://meet.test", &[crate::PermissionKind::Camera]));
    call(
        &s,
        "permissions.set",
        json!({"origin": "https://meet.test", "kind": "camera"}),
    )
    .await
    .unwrap();
    let out = call(&s, "permissions.list", json!({})).await.unwrap();
    assert_eq!(out["rules"], json!([]));
    assert!(
        call(
            &s,
            "permissions.set",
            json!({"origin": "x", "kind": "telepathy"})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn update_sets_label_and_color() {
    let s = setup();
    // The launch profile has no stored settings yet: it gets an entry.
    let out = call(
        &s,
        "profile.update",
        json!({"id": "default", "label": "  Personal ", "color": "Blue"}),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!({"id": "default", "label": "Personal", "color": "blue"})
    );
    let list = call(&s, "profile.list", json!({})).await.unwrap();
    let entry = &list["profiles"][0];
    assert_eq!(
        (entry["id"].as_str(), entry["label"].as_str()),
        (Some("default"), Some("Personal"))
    );
    assert_eq!(entry["color"], "blue");
    // The browser got the label (context menu).
    let configured = s.backend.configured.lock().unwrap().clone();
    assert_eq!(
        configured.last().unwrap().label.as_deref(),
        Some("Personal")
    );
    // "" clears; a profile without a label shows its id.
    let out = call(
        &s,
        "profile.update",
        json!({"id": "default", "label": "", "color": ""}),
    )
    .await
    .unwrap();
    assert_eq!(
        out,
        json!({"id": "default", "label": "default", "color": null})
    );
    // Bad color / label / id.
    for args in [
        json!({"id": "default", "color": "teal"}),
        json!({"id": "default", "label": "x".repeat(41)}),
        json!({"id": "Bad Id", "label": "x"}),
    ] {
        assert!(
            call(&s, "profile.update", args.clone()).await.is_err(),
            "{args}"
        );
    }
}

#[tokio::test]
async fn permission_prompts_are_answered_and_remembered() {
    let s = setup();
    s.backend.prompts.lock().unwrap().push(PermissionPrompt {
        id: 7,
        tab: TabId(3),
        origin: "https://maps.test".into(),
        kinds: vec![crate::PermissionKind::Geolocation],
    });
    let out = call(&s, "permissions.pending", json!({})).await.unwrap();
    assert_eq!(out["prompts"][0]["kinds"], json!(["geolocation"]));
    let out = call(
        &s,
        "permissions.answer",
        json!({"id": 7, "allow": true, "remember": true}),
    )
    .await
    .unwrap();
    assert_eq!(out["answered"], true);
    assert_eq!(
        out["remembered"],
        json!([{"origin": "https://maps.test", "kind": "geolocation", "allow": true}])
    );
    assert_eq!(*s.backend.answers.lock().unwrap(), vec![(7, true)]);
    let store = s.registry.extensions().get::<PermissionStore>().unwrap();
    assert_eq!(
        store.decide("https://maps.test", crate::PermissionKind::Geolocation),
        Some(true)
    );
    // Gone already: answered false, nothing stored.
    let out = call(
        &s,
        "permissions.answer",
        json!({"id": 7, "allow": false, "remember": true}),
    )
    .await
    .unwrap();
    assert_eq!(out["answered"], false);
    assert_eq!(out["remembered"], json!([]));
}

#[test]
fn every_command_is_in_the_profile_toolset() {
    let mut registry = Registry::new();
    crate::register(&mut registry);
    for info in registry.list(None) {
        assert_eq!(info.toolset, bmcp_core::Toolset::Profile, "{}", info.name);
    }
}
