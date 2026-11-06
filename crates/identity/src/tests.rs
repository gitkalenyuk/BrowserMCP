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
        crate::register(&mut registry, dir.path().join("config"));
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
    crate::register(&mut registry, dir.path());
    let all = registry.list(None);
    assert_eq!(all.len(), 20);
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
