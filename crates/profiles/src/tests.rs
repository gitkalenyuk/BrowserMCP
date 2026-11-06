//! Command tests with a mock [`ProfileBackend`] (no browser) and `FakeBackend` for CDP.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bmcp_cdpshare::CdpShare;
use bmcp_core::testing::FakeBackend;
use bmcp_core::{
    BackendError, BackendResult, BoxFuture, BrowserBackend, Caller, CmdError, Ctx, Registry, TabId,
};
use bmcp_data::DataStore;
use bmcp_identity::Services;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::backend::{CookieTarget, OpenProfile, ProfileBackend, ProfileSettings};
use crate::cookies::{CookieInfo, domain_matches};
use crate::permissions::PermissionStore;
use crate::rows::{RowProfiles, RowRequest};
use crate::service::ProfilesService;

/// Remembers what it was told; one cookie jar for every profile.
#[derive(Default)]
struct MockBackend {
    configured: Mutex<Vec<ProfileSettings>>,
    forgotten: Mutex<Vec<String>>,
    jar: Mutex<Vec<CookieInfo>>,
    /// Profiles that still have tabs (forget fails).
    busy: Mutex<Vec<String>>,
}

impl MockBackend {
    /// The settings the profile got last.
    fn last(&self, id: &str) -> Option<ProfileSettings> {
        self.configured
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|s| s.id == id)
            .cloned()
    }

    fn count(&self, id: &str) -> usize {
        self.configured
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.id == id)
            .count()
    }
}

impl ProfileBackend for MockBackend {
    fn configure(&self, settings: ProfileSettings) -> BoxFuture<'_, BackendResult<()>> {
        self.configured.lock().unwrap().push(settings);
        Box::pin(async { Ok(()) })
    }

    fn forget(&self, id: String) -> BoxFuture<'_, BackendResult<()>> {
        let busy = self.busy.lock().unwrap().contains(&id);
        if !busy {
            self.forgotten.lock().unwrap().push(id.clone());
        }
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
}

struct Setup {
    registry: Registry,
    /// A context with the registry's extensions (as the app's startup code has).
    ctx: Ctx,
    backend: Arc<MockBackend>,
    service: Arc<ProfilesService>,
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
    let mut ctx = bmcp_core::testing::ctx(Caller::Rest, FakeBackend::new());
    ctx.extensions = registry.extensions().clone();
    Setup {
        registry,
        ctx,
        backend,
        service,
        dir,
    }
}

impl Setup {
    fn identities(&self) -> Arc<Services> {
        self.registry.extensions().get::<Services>().unwrap()
    }

    fn file(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join("config").join(name)).unwrap_or_default()
    }

    /// Wait (real time, bounded) until the profile got `n` configurations.
    async fn configured(&self, id: &str, n: usize) -> ProfileSettings {
        for _ in 0..200 {
            if self.backend.count(id) >= n {
                return self.backend.last(id).unwrap();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "profile {id} was configured {} times",
            self.backend.count(id)
        );
    }
}

async fn call(s: &Setup, name: &str, args: Value) -> Result<Value, CmdError> {
    s.registry.call(s.ctx.clone(), name, args).await
}

/// One preference the browser was told to set.
fn pref(settings: &ProfileSettings, name: &str) -> Option<Value> {
    settings
        .prefs
        .to_preferences()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
}

