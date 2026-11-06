//! Applying an [`Identity`] to a tab through CDP, removing it again, and probing the page.
//!
//! What goes where:
//!
//! | value                                  | how                                              |
//! |----------------------------------------|--------------------------------------------------|
//! | UA, Accept-Language, platform, UA-CH   | `Network.setUserAgentOverride`                   |
//! | timezone / locale                      | `Emulation.setTimezoneOverride` / `setLocaleOverride` |
//! | screen, DPR, viewport, mobile          | `Emulation.setDeviceMetricsOverride`             |
//! | touch                                  | `Emulation.setTouchEmulationEnabled`             |
//! | location                               | `Emulation.setGeolocationOverride`               |
//! | CPU cores                              | `Emulation.setHardwareConcurrencyOverride` (JS patch if missing) |
//! | deviceMemory, webdriver, WebGL strings, screen.avail* | small main-world script (`js/patch.js`) via `Page.addScriptToEvaluateOnNewDocument` |
//!
//! Overrides belong to the tab's DevTools session: they last until cleared or the tab closes,
//! and a new tab starts clean (auto-applying a profile's identity to new tabs is a CEF-side
//! follow-up). We never call `Runtime.enable` (pages can detect it).

use std::collections::HashMap;
use std::sync::Mutex;

use bmcp_core::{BackendError, CmdError, Ctx, TabId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::identity::{Identity, Kind, Os, problems};

const PATCH_JS: &str = include_str!("js/patch.js");
const PROBE_JS: &str = include_str!("js/probe.js");

/// What is applied to one tab right now.
#[derive(Debug, Clone)]
pub struct Applied {
    pub identity: Identity,
    /// `Page.addScriptToEvaluateOnNewDocument` identifier of our patch script.
    pub script: Option<String>,
    /// `navigator.hardwareConcurrency` before we overrode it (to restore it on clear).
    pub original_cores: Option<u64>,
    /// True for `device.emulate` (only UA, metrics and touch).
    pub device_only: bool,
}

/// Per-tab applied identities.
#[derive(Default)]
pub struct TabIdentities(Mutex<HashMap<TabId, Applied>>);

impl TabIdentities {
    pub fn get(&self, tab: TabId) -> Option<Applied> {
        self.lock().get(&tab).cloned()
    }

    pub fn set(&self, tab: TabId, applied: Applied) {
        self.lock().insert(tab, applied);
    }

    pub fn remove(&self, tab: TabId) -> Option<Applied> {
        self.lock().remove(&tab)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<TabId, Applied>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Which parts of an identity to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Everything (stealth).
    Fingerprint,
    /// UA, screen/viewport and touch only (responsive testing); keeps the real timezone,
    /// language and hardware.
    Device,
}

/// Result of [`apply`].
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct ApplyReport {
    /// CDP methods that succeeded, in order.
    pub applied: Vec<String>,
    /// Things that did not work or do not fit (the rest was still applied).
    pub warnings: Vec<String>,
}

/// One CDP call; failures become `CmdError`s.
async fn call(ctx: &Ctx, tab: TabId, method: &str, params: Value) -> Result<Value, CmdError> {
    Ok(ctx.backend.cdp(tab, method.to_string(), params).await?)
}

/// A CDP call whose failure is only a warning (older engines miss some methods). Tab and
/// shutdown errors still fail the command.
async fn try_call(
    ctx: &Ctx,
    tab: TabId,
    method: &str,
    params: Value,
    report: &mut ApplyReport,
) -> Result<Option<Value>, CmdError> {
    match ctx.backend.cdp(tab, method.to_string(), params).await {
        Ok(v) => {
            report.applied.push(method.to_string());
            Ok(Some(v))
        }
        Err(BackendError::Cdp(msg)) => {
            report.warnings.push(format!("{method} failed: {msg}"));
            Ok(None)
        }
        Err(other) => Err(other.into()),
    }
}

/// Timezone and locale overrides refuse to replace an existing one ("already in effect"):
/// clear first, then set.
async fn set_exclusive(
    ctx: &Ctx,
    tab: TabId,
    method: &str,
    params: Value,
    clear: Value,
    report: &mut ApplyReport,
) -> Result<(), CmdError> {
    match ctx
        .backend
        .cdp(tab, method.to_string(), params.clone())
        .await
    {
        Ok(_) => {
            report.applied.push(method.to_string());
            Ok(())
        }
        Err(BackendError::Cdp(msg)) if msg.contains("already") => {
            let _ = ctx.backend.cdp(tab, method.to_string(), clear).await;
            try_call(ctx, tab, method, params, report).await.map(|_| ())
        }
        Err(BackendError::Cdp(msg)) => {
            report.warnings.push(format!("{method} failed: {msg}"));
            Ok(())
        }
        Err(other) => Err(other.into()),
    }
}

/// `Network.setUserAgentOverride` parameters of a fingerprint identity (UA, platform,
/// Accept-Language, client hints). The CEF crate sends the same to worker targets.
pub fn user_agent_params(identity: &Identity) -> Value {
    let mut ua = json!({
        "userAgent": identity.user_agent,
        "platform": identity.platform,
        "acceptLanguage": identity.accept_language,
    });
    if let Some(meta) = ua_metadata_json(identity) {
        ua["userAgentMetadata"] = meta;
    }
    ua
}

/// CDP `UserAgentMetadata` for the identity's client hints.
fn ua_metadata_json(identity: &Identity) -> Option<Value> {
    let m = identity.ua_metadata.as_ref()?;
    let brands = |list: &[crate::identity::Brand]| -> Vec<Value> {
        list.iter()
            .map(|b| json!({ "brand": b.brand, "version": b.version }))
            .collect()
    };
    let full_version = m
        .full_version_list
        .iter()
        .find(|b| b.brand == "Google Chrome")
        .map(|b| b.version.clone())
        .unwrap_or_default();
    Some(json!({
        "brands": brands(&m.brands),
        "fullVersionList": brands(&m.full_version_list),
        "fullVersion": full_version,
        "platform": m.platform,
        "platformVersion": m.platform_version,
        "architecture": m.architecture,
        "model": m.model,
        "mobile": m.mobile,
        "bitness": m.bitness,
        "wow64": m.wow64,
    }))
}

/// `Network.setUserAgentOverride` parameters: user agent, `navigator.platform`, client hints
/// and (`with_language`) Accept-Language / `navigator.languages`. Also used for workers.
pub fn ua_override_params(identity: &Identity, with_language: bool) -> Value {
    let mut ua = json!({ "userAgent": identity.user_agent, "platform": identity.platform });
    if with_language {
        ua["acceptLanguage"] = json!(identity.accept_language);
    }
    if let Some(meta) = ua_metadata_json(identity) {
        ua["userAgentMetadata"] = meta;
    }
    ua
}

/// `screen.availWidth/availHeight` for a desktop identity: the screen minus the taskbar
/// (Windows 11: 48 px, Windows 10: 40 px) or the menu bar (macOS: 25 px). Under
/// `Emulation.setDeviceMetricsOverride` Chromium reports avail = screen size, which no real
/// desktop has. `None` for phones and tablets (they report the full screen).
pub fn available_screen(identity: &Identity) -> Option<(u32, u32)> {
    let bar = match identity.os {
        Os::Windows => {
            // UA-CH platformVersion 13+ = Windows 11.
            let major = identity
                .ua_metadata
                .as_ref()
                .and_then(|m| m.platform_version.split('.').next()?.parse::<u32>().ok())
                .unwrap_or(13);
            if major >= 13 { 48 } else { 40 }
        }
        Os::Macos => 25,
        Os::Android | Os::Ios => return None,
    };
    let s = identity.screen;
    Some((s.width, s.height.saturating_sub(bar)))
}

/// The patch script for a page (see `js/patch.js`). `patch_cores`: also patch
/// `hardwareConcurrency` (when the CDP override is missing).
pub fn patch_script(identity: &Identity, patch_cores: bool) -> String {
    let avail = available_screen(identity);
    let config = json!({
        "hardwareConcurrency": if patch_cores { json!(identity.hardware_concurrency) } else { Value::Null },
        "deviceMemory": identity.device_memory,
        "webglVendor": identity.webgl_vendor,
        "webglRenderer": identity.webgl_renderer,
        "availWidth": avail.map(|a| a.0),
        "availHeight": avail.map(|a| a.1),
        // Set by Network.setUserAgentOverride on pages; only workers need the JS fallback.
        "userAgent": Value::Null,
        "appVersion": Value::Null,
        "platform": Value::Null,
        "languages": Value::Null,
    });
    PATCH_JS.replace("__CONFIG__", &config.to_string())
}

/// The patch script for a worker (`WorkerNavigator`): everything as a fallback, because
/// the CDP overrides may not reach the worker (see [`crate::worker`]). Workers have no
/// `screen`, so no avail* values.
pub fn worker_patch_script(identity: &Identity) -> String {
    let languages: Vec<&str> = identity.accept_language.split(',').map(str::trim).collect();
    let config = json!({
        "hardwareConcurrency": identity.hardware_concurrency,
        "deviceMemory": identity.device_memory,
        "webglVendor": identity.webgl_vendor,
        "webglRenderer": identity.webgl_renderer,
        "availWidth": Value::Null,
        "availHeight": Value::Null,
        "userAgent": identity.user_agent,
        "appVersion": identity.user_agent.strip_prefix("Mozilla/").unwrap_or(&identity.user_agent),
        "platform": identity.platform,
        "languages": languages,
    });
    PATCH_JS.replace("__CONFIG__", &config.to_string())
}

/// The full version of the running engine ("156.0.7632.45"), when the backend can tell.
async fn engine_version(ctx: &Ctx, tab: TabId) -> Option<String> {
    let reply = ctx
        .backend
        .cdp(tab, "Browser.getVersion".into(), json!({}))
        .await
        .ok()?;
    let product = reply["product"].as_str()?;
    let version = product.rsplit('/').next()?;
    version
        .split('.')
        .all(|p| p.parse::<u32>().is_ok())
        .then(|| version.to_string())
}

/// Apply `identity` to `tab`. Replaces whatever this crate applied before.
pub async fn apply(
    ctx: &Ctx,
    tab: TabId,
    tabs: &TabIdentities,
    mut identity: Identity,
    scope: Scope,
) -> Result<ApplyReport, CmdError> {
    let mut report = ApplyReport::default();
    for p in problems(&identity) {
        report.warnings.push(format!("inconsistent identity: {p}"));
    }
    if scope == Scope::Fingerprint && identity.kind == Kind::DeviceEmulation {
        report.warnings.push(format!(
            "{} claims a non-Chromium browser: good for layout testing, not for stealth",
            identity.preset
        ));
    }
    if let Some(engine) = engine_version(ctx, tab).await
        && let Some(warning) = identity.adapt_to_engine(&engine)
    {
        report.warnings.push(warning);
    }
    let previous = tabs.get(tab);

    // 1. User agent + client hints (+ Accept-Language for stealth).
    let ua = ua_override_params(&identity, scope == Scope::Fingerprint);
    call(ctx, tab, "Network.setUserAgentOverride", ua).await?;
    report.applied.push("Network.setUserAgentOverride".into());

    // 2. Screen, viewport, mobile, touch.
    let (width, height) = identity
        .viewport
        .map(|v| (v.width, v.height))
        .unwrap_or((0, 0)); // 0 = keep the real window size
    let mut metrics = json!({
        "width": width,
        "height": height,
        "deviceScaleFactor": identity.screen.dpr,
        "mobile": identity.mobile,
        "screenWidth": identity.screen.width,
        "screenHeight": identity.screen.height,
    });
    if identity.mobile {
        metrics["screenOrientation"] = json!({ "type": "portraitPrimary", "angle": 0 });
    }
    try_call(
        ctx,
        tab,
        "Emulation.setDeviceMetricsOverride",
        metrics,
        &mut report,
    )
    .await?;
    let touch = if identity.touch {
        json!({ "enabled": true, "maxTouchPoints": identity.max_touch_points.clamp(1, 16) })
    } else {
        json!({ "enabled": false })
    };
    try_call(
        ctx,
        tab,
        "Emulation.setTouchEmulationEnabled",
        touch,
        &mut report,
    )
    .await?;

    let mut script = None;
    let mut original_cores = previous.as_ref().and_then(|p| p.original_cores);
    if scope == Scope::Fingerprint {
        // 3. Time, language, place.
        set_exclusive(
            ctx,
            tab,
            "Emulation.setTimezoneOverride",
            json!({ "timezoneId": identity.timezone }),
            json!({ "timezoneId": "" }),
            &mut report,
        )
        .await?;
        set_exclusive(
            ctx,
            tab,
            "Emulation.setLocaleOverride",
            json!({ "locale": identity.locale }),
            json!({}),
            &mut report,
        )
        .await?;
        if let Some(geo) = identity.geolocation {
            let params = json!({
                "latitude": geo.latitude, "longitude": geo.longitude, "accuracy": geo.accuracy,
            });
            try_call(
                ctx,
                tab,
                "Emulation.setGeolocationOverride",
                params,
                &mut report,
            )
            .await?;
        }

        // 4. CPU cores: engine-level override (also covers workers), JS patch as fallback.
        if original_cores.is_none() {
            original_cores = evaluate(ctx, tab, "navigator.hardwareConcurrency")
                .await
                .ok()
                .and_then(|v| v.as_u64());
        }
        let cores = json!({ "hardwareConcurrency": identity.hardware_concurrency });
        let patch_cores = try_call(
            ctx,
            tab,
            "Emulation.setHardwareConcurrencyOverride",
            cores,
            &mut report,
        )
        .await?
        .is_none();

        // 5. The rest in a script that runs before page scripts in every new document, and
        // right now in the current one (`runImmediately`).
        if let Some(old) = previous.as_ref().and_then(|p| p.script.clone()) {
            let _ = ctx
                .backend
                .cdp(
                    tab,
                    "Page.removeScriptToEvaluateOnNewDocument".into(),
                    json!({ "identifier": old }),
                )
                .await;
        }
        // New-document scripts only run while the Page domain is enabled (verified live: without
        // it the script was accepted but never ran). Unlike Runtime.enable, pages cannot see it.
        try_call(ctx, tab, "Page.enable", json!({}), &mut report).await?;
        let source = patch_script(&identity, patch_cores);
        let added = try_call(
            ctx,
            tab,
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source, "runImmediately": true }),
            &mut report,
        )
        .await?;
        script = added.and_then(|v| v["identifier"].as_str().map(str::to_string));
    } else if let Some(prev) = &previous
        && !prev.device_only
    {
        // Device emulation on top of a fingerprint keeps the fingerprint's other parts.
        script = prev.script.clone();
    }

    tabs.set(
        tab,
        Applied {
            identity,
            script,
            original_cores,
            device_only: scope == Scope::Device,
        },
    );
    Ok(report)
}

