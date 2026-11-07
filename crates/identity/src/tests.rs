//! Command tests against `FakeBackend`: which CDP calls the stealth commands make, and the
//! proxy/identity commands end to end with temporary config files.

use std::sync::Arc;

use bmcp_core::testing::{FakeBackend, ctx};
use bmcp_core::{BackendError, Caller, CmdError, ErrorCode, Registry};
use serde_json::{Value, json};

struct Harness {
    fake: Arc<FakeBackend>,
    registry: Registry,
    _dir: tempfile::TempDir,
}

impl Harness {
    fn new() -> Harness {
        let fake = FakeBackend::with_tabs(&["https://example.com/"]);
        // Answer the CDP methods the commands use like Chromium would.
        fake.on_cdp(|_tab, method, params| {
            Some(match method {
                "Browser.getVersion" => Ok(json!({ "product": "Chrome/156.0.7632.45" })),
                "Page.addScriptToEvaluateOnNewDocument" => Ok(json!({ "identifier": "7" })),
                "Runtime.evaluate" => {
                    let expr = params["expression"].as_str().unwrap_or_default();
                    if expr == "navigator.hardwareConcurrency" {
                        Ok(json!({ "result": { "type": "number", "value": 12 } }))
                    } else if expr.contains("checks") {
                        Ok(json!({ "result": { "type": "object", "value": { "checks": [
                            { "name": "navigator.webdriver", "status": "pass", "detail": "webdriver=false" },
                            { "name": "UA vs UA-CH version", "status": "pass", "detail": "ok" },
                            { "name": "plugins", "status": "warn", "detail": "plugins=0" },
                        ] } } }))
                    } else {
                        return None;
                    }
                }
                m if m.starts_with("Network.") || m.starts_with("Emulation.") || m.starts_with("Page.") => {
                    Ok(json!({}))
                }
                _ => return None,
            })
        });
        let dir = tempfile::tempdir().unwrap();
        let mut registry = Registry::new();
        crate::register(&mut registry);
        crate::install(&registry, dir.path().join("config"));
        Harness {
            fake,
            registry,
            _dir: dir,
        }
    }

    async fn call(&self, name: &str, args: Value) -> Result<Value, CmdError> {
        self.registry
            .call(ctx(Caller::Rest, self.fake.clone()), name, args)
            .await
    }

    async fn ok(&self, name: &str, args: Value) -> Value {
        self.call(name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    fn methods(&self) -> Vec<String> {
        self.fake.cdp_calls().into_iter().map(|(m, _)| m).collect()
    }

    fn params(&self, method: &str) -> Vec<Value> {
        self.fake
            .cdp_calls()
            .into_iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p)
            .collect()
    }
}

#[test]
fn every_command_is_in_the_stealth_toolset() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = Registry::new();
    crate::register(&mut registry);
    crate::install(&registry, dir.path());
    let all = registry.list(None);
    assert_eq!(all.len(), 26);
    assert!(all.iter().all(|c| c.toolset == bmcp_core::Toolset::Stealth));
}