fn pref_str(settings: &ProfileSettings, name: &str) -> String {
    pref(settings, name)
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

// ----------------------------------------------------------------- proxy via bmcp-identity

#[tokio::test]
async fn create_with_proxy_uses_the_identity_routing_and_forwarder() {
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
    assert_eq!(out["proxy_mode"], "fixed_servers");
    let server = out["proxy_server"].as_str().unwrap().to_string();
    assert!(server.starts_with("socks5://127.0.0.1:"), "{server}");

    // Browser: the same forwarder, WebRTC limited, the explicit language.
    let last = s.backend.last("work").unwrap();
    assert_eq!(last.prefs.proxy_server(), Some(server.as_str()));
    assert_eq!(
        pref(&last, "webrtc.ip_handling_policy").unwrap(),
        "disable_non_proxied_udp"
    );
    assert_eq!(pref(&last, "intl.accept_languages").unwrap(), "de-DE,de");
    assert_eq!(last.prefs.accept_language.as_deref(), Some("de-DE,de"));

    // One source of truth: the proxy is the profile's routing in proxies.json (with its
    // password, like every proxy there), not in profiles.json.
    let routes = s.identities().routes("work").unwrap();
    assert_eq!(routes.default, "socks5://user:secret@10.0.0.1:1080");
    assert!(!s.file("profiles.json").contains("secret"));
    // ... and the forwarder is bmcp-identity's (proxy_forward_list shows it).
    let fwd = call(&s, "proxy.forward_list", json!({})).await.unwrap();
    assert!(fwd.to_string().contains(&server[9..]), "{fwd}");

    // Updating keeps the same forwarder; removing the proxy goes direct and resets WebRTC.
    let again = call(&s, "profile.create", json!({"id": "work"}))
        .await
        .unwrap();
    assert_eq!(again["created"], false);
    assert_eq!(again["proxy_server"], server.as_str());
    let direct = call(&s, "profile.create", json!({"id": "work", "proxy": ""}))
        .await
        .unwrap();
    assert!(direct["proxy"].is_null());
    assert_eq!(direct["proxy_mode"], "direct");
    let last = s.backend.last("work").unwrap();
    assert_eq!(last.prefs.proxy, json!({"mode": "direct"}));
    assert_eq!(pref(&last, "webrtc.ip_handling_policy").unwrap(), "default");
    assert!(!s.file("proxies.json").contains("\"work\""));
}

#[tokio::test]
async fn create_rejects_bad_input_without_storing_anything() {
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
    for bad in ["ftp://nope", "de", "pool:"] {
        let err = call(&s, "profile.create", json!({"id": "x", "proxy": bad}))
            .await
            .unwrap_err();
        assert_eq!(err.code, bmcp_core::ErrorCode::InvalidParams, "{bad}");
    }
    // A pool that does not exist cannot be routed through.
    let err = call(
        &s,
        "profile.create",
        json!({"id": "x", "proxy": "pool:nope"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, bmcp_core::ErrorCode::NotFound);
    assert!(!s.file("profiles.json").contains("\"x\""));
    assert!(s.backend.last("x").is_none());
}

#[tokio::test]
async fn proxy_bypass_for_fixed_proxies_and_pool_references() {
    let s = setup();
    call(
        &s,
        "proxy.pool_add",
        json!({"pool": "de", "proxies": ["socks5://a:b@10.1.0.1:1080"]}),
    )
    .await
    .unwrap();
    let out = call(
        &s,
        "profile.create",
        json!({"id": "h", "proxy": "pool:de", "proxy_bypass": "<-loopback>"}),
    )
    .await
    .unwrap();
    assert_eq!(out["proxy"], "pool:de");
    let last = s.backend.last("h").unwrap();
    assert_eq!(last.prefs.proxy["mode"], "fixed_servers");
    assert_eq!(last.prefs.proxy["bypass_list"], "<-loopback>");
    // The pool proxy is leased to the profile (it keeps its IP).
    assert!(s.file("proxies.json").contains("profile.h"));
}

#[tokio::test]
async fn rules_set_and_identity_assign_are_applied_live() {
    let s = setup();
    crate::watch(s.service.clone(), &s.ctx.events);
    call(&s, "profile.create", json!({"id": "work"}))
        .await
        .unwrap();
    assert_eq!(s.backend.count("work"), 1);

    // proxy.rules_set → proxy.routes_changed → the profile's pref is re-applied (PAC).
    call(
        &s,
        "proxy.rules_set",
        json!({"profile": "work", "rules": [{"match": "*.example.com", "via": "socks5://u:p@10.0.0.2:1080"}]}),
    )
    .await
    .unwrap();
    let live = s.configured("work", 2).await;
    assert_eq!(live.prefs.proxy["mode"], "pac_script");
    assert_eq!(live.prefs.proxy["pac_mandatory"], true);
    assert!(!live.prefs.proxy.to_string().contains("u:p"), "no password");
    assert_eq!(
        pref(&live, "webrtc.ip_handling_policy").unwrap(),
        "disable_non_proxied_udp"
    );
    let list = call(&s, "profile.list", json!({})).await.unwrap();
    let work = list["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "work")
        .unwrap()
        .clone();
    assert_eq!(work["proxy_rules"], 1);
    assert_eq!(work["proxy_mode"], "pac_script");

    // identity.assign → identity.assigned → Accept-Language of the identity.
    call(
        &s,
        "identity.generate",
        json!({"preset": "windows-1080p", "seed": 3, "id": "fr1", "save": true, "locale": "fr-FR"}),
    )
    .await
    .unwrap();
    call(
        &s,
        "identity.assign",
        json!({"profile": "work", "id": "fr1"}),
    )
    .await
    .unwrap();
    let live = s.configured("work", 3).await;
    assert!(
        pref_str(&live, "intl.accept_languages").starts_with("fr-FR"),
        "{live:?}"
    );

    // A profile that only has routing (no profile.create) is applied too.
    call(
        &s,
        "proxy.rules_set",
        json!({"profile": "routed", "default": "socks5://10.0.0.9:1080"}),
    )
    .await
    .unwrap();
    let routed = s.configured("routed", 1).await;
    assert_eq!(routed.prefs.proxy["mode"], "fixed_servers");
}

#[tokio::test]
async fn startup_applies_routed_profiles_and_migrates_old_proxies() {
    let s = setup();
    // A profiles.json from before: the proxy stored in the profile.
    std::fs::create_dir_all(s.dir.path().join("config")).unwrap();
    std::fs::write(
        s.dir.path().join("config/profiles.json"),
        r#"{"profiles": {"old": {"proxy": "socks5://u:pw@10.0.0.3:1080", "created_ms": 1}}}"#,
    )
    .unwrap();
    s.identities()
        .set_routes(
            "split",
            bmcp_identity::proxy::RuleSet {
                rules: vec![bmcp_identity::proxy::Rule {
                    pattern: "*.a.test".into(),
                    via: "socks5://10.0.0.4:1080".into(),
                }],
                default: "direct".into(),
            },
        )
        .await
        .unwrap();
    crate::apply_stored(&s.ctx).await;
    let old = s.backend.last("old").unwrap();
    assert_eq!(old.prefs.proxy["mode"], "fixed_servers");
    assert_eq!(
        s.identities().routes("old").unwrap().default,
        "socks5://u:pw@10.0.0.3:1080"
    );
    assert!(!s.file("profiles.json").contains("pw@"), "moved out");
    assert_eq!(
        s.backend.last("split").unwrap().prefs.proxy["mode"],
        "pac_script"
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
    call(
        &s,
        "profile.create",
        json!({"id": "fr", "identity": "w1", "proxy": "socks5://10.0.0.5:1080"}),
    )
    .await
    .unwrap();
    // The identity's language becomes the profile's Accept-Language.
    let last = s.backend.last("fr").unwrap();
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
    // Routing and assignment are gone, the identity itself stays.
    assert!(s.identities().routes("fr").unwrap().is_direct());
    assert!(s.identities().identity_for_profile("fr").unwrap().is_none());
    assert!(s.file("identities.json").contains("\"w1\""));
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

// ----------------------------------------------------------------- workers

/// A fake tab that answers auto-attach and every call in a child session; `failing`
/// methods fail.
fn worker_backend(failing: &'static [&'static str]) -> Arc<FakeBackend> {
    let fake = FakeBackend::with_tabs(&["about:blank"]);
    fake.on_cdp(move |_, method, params| {
        if failing.contains(&method) {
            return Some(Err(BackendError::Cdp("not available on workers".into())));
        }
        (method == "Target.setAutoAttach" || params.get("sessionId").is_some())
            .then(|| Ok(json!({})))
    });
    fake
}

fn session_calls(fake: &FakeBackend, session: &str) -> Vec<String> {
    fake.cdp_calls()
        .into_iter()
        .filter(|(_, p)| p["sessionId"] == session)
        .map(|(m, _)| m)
        .collect()
}

fn identity() -> bmcp_identity::Identity {
    bmcp_identity::generate(Some(bmcp_identity::Os::Windows), 5, &Default::default())
}

#[tokio::test]
async fn worker_gets_identity_commands_in_order_and_resume_last() {
    let fake = worker_backend(&[
        "Emulation.setHardwareConcurrencyOverride",
        "Runtime.evaluate",
    ]);
    let attached = json!({
        "sessionId": "W1",
        "targetInfo": {"targetId": "T1", "type": "worker"},
        "waitingForDebugger": true,
    });
    let sent = crate::hooks::patch_attached(&*fake, TabId(1), &attached, &identity()).await;
    let expected = [
        "Network.setUserAgentOverride",
        "Emulation.setTimezoneOverride",
        "Emulation.setLocaleOverride",
        "Emulation.setHardwareConcurrencyOverride",
        "Runtime.evaluate",
        "Runtime.runIfWaitingForDebugger",
    ];
    assert_eq!(sent, expected);
    // Failures (even of a required command) never stop the resume.
    assert_eq!(session_calls(&fake, "W1"), expected);

    // An iframe paused by the shared auto-attach is only resumed.
    let iframe = json!({"sessionId": "F1", "targetInfo": {"type": "iframe"}});
    let sent = crate::hooks::patch_attached(&*fake, TabId(1), &iframe, &identity()).await;
    assert_eq!(sent, ["Runtime.runIfWaitingForDebugger"]);
}

#[tokio::test]
async fn watched_tab_pauses_workers_through_the_shared_auto_attach() {
    let s = setup();
    let fake = worker_backend(&[]);
    let backend: Arc<dyn BrowserBackend> = fake.clone();
    let share = Arc::new(CdpShare::default());
    // The OOPIF support switched auto-attach on first (no pausing).
    share
        .hold_auto_attach(&backend, TabId(1), None, "frames", false)
        .await
        .unwrap();
    crate::hooks::watch_workers(
        backend.clone(),
        share.clone(),
        s.identities(),
        TabId(1),
        identity(),
    )
    .await
    .unwrap();
    // Pausing is on now (and stays on whatever frames does), and the existing targets were
    // announced again for us.
    assert_eq!(share.auto_attach_waits(TabId(1), None), Some(true));
    let attach: Vec<Value> = fake
        .cdp_calls()
        .into_iter()
        .filter(|(m, _)| m == "Target.setAutoAttach")
        .map(|(_, p)| p)
        .collect();
    assert_eq!(attach.last().unwrap()["waitForDebuggerOnStart"], true);
    assert!(
        attach.iter().any(|p| p["autoAttach"] == false),
        "re-announced"
    );

    fake.emit_cdp_event(
        TabId(1),
        "Target.attachedToTarget",
        json!({"sessionId": "W9", "targetInfo": {"type": "service_worker"}, "waitingForDebugger": true}),
    );
    for _ in 0..200 {
        if session_calls(&fake, "W9").last().map(String::as_str)
            == Some("Runtime.runIfWaitingForDebugger")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let calls = session_calls(&fake, "W9");
    assert_eq!(calls.first().unwrap(), "Network.setUserAgentOverride");
    assert_eq!(calls.last().unwrap(), "Runtime.runIfWaitingForDebugger");
    assert_eq!(calls.len(), 6);
}

// ----------------------------------------------------------------- mass-run rows

fn row(profile: &str, row: u64, rotate: bool, fit: bool) -> RowRequest {
    RowRequest {
        run: "run-7".into(),
        row,
        profile: profile.into(),
        pool: Some("res".into()),
        fit_identity: fit,
        rotate,
    }
}

fn leases(s: &Setup) -> Vec<(u64, String)> {
    let file = s.identities().proxies.read().unwrap();
    file.pools["res"]
        .leases
        .iter()
        .filter(|l| l.run == "run-7")
        .map(|l| (l.row, l.url.clone()))
        .collect()
}

#[tokio::test]
async fn rows_lease_a_proxy_each_and_rotate_on_retry() {
    let s = setup();
    call(
        &s,
        "proxy.pool_add",
        json!({"pool": "res", "proxies": ["socks5://a:1@10.2.0.1:1080", "socks5://b:2@10.2.0.2:1080"]}),
    )
    .await
    .unwrap();
    let rows = RowProfiles::new(s.service.clone());
    assert!(rows.check(Some("res"), true).is_ok());
    assert_eq!(
        rows.check(Some("nope"), false).unwrap_err().code,
        bmcp_core::ErrorCode::NotFound
    );
    assert!(rows.check(None, true).is_err(), "fit needs a pool");

    // Two rows at once: one proxy each (least used first).
    let a1 = row("run-7-r0-a1", 0, false, false);
    let b1 = row("run-7-r1-a1", 1, false, false);
    let setup_a = rows.prepare(&a1).await.unwrap();
    let setup_b = rows.prepare(&b1).await.unwrap();
    assert_eq!(setup_a.proxy_id.as_deref(), Some("lease:res/run-7/0"));
    assert_ne!(setup_a.proxy, setup_b.proxy);
    let pa = s.backend.last("run-7-r0-a1").unwrap();
    let pb = s.backend.last("run-7-r1-a1").unwrap();
    assert_eq!(pa.prefs.proxy["mode"], "fixed_servers");
    assert_ne!(pa.prefs.proxy_server(), pb.prefs.proxy_server());
    assert_eq!(leases(&s).len(), 2);

    // Row 0 failed: its attempt profile goes, the retry rotates to the other proxy.
    let before = leases(&s)[0].1.clone();
    rows.release(&a1, false).await;
    assert!(
        s.backend
            .forgotten
            .lock()
            .unwrap()
            .contains(&"run-7-r0-a1".to_string())
    );
    assert!(s.identities().routes("run-7-r0-a1").unwrap().is_direct());
    assert_eq!(leases(&s).len(), 2, "the lease stays between attempts");
    let a2 = row("run-7-r0-a2", 0, true, false);
    let setup_a2 = rows.prepare(&a2).await.unwrap();
    assert_ne!(setup_a2.proxy, setup_a.proxy, "rotated");
    let after = leases(&s).into_iter().find(|l| l.0 == 0).unwrap().1;
    assert_ne!(before, after);
    let pa2 = s.backend.last("run-7-r0-a2").unwrap();
    assert_eq!(
        pa2.prefs.proxy_server(),
        pb.prefs.proxy_server(),
        "shares b's forwarder"
    );

    // Rows end: leases released; the run end cleans up leftovers (b1 was never released).
    rows.release(&a2, true).await;
    assert_eq!(leases(&s).len(), 1);
    rows.finish_run("run-7", Some("res"), "run-7-r").await;
    assert!(leases(&s).is_empty());
    assert!(s.identities().routes("run-7-r1-a1").unwrap().is_direct());
    assert!(!s.file("proxies.json").contains("run-7-r"));
}

/// An HTTP proxy that answers every tunnelled request itself with an ip-api.com style body
/// for a German IP (stands in for "proxy + geo service").
async fn fake_geo_proxy() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                async fn head(stream: &mut tokio::net::TcpStream) -> String {
                    let mut buf = Vec::new();
                    let mut byte = [0u8; 1];
                    while !buf.ends_with(b"\r\n\r\n") {
                        if stream.read(&mut byte).await.unwrap_or(0) == 0 {
                            break;
                        }
                        buf.push(byte[0]);
                    }
                    String::from_utf8_lossy(&buf).into_owned()
                }
                let connect = head(&mut stream).await;
                if !connect.starts_with("CONNECT") {
                    return;
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                    .await;
                let _ = head(&mut stream).await;
                let body = r#"{"status":"success","country":"Germany","countryCode":"DE","regionName":"Berlin","city":"Berlin","lat":52.52,"lon":13.4,"timezone":"Europe/Berlin","isp":"Test","query":"203.0.113.7"}"#;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn rows_get_an_identity_fitted_to_their_proxy() {
    let s = setup();
    let proxy = fake_geo_proxy().await;
    call(
        &s,
        "proxy.pool_add",
        json!({"pool": "res", "proxies": [format!("http://{proxy}")]}),
    )
    .await
    .unwrap();
    let rows = RowProfiles::new(s.service.clone()).with_geo_url("http://geo.test/json/");
    let r = row("run-7-r3-a1", 3, false, true);
    let setup_r = rows.prepare(&r).await.unwrap();
    assert_eq!(setup_r.identity.as_deref(), Some("run-7-r3-a1"));
    let identity = s
        .identities()
        .identity_for_profile("run-7-r3-a1")
        .unwrap()
        .expect("assigned to the row profile");
    assert_eq!(identity.timezone, "Europe/Berlin");
    assert!(identity.accept_language.starts_with("de-DE"));
    // The browser got the identity's language with the proxy.
    let p = s.backend.last("run-7-r3-a1").unwrap();
    assert!(p.prefs.proxied());
    assert!(pref_str(&p, "intl.accept_languages").starts_with("de-DE"));
    // The same row gets the same device on every attempt.
    let again = rows
        .prepare(&row("run-7-r3-a2", 3, false, true))
        .await
        .unwrap();
    let second = s
        .identities()
        .identity_for_profile("run-7-r3-a2")
        .unwrap()
        .unwrap();
    assert_eq!(again.identity.as_deref(), Some("run-7-r3-a2"));
    assert_eq!(second.user_agent, identity.user_agent);

    rows.release(&r, false).await;
    assert!(!s.file("identities.json").contains("run-7-r3-a1"));
    rows.finish_run("run-7", Some("res"), "run-7-r").await;
    assert!(!s.file("identities.json").contains("run-7-r3"));
}

// ----------------------------------------------------------------- cookies, permissions

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

#[test]
fn every_command_is_in_the_profile_toolset() {
    let mut registry = Registry::new();
    crate::register(&mut registry);
    for info in registry.list(None) {
        assert_eq!(info.toolset, bmcp_core::Toolset::Profile, "{}", info.name);
    }
}