/// Remove everything [`apply`] set on the tab. JS patches stay in the current document until
/// it reloads.
pub async fn clear(
    ctx: &Ctx,
    tab: TabId,
    tabs: &TabIdentities,
    scope: Scope,
) -> Result<ApplyReport, CmdError> {
    let mut report = ApplyReport::default();
    let previous = tabs.remove(tab);
    // An empty user agent turns the override off.
    call(
        ctx,
        tab,
        "Network.setUserAgentOverride",
        json!({ "userAgent": "" }),
    )
    .await?;
    report.applied.push("Network.setUserAgentOverride".into());
    try_call(
        ctx,
        tab,
        "Emulation.clearDeviceMetricsOverride",
        json!({}),
        &mut report,
    )
    .await?;
    try_call(
        ctx,
        tab,
        "Emulation.setTouchEmulationEnabled",
        json!({ "enabled": false }),
        &mut report,
    )
    .await?;
    if scope == Scope::Fingerprint {
        try_call(
            ctx,
            tab,
            "Emulation.setTimezoneOverride",
            json!({ "timezoneId": "" }),
            &mut report,
        )
        .await?;
        try_call(
            ctx,
            tab,
            "Emulation.setLocaleOverride",
            json!({}),
            &mut report,
        )
        .await?;
        try_call(
            ctx,
            tab,
            "Emulation.clearGeolocationOverride",
            json!({}),
            &mut report,
        )
        .await?;
        if let Some(cores) = previous.as_ref().and_then(|p| p.original_cores) {
            let params = json!({ "hardwareConcurrency": cores });
            try_call(
                ctx,
                tab,
                "Emulation.setHardwareConcurrencyOverride",
                params,
                &mut report,
            )
            .await?;
        }
        if let Some(id) = previous.as_ref().and_then(|p| p.script.clone()) {
            let params = json!({ "identifier": id });
            try_call(
                ctx,
                tab,
                "Page.removeScriptToEvaluateOnNewDocument",
                params,
                &mut report,
            )
            .await?;
            report
                .warnings
                .push("JS patches stay active in the current page until it reloads".into());
        }
    }
    Ok(report)
}