#[tokio::test]
async fn fingerprint_set_sends_the_cdp_overrides() {
    let h = Harness::new();
    let generated = h
        .ok(
            "identity.generate",
            json!({ "os": "windows", "seed": 42, "save": true }),
        )
        .await;
    let id = generated["identity"]["id"].as_str().unwrap().to_string();
    assert_eq!(generated["problems"], json!([]));

    let out = h
        .ok(
            "stealth.fingerprint_set",
            json!({ "tab": 1, "identity_id": id }),
        )
        .await;
    assert_eq!(out["identity_id"], json!(id));
    assert_eq!(out["warnings"], json!([]), "{out}");

    assert_eq!(
        h.methods(),
        [
            "Browser.getVersion",
            "Network.setUserAgentOverride",
            "Emulation.setDeviceMetricsOverride",
            "Emulation.setTouchEmulationEnabled",
            "Emulation.setTimezoneOverride",
            "Emulation.setLocaleOverride",
            "Emulation.setGeolocationOverride",
            "Runtime.evaluate",
            "Emulation.setHardwareConcurrencyOverride",
            "Page.enable",
            "Page.addScriptToEvaluateOnNewDocument",
        ]
    );
    let identity = &generated["identity"];
    let ua = &h.params("Network.setUserAgentOverride")[0];
    assert_eq!(ua["userAgent"], identity["user_agent"]);
    assert_eq!(ua["platform"], "Win32");
    assert_eq!(ua["acceptLanguage"], identity["accept_language"]);
    let meta = &ua["userAgentMetadata"];
    assert_eq!(meta["platform"], "Windows");
    assert_eq!(meta["mobile"], false);
    // The engine's real full version replaces the placeholder.
    assert!(
        meta["fullVersionList"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["brand"] == "Google Chrome" && b["version"] == "156.0.7632.45"),
        "{meta}"
    );
    let metrics = &h.params("Emulation.setDeviceMetricsOverride")[0];
    assert_eq!(metrics["width"], 0, "desktop keeps the real window");
    assert_eq!(metrics["screenWidth"], identity["screen"]["width"]);
    assert_eq!(metrics["deviceScaleFactor"], identity["screen"]["dpr"]);
    assert_eq!(
        h.params("Emulation.setTouchEmulationEnabled")[0],
        json!({ "enabled": false })
    );
    assert_eq!(
        h.params("Emulation.setTimezoneOverride")[0]["timezoneId"],
        identity["timezone"]
    );
    assert_eq!(
        h.params("Emulation.setLocaleOverride")[0]["locale"],
        identity["locale"]
    );
    assert_eq!(
        h.params("Emulation.setHardwareConcurrencyOverride")[0]["hardwareConcurrency"],
        identity["hardware_concurrency"]
    );
    let script = &h.params("Page.addScriptToEvaluateOnNewDocument")[0];
    assert_eq!(script["runImmediately"], true);
    let source = script["source"].as_str().unwrap();
    assert!(source.contains(identity["webgl_renderer"].as_str().unwrap()));
    // Cores were set by CDP, so the script does not patch them.
    assert!(source.contains("\"hardwareConcurrency\":null"), "{source}");
    assert!(!h.methods().iter().any(|m| m == "Runtime.enable"));

    // Applying again replaces the old script; clearing undoes everything.
    h.ok(
        "stealth.fingerprint_set",
        json!({ "preset": "macos-mbp14" }),
    )
    .await;
    assert_eq!(
        h.params("Page.removeScriptToEvaluateOnNewDocument"),
        [json!({ "identifier": "7" })]
    );
    let before = h.methods().len();
    let cleared = h.ok("stealth.fingerprint_clear", json!({})).await;
    let after: Vec<String> = h.methods()[before..].to_vec();
    assert_eq!(
        after,
        [
            "Network.setUserAgentOverride",
            "Emulation.clearDeviceMetricsOverride",
            "Emulation.setTouchEmulationEnabled",
            "Emulation.setTimezoneOverride",
            "Emulation.setLocaleOverride",
            "Emulation.clearGeolocationOverride",
            "Emulation.setHardwareConcurrencyOverride",
            "Page.removeScriptToEvaluateOnNewDocument",
        ]
    );
    // The real core count (read before the first override) is restored.
    assert_eq!(
        h.params("Emulation.setHardwareConcurrencyOverride")
            .last()
            .unwrap()["hardwareConcurrency"],
        12
    );
    assert_eq!(
        h.params("Network.setUserAgentOverride").last().unwrap()["userAgent"],
        ""
    );
    assert!(cleared["warnings"][0].as_str().unwrap().contains("reload"));
}

#[tokio::test]
async fn missing_cdp_methods_become_warnings_and_js_fallbacks() {
    let h = Harness::new();
    // An engine without setHardwareConcurrencyOverride, with a timezone override already set.
    let tz_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = tz_calls.clone();
    h.fake.on_cdp(move |_tab, method, params| {
        Some(match method {
            "Emulation.setHardwareConcurrencyOverride" => Err(BackendError::Cdp(
                "'Emulation.setHardwareConcurrencyOverride' wasn't found".into(),
            )),
            "Emulation.setTimezoneOverride" if params["timezoneId"] != "" => {
                if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    Err(BackendError::Cdp(
                        "Timezone override is already in effect".into(),
                    ))
                } else {
                    Ok(json!({}))
                }
            }
            "Page.addScriptToEvaluateOnNewDocument" => Ok(json!({ "identifier": "1" })),
            "Runtime.evaluate" | "Browser.getVersion" => return None,
            _ => Ok(json!({})),
        })
    });
    let out = h
        .ok(
            "stealth.fingerprint_set",
            json!({ "preset": "windows-1080p" }),
        )
        .await;
    let warnings = out["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{out}");
    assert!(
        warnings[0]
            .as_str()
            .unwrap()
            .contains("setHardwareConcurrencyOverride")
    );
    // Timezone: failed once, cleared, set again.
    let tz = h.params("Emulation.setTimezoneOverride");
    assert_eq!(tz.len(), 3);
    assert_eq!(tz[1]["timezoneId"], "");
    // Cores now come from the patch script.
    let script = &h.params("Page.addScriptToEvaluateOnNewDocument")[0];
    assert!(
        script["source"]
            .as_str()
            .unwrap()
            .contains("\"hardwareConcurrency\":4")
    );
}