/// `Runtime.evaluate` (no `Runtime.enable`), returning the value.
async fn evaluate(ctx: &Ctx, tab: TabId, expression: &str) -> Result<Value, CmdError> {
    let reply = call(
        ctx,
        tab,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
    )
    .await?;
    if let Some(details) = reply.get("exceptionDetails") {
        let text = details["exception"]["description"]
            .as_str()
            .or_else(|| details["text"].as_str())
            .unwrap_or("JavaScript exception");
        return Err(CmdError::internal(format!(
            "script in the page failed: {text}"
        )));
    }
    Ok(reply["result"]["value"].clone())
}

/// Result of one probe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Check {
    pub name: String,
    /// pass, warn or fail.
    pub status: String,
    pub detail: String,
}

/// Run the probes (`js/probe.js`) in the page. `expected` adds a "matches applied identity"
/// check.
pub async fn probe(
    ctx: &Ctx,
    tab: TabId,
    expected: Option<&Identity>,
) -> Result<Vec<Check>, CmdError> {
    let expected = match expected {
        Some(id) => {
            let mut v = serde_json::to_value(id).map_err(|e| CmdError::internal(e.to_string()))?;
            // What the patch script sets for screen.avail* (compared by the probe).
            v["screen_avail"] = match available_screen(id) {
                Some((w, h)) => json!({ "width": w, "height": h }),
                None => Value::Null,
            };
            v
        }
        None => Value::Null,
    };
    let source = PROBE_JS.replace("__EXPECTED__", &expected.to_string());
    let value = evaluate(ctx, tab, &source).await?;
    let checks = value
        .get("checks")
        .cloned()
        .ok_or_else(|| CmdError::internal(format!("probe returned no checks: {value}")))?;
    serde_json::from_value(checks).map_err(|e| CmdError::internal(format!("bad probe result: {e}")))
}

/// Fixed-width table: `PASS  navigator.webdriver           webdriver=false`.
pub fn table(checks: &[Check]) -> String {
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    checks
        .iter()
        .map(|c| {
            format!(
                "{:<4}  {:<width$}  {}",
                c.status.to_ascii_uppercase(),
                c.name,
                c.detail
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