#[tokio::test]
async fn device_emulation_only_touches_ua_metrics_and_touch() {
    let h = Harness::new();
    let out = h
        .ok("device.emulate", json!({ "device": "iPhone 15" }))
        .await;
    assert_eq!(out["kind"], "device_emulation");
    assert_eq!(
        h.methods(),
        [
            "Browser.getVersion",
            "Network.setUserAgentOverride",
            "Emulation.setDeviceMetricsOverride",
            "Emulation.setTouchEmulationEnabled",
        ]
    );
    let ua = &h.params("Network.setUserAgentOverride")[0];
    assert!(ua["userAgent"].as_str().unwrap().contains("iPhone"));
    assert!(
        ua.get("userAgentMetadata").is_none(),
        "Safari has no client hints"
    );
    assert!(
        ua.get("acceptLanguage").is_none(),
        "device emulation keeps the language"
    );
    let metrics = &h.params("Emulation.setDeviceMetricsOverride")[0];
    assert_eq!(
        (metrics["width"].clone(), metrics["mobile"].clone()),
        (json!(393), json!(true))
    );
    assert_eq!(metrics["deviceScaleFactor"], 3.0);
    assert_eq!(
        h.params("Emulation.setTouchEmulationEnabled")[0],
        json!({ "enabled": true, "maxTouchPoints": 5 })
    );

    let pixel = h.ok("device.emulate", json!({ "device": "pixel 8" })).await;
    assert_eq!(pixel["kind"], "stealth");
    let ua = h.params("Network.setUserAgentOverride").pop().unwrap();
    assert_eq!(ua["userAgentMetadata"]["model"], "Pixel 8");
    assert_eq!(ua["userAgentMetadata"]["mobile"], true);

    let err = h
        .call("device.emulate", json!({ "device": "Nokia" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.candidates.contains(&json!("iPhone 15")));

    h.ok("device.clear", json!({ "tab": 1 })).await;
    assert!(h.methods().ends_with(&[
        "Network.setUserAgentOverride".to_string(),
        "Emulation.clearDeviceMetricsOverride".to_string(),
        "Emulation.setTouchEmulationEnabled".to_string(),
    ]));
}

#[tokio::test]
async fn check_reports_probe_results_against_the_applied_identity() {
    let h = Harness::new();
    h.ok(
        "stealth.fingerprint_set",
        json!({ "preset": "windows-1080p" }),
    )
    .await;
    let out = h.ok("stealth.check", json!({})).await;
    assert_eq!(
        (out["passed"].clone(), out["warned"].clone()),
        (json!(2), json!(1))
    );
    assert_eq!(out["identity_id"], "windows-1080p");
    let text = out["text"].as_str().unwrap();
    assert!(
        text.lines()
            .next()
            .unwrap()
            .starts_with("PASS  navigator.webdriver"),
        "{text}"
    );
    // The probe got the expected identity embedded.
    let probe = h
        .params("Runtime.evaluate")
        .into_iter()
        .find(|p| p["expression"].as_str().unwrap().contains("checks"))
        .unwrap();
    assert!(
        probe["expression"]
            .as_str()
            .unwrap()
            .contains("\"timezone\":\"America/New_York\"")
    );
    assert_eq!(probe["awaitPromise"], true);
}

#[tokio::test]
async fn fingerprint_set_needs_exactly_one_source() {
    let h = Harness::new();
    for args in [
        json!({}),
        json!({ "preset": "windows-1080p", "identity_id": "x" }),
    ] {
        let err = h.call("stealth.fingerprint_set", args).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);
    }
    let err = h
        .call("stealth.fingerprint_set", json!({ "identity_id": "nope" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = h
        .call(
            "stealth.fingerprint_set",
            json!({ "tab": 9, "preset": "windows-1080p" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::TabNotFound);
    // An inline identity works and inconsistencies are reported as warnings.
    let mut identity = h
        .ok("identity.generate", json!({ "os": "windows", "seed": 1 }))
        .await["identity"]
        .clone();
    identity["platform"] = json!("MacIntel");
    let out = h
        .ok("stealth.fingerprint_set", json!({ "identity": identity }))
        .await;
    assert!(
        out["warnings"][0]
            .as_str()
            .unwrap()
            .contains("navigator.platform"),
        "{out}"
    );
}

#[tokio::test]
async fn no_identity_is_assigned_while_the_cdp_port_is_open() {
    let h = Harness::new();
    h.ok(
        "identity.generate",
        json!({ "seed": 1, "id": "alpha", "save": true }),
    )
    .await;
    h.registry.extensions().insert(bmcp_core::CdpLaunch {
        port: Some(9222),
        ..Default::default()
    });
    let err = h
        .call(
            "identity.assign",
            json!({ "profile": "work", "id": "alpha" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    assert!(err.message.contains("CDP port 9222"), "{}", err.message);
    let list = h.ok("identity.list", json!({})).await;
    assert!(list["profiles"].as_object().unwrap().is_empty());
    // Removing an assignment is always allowed.
    h.ok("identity.assign", json!({ "profile": "work" })).await;

    // Port closed: assigning works.
    h.registry
        .extensions()
        .insert(bmcp_core::CdpLaunch::default());
    h.ok(
        "identity.assign",
        json!({ "profile": "work", "id": "alpha" }),
    )
    .await;
}

#[tokio::test]
async fn identities_are_saved_listed_assigned_and_removed() {
    let h = Harness::new();
    let a = h
        .ok(
            "identity.generate",
            json!({ "seed": 5, "id": "alpha", "save": true, "country": "DE" }),
        )
        .await;
    assert_eq!(a["identity"]["timezone"], "Europe/Berlin");
    // Same seed → same identity (apart from the id we chose).
    let again = h.ok("identity.generate", json!({ "seed": 5 })).await;
    assert_eq!(again["identity"]["user_agent"], a["identity"]["user_agent"]);
    assert_eq!(
        again["identity"]["webgl_renderer"],
        a["identity"]["webgl_renderer"]
    );

    h.ok(
        "identity.assign",
        json!({ "profile": "work", "id": "alpha" }),
    )
    .await;
    let list = h.ok("identity.list", json!({})).await;
    assert_eq!(list["identities"].as_array().unwrap().len(), 1);
    assert_eq!(list["profiles"]["work"], "alpha");
    let got = h.ok("identity.get", json!({ "id": "alpha" })).await;
    assert_eq!(got["identity"], a["identity"]);
    let err = h
        .call(
            "identity.assign",
            json!({ "profile": "work", "id": "beta" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = h
        .call("identity.generate", json!({ "id": "Bad Name" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);

    assert_eq!(
        h.ok("identity.remove", json!({ "id": "alpha" })).await["removed"],
        true
    );
    let list = h.ok("identity.list", json!({})).await;
    assert!(list["identities"].as_array().unwrap().is_empty());
    assert!(list["profiles"].as_object().unwrap().is_empty());

    let presets = h.ok("identity.presets", json!({})).await;
    assert!(presets["presets"].as_array().unwrap().len() >= 8);
    let p = h
        .ok(
            "identity.generate",
            json!({ "preset": "android-pixel-8", "seed": 3 }),
        )
        .await;
    assert_eq!(p["identity"]["os"], "android");
    let err = h
        .call(
            "identity.generate",
            json!({ "preset": "android-pixel-8", "os": "macos" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

#[tokio::test]
async fn proxy_pools_rotate_and_mask_passwords() {
    let h = Harness::new();
    let out = h
        .ok(
            "proxy.pool_add",
            json!({ "pool": "de", "proxies": ["http://u:secret@10.0.0.1:8080", "10.0.0.2:8080:u:secret", "http://u:secret@10.0.0.1:8080"] }),
        )
        .await;
    assert_eq!(
        (out["added"].clone(), out["total"].clone()),
        (json!(2), json!(2))
    );
    let err = h
        .call(
            "proxy.pool_add",
            json!({ "pool": "de", "proxies": ["ftp://x:1"] }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);

    let list = h.ok("proxy.pool_list", json!({})).await;
    let text = list.to_string();
    assert!(!text.contains("secret"), "{text}");
    assert!(text.contains("http://u:***@10.0.0.2:8080"));

    let a = h.ok("proxy.next", json!({ "pool": "de" })).await["proxy"].clone();
    let b = h.ok("proxy.next", json!({ "pool": "de" })).await["proxy"].clone();
    let c = h.ok("proxy.next", json!({ "pool": "de" })).await["proxy"].clone();
    assert_ne!(a, b);
    assert_eq!(a, c);

    // Sticky: the same run keeps its proxy.
    h.ok(
        "proxy.pool_add",
        json!({ "pool": "de", "proxies": [], "strategy": "sticky" }),
    )
    .await;
    let r1 = h
        .ok("proxy.next", json!({ "pool": "de", "run": "row-1" }))
        .await["proxy"]
        .clone();
    for _ in 0..3 {
        h.ok("proxy.next", json!({ "pool": "de", "run": "row-2" }))
            .await;
        assert_eq!(
            h.ok("proxy.next", json!({ "pool": "de", "run": "row-1" }))
                .await["proxy"],
            r1
        );
    }

    let removed = h
        .ok(
            "proxy.pool_remove",
            json!({ "pool": "de", "proxy": "http://u:***@10.0.0.2:8080" }),
        )
        .await;
    assert_eq!(
        (removed["removed"].clone(), removed["remaining"].clone()),
        (json!(true), json!(1))
    );
    assert_eq!(
        h.ok("proxy.pool_remove", json!({ "pool": "de" })).await["removed"],
        true
    );
    let err = h
        .call("proxy.next", json!({ "pool": "de" }))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn pool_check_and_forwarder_commands_work_end_to_end() {
    use crate::proxy::forward::tests::{fake_socks5, origin};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let h = Harness::new();
    let origin = origin().await;
    let socks = fake_socks5("u", "p").await;
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    h.ok(
        "proxy.pool_add",
        json!({ "pool": "local", "proxies": [format!("socks5://u:p@{socks}"), format!("socks5://u:p@{dead}")] }),
    )
    .await;
    let checked = h
        .ok(
            "proxy.pool_check",
            json!({ "pool": "local", "target": format!("127.0.0.1:{}", origin.port()), "timeout_ms": 2000 }),
        )
        .await;
    assert_eq!(
        (checked["healthy"].clone(), checked["unhealthy"].clone()),
        (json!(1), json!(1)),
        "{checked}"
    );

    // Rotation now always picks the healthy one; the forwarder uses it.
    let fwd = h
        .ok("proxy.forward_start", json!({ "pool": "local" }))
        .await;
    assert!(
        fwd["upstream"]
            .as_str()
            .unwrap()
            .contains(&socks.to_string())
    );
    let local: crate::ProxyConfig = fwd["socks5"].as_str().unwrap().parse().unwrap();
    let mut s = crate::proxy::forward::connect_via(&local, "127.0.0.1", origin.port())
        .await
        .unwrap();
    s.write_all(b"ECHO\nhi").await.unwrap();
    let mut buf = [0u8; 2];
    s.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"hi");

    let list = h.ok("proxy.forward_list", json!({})).await;
    assert_eq!(list["forwarders"].as_array().unwrap().len(), 1);
    let id = fwd["id"].as_str().unwrap();
    assert_eq!(
        h.ok("proxy.forward_stop", json!({ "id": id })).await["stopped"],
        true
    );
    assert_eq!(
        h.ok("proxy.forward_stop", json!({ "id": id })).await["stopped"],
        false
    );

    let err = h
        .call(
            "proxy.forward_start",
            json!({ "proxy": "x:1", "pool": "local" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
}

impl Harness {
    /// Call with a context whose event bus the test can watch.
    async fn ok_on(&self, bus: &bmcp_core::EventBus, name: &str, args: Value) -> Value {
        let ctx = bmcp_core::Ctx::new(Caller::Rest, self.fake.clone(), bus.clone());
        self.registry
            .call(ctx, name, args)
            .await
            .unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    fn services(&self) -> Arc<crate::Services> {
        self.registry.extensions().get::<crate::Services>().unwrap()
    }
}

#[tokio::test]
async fn rules_set_builds_a_pac_script_and_rules_test_explains_routes() {
    let h = Harness::new();
    h.ok(
        "proxy.pool_add",
        json!({ "pool": "de", "proxies": ["socks5://u:secret@127.0.0.1:1081", "socks5://u:secret@127.0.0.1:1082"] }),
    )
    .await;
    let bus = bmcp_core::EventBus::new();
    let mut events = bus.subscribe();
    let rules = json!({
        "profile": "work",
        "rules": [
            { "match": "*.example.com", "via": "pool:de" },
            { "match": "re:\\.ru/", "via": "direct" },
        ],
        "default": "http://bob:pw@127.0.0.1:3128",
    });
    let out = h.ok_on(&bus, "proxy.rules_set", rules.clone()).await;
    let text = out.to_string();
    assert!(!text.contains("secret") && !text.contains(":pw@"), "{text}");
    assert_eq!(out["pref"]["mode"], "pac_script");
    assert_eq!(out["pref"]["pac_mandatory"], true);
    assert_eq!(out["forwarders"].as_array().unwrap().len(), 2, "{out}");
    let script = out["pac_script"].as_str().unwrap();
    assert!(script.contains("return \"DIRECT\""), "{script}");
    assert_eq!(script.matches("SOCKS5 127.0.0.1:").count(), 2, "{script}");
    assert_eq!(out["default"], "http://bob:***@127.0.0.1:3128");

    // The browser hears about it.
    let event = std::iter::from_fn(|| events.try_recv().ok())
        .find(|e| e.topic == crate::ROUTES_CHANGED)
        .expect("proxy.routes_changed event");
    assert_eq!(event.data["profile"], "work");
    assert_eq!(event.data["pref"], out["pref"]);

    // Setting the same rules again reuses the forwarders and the profile's pool lease.
    let again = h.ok("proxy.rules_set", rules.clone()).await;
    assert_eq!(again["pref"], out["pref"]);
    assert_eq!(h.services().forwarders().len(), 2);
    // ... and the cef crate gets the same pref from the stored rules.
    let pref = h.services().profile_proxy_pref("work").await.unwrap();
    assert_eq!(pref.pref, out["pref"]);
    assert!(
        h.services().profile_proxy_pref("other").await.unwrap().pref == json!({ "mode": "direct" })
    );

    let test = |url: &str| h.ok("proxy.rules_test", json!({ "profile": "work", "url": url }));
    let r = test("https://www.example.com/secret/path").await;
    assert_eq!(r["rule"], 1);
    assert_eq!(r["via"], "pool:de");
    assert_eq!(r["pac_url"], "https://www.example.com/");
    assert!(
        r["upstream"]
            .as_str()
            .unwrap()
            .starts_with("socks5://u:***@127.0.0.1:108"),
        "{r}"
    );
    let r = test("https://yandex.ru/search?q=x").await;
    assert_eq!(
        (r["rule"].clone(), r["via"].clone()),
        (json!(2), json!("direct"))
    );
    assert_eq!(r["upstream"], Value::Null);
    let r = test("http://other.org/").await;
    assert_eq!(r["rule"], Value::Null);
    assert_eq!(r["upstream"], "http://bob:***@127.0.0.1:3128");
    let err = h
        .call(
            "proxy.rules_test",
            json!({ "profile": "work", "url": "nope" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);

    // Bad rules and unusable routes store nothing.
    let err = h
        .call(
            "proxy.rules_set",
            json!({ "profile": "work", "rules": [{ "match": "*", "via": "nowhere" }] }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = h
        .call(
            "proxy.rules_set",
            json!({ "profile": "work", "default": "pool:missing" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert_eq!(test("https://a.example.com/").await["rule"], 1);

    // No rules + direct removes the routing and gives the pool lease back.
    let cleared = h.ok("proxy.rules_set", json!({ "profile": "work" })).await;
    assert_eq!(cleared["pref"], json!({ "mode": "direct" }));
    let file = h.services().proxies.read().unwrap();
    assert!(file.routes.is_empty());
    assert!(file.pools["de"].leases.is_empty());

    // Only a default proxy: fixed_servers, no PAC.
    let fixed = h
        .ok(
            "proxy.rules_set",
            json!({ "profile": "work", "default": "pool:de" }),
        )
        .await;
    assert_eq!(fixed["pref"]["mode"], "fixed_servers");
    assert_eq!(fixed["pac_script"], Value::Null);
}

#[tokio::test]
async fn leases_stick_per_row_rotate_and_release() {
    let h = Harness::new();
    h.ok(
        "proxy.pool_add",
        json!({ "pool": "runs", "proxies": ["http://u:pw@10.0.0.1:8080", "http://u:pw@10.0.0.2:8080"] }),
    )
    .await;
    let lease = |row: u64, rotate: bool| {
        h.ok(
            "proxy.lease",
            json!({ "pool": "runs", "run_id": "run-1", "row": row, "rotate": rotate }),
        )
    };
    let a = lease(0, false).await;
    assert_eq!(a["proxy_id"], "lease:runs/run-1/0");
    assert_eq!(a["reused"], false);
    assert!(!a.to_string().contains(":pw@"));
    let again = lease(0, false).await;
    assert_eq!(
        (again["proxy"].clone(), again["reused"].clone()),
        (a["proxy"].clone(), json!(true))
    );
    let b = lease(1, false).await;
    assert_ne!(b["proxy"], a["proxy"], "rows spread over the pool");
    let rotated = lease(0, true).await;
    assert_ne!(rotated["proxy"], a["proxy"]);
    assert_eq!(rotated["load"], 2);

    // The lease reference works where proxies are referenced.
    let fwd = h
        .ok(
            "proxy.forward_start",
            json!({ "proxy_id": "lease:runs/run-1/0" }),
        )
        .await;
    assert_eq!(fwd["upstream"], rotated["proxy"]);
    let routed = h
        .ok(
            "proxy.rules_set",
            json!({ "profile": "row-0", "default": "lease:runs/run-1/0" }),
        )
        .await;
    assert_eq!(
        routed["pref"]["server"], fwd["socks5"],
        "the running forwarder for that upstream is reused"
    );

    assert_eq!(
        h.ok(
            "proxy.release",
            json!({ "pool": "runs", "run_id": "run-1", "row": 0 })
        )
        .await["released"],
        1
    );
    assert_eq!(
        h.ok(
            "proxy.release",
            json!({ "pool": "runs", "run_id": "run-1" })
        )
        .await["released"],
        1
    );
    let err = h
        .call(
            "proxy.forward_start",
            json!({ "proxy_id": "lease:runs/run-1/0" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    let err = h
        .call(
            "proxy.lease",
            json!({ "pool": "runs", "run_id": "a/b", "row": 0 }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = h
        .call(
            "proxy.lease",
            json!({ "pool": "none", "run_id": "r", "row": 0 }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn fit_to_proxy_moves_the_identity_to_the_exit_ip() {
    use crate::proxy::forward::tests::fake_socks5;
    use crate::proxy::geo::tests::fake_ip_api;

    let h = Harness::new();
    let generated = h
        .ok(
            "identity.generate",
            json!({ "seed": 9, "id": "fit", "save": true, "country": "US" }),
        )
        .await;
    let api = fake_ip_api(false).await;
    let url = format!("http://127.0.0.1:{}/json/", api.port());
    let socks = fake_socks5("u", "p").await;
    let out = h
        .ok(
            "identity.fit_to_proxy",
            json!({ "identity_id": "fit", "proxy_id": format!("socks5://u:p@{socks}"), "url": url }),
        )
        .await;
    assert_eq!(out["via"], format!("socks5://u:***@{socks}"));
    assert_eq!(out["geo"]["country_code"], "DE");
    assert_eq!(out["saved"], true);
    let fields: Vec<&str> = out["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["field"].as_str().unwrap())
        .collect();
    assert_eq!(
        fields,
        ["timezone", "locale", "accept_language", "geolocation"]
    );
    let saved = h.ok("identity.get", json!({ "id": "fit" })).await["identity"].clone();
    assert_eq!(saved["timezone"], "Europe/Berlin");
    assert_eq!(saved["locale"], "de-DE");
    assert_eq!(saved["accept_language"], "de-DE,de,en-US,en");
    assert_eq!(saved["user_agent"], generated["identity"]["user_agent"]);
    let lat = saved["geolocation"]["latitude"].as_f64().unwrap();
    assert!((lat - 50.1109).abs() < 0.05, "{lat}");

    // Already fitting: nothing changes, nothing is written.
    let again = h
        .ok(
            "identity.fit_to_proxy",
            json!({ "identity_id": "fit", "proxy_id": "direct", "url": url }),
        )
        .await;
    assert_eq!(again["changes"], json!([]));
    assert_eq!(
        (again["saved"].clone(), again["via"].clone()),
        (json!(false), Value::Null)
    );

    let err = h
        .call(
            "identity.fit_to_proxy",
            json!({ "identity_id": "fit", "proxy_id": "de" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidParams);
    let err = h
        .call(
            "identity.fit_to_proxy",
            json!({ "identity_id": "nope", "proxy_id": "direct" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
}

#[tokio::test]
async fn patch_and_probe_cover_the_available_screen() {
    let h = Harness::new();
    // windows-1080p as-is: Windows 10 (platformVersion 10.0.0) → 40 px taskbar.
    h.ok(
        "stealth.fingerprint_set",
        json!({ "preset": "windows-1080p" }),
    )
    .await;
    let source = h.params("Page.addScriptToEvaluateOnNewDocument")[0]["source"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(source.contains("\"availHeight\":1040"), "{source}");
    assert!(source.contains("\"availWidth\":1920"));
    assert!(
        source.contains("\"userAgent\":null"),
        "pages get the UA from CDP"
    );
    h.ok("stealth.check", json!({})).await;
    let probe = h
        .params("Runtime.evaluate")
        .into_iter()
        .find(|p| p["expression"].as_str().unwrap().contains("checks"))
        .unwrap();
    let expr = probe["expression"].as_str().unwrap();
    assert!(expr.contains("\"screen_avail\":{"), "{expr}");
    assert!(expr.contains("UA-CH high entropy vs identity"));
    assert!(expr.contains("media devices") && expr.contains("fonts vs OS"));

    // Phones report the full screen: no avail patch.
    let android = crate::identity::preset("android-pixel-8").unwrap();
    let id = crate::identity::from_preset(android, "a");
    assert_eq!(crate::stealth::available_screen(&id), None);
    assert!(crate::stealth::patch_script(&id, false).contains("\"availHeight\":null"));
}

#[tokio::test]
async fn rules_get_masks_and_masked_urls_keep_their_password() {
    let h = Harness::new();
    let none = h.ok("proxy.rules_get", json!({})).await;
    assert_eq!(none["routes"], json!([]));
    h.ok(
        "proxy.rules_set",
        json!({
            "profile": "work",
            "rules": [{ "match": "*.example.com", "via": "socks5://u:secret@127.0.0.1:1081" }],
            "default": "http://bob:pw@127.0.0.1:3128",
        }),
    )
    .await;
    let got = h.ok("proxy.rules_get", json!({ "profile": "work" })).await;
    let text = got.to_string();
    assert!(!text.contains("secret") && !text.contains(":pw@"), "{text}");
    let route = &got["routes"][0];
    assert_eq!(route["profile"], "work");
    assert_eq!(route["default"], "http://bob:***@127.0.0.1:3128");
    assert_eq!(route["rules"][0]["match"], "*.example.com");
    assert_eq!(
        h.ok("proxy.rules_get", json!({ "profile": "other" })).await["routes"],
        json!([])
    );

    // Sending the masked routes back (plus a new rule) keeps both stored passwords.
    h.ok(
        "proxy.rules_set",
        json!({
            "profile": "work",
            "rules": [route["rules"][0].clone(), { "match": "*.ru", "via": "direct" }],
            "default": route["default"].clone(),
        }),
    )
    .await;
    let file = h.services().proxies.read().unwrap();
    let stored = &file.routes["work"];
    assert_eq!(stored.default, "http://bob:pw@127.0.0.1:3128");
    assert_eq!(stored.rules[0].via, "socks5://u:secret@127.0.0.1:1081");
    assert_eq!(stored.rules[1].via, "direct");
}
