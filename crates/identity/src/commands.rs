//! The commands of this crate (all in the `stealth` toolset):
//!
//! * identities: `identity.presets`, `identity.generate`, `identity.list`, `identity.get`,
//!   `identity.remove`, `identity.assign`;
//! * tabs: `stealth.fingerprint_set`, `stealth.fingerprint_clear`, `stealth.check`,
//!   `device.emulate`, `device.clear`;
//! * proxies: `proxy.pool_add`, `proxy.pool_list`, `proxy.pool_check`, `proxy.pool_remove`,
//!   `proxy.next`, `proxy.forward_start`, `proxy.forward_stop`, `proxy.forward_list`,
//!   `geo.lookup`;
//! * routing and mass runs: `proxy.rules_set`, `proxy.rules_test`, `proxy.lease`,
//!   `proxy.release`, `identity.fit_to_proxy`.

use std::collections::BTreeMap;
use std::time::Duration;

use bmcp_core::command::Empty;
use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, ProfileId, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Services;
use crate::fit::{self, Change};
use crate::identity::{self, Identity, Kind, Os, Overrides, PRESETS, Screen};
use crate::proxy::route::{self, ProxyRef, Rule, RuleSet};
use crate::proxy::{Forwarder, GeoInfo, ProxyConfig, Strategy, forward, geo};
use crate::rng::Rng;
use crate::routing::{self, IDENTITY_ASSIGNED, ROUTES_CHANGED};
use crate::stealth::{self, ApplyReport, Scope};

/// Names: 1-64 chars of a-z, 0-9, '-' and '_' (same rule as profile ids).
fn check_name(what: &str, name: &str) -> Result<(), CmdError> {
    ProfileId::new(name).map(|_| ()).map_err(|_| {
        CmdError::invalid_params(format!(
            "{what} {name:?} is invalid: use 1-64 chars of a-z, 0-9, '-' or '_'"
        ))
    })
}

fn parse_proxy(url: &str) -> Result<ProxyConfig, CmdError> {
    url.parse::<ProxyConfig>().map_err(|e| {
        CmdError::invalid_params(e.to_string()).with_hint(
            "Use http://user:pass@host:port, socks5://user:pass@host:port or host:port:user:pass.",
        )
    })
}

fn not_found(message: String, hint: &str) -> CmdError {
    CmdError::new(ErrorCode::NotFound, message)
        .with_retry(bmcp_core::Retry::No)
        .with_hint(hint)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ================================================================== identity.presets

pub struct Presets;

#[derive(Debug, Serialize, JsonSchema)]
pub struct PresetInfo {
    pub name: &'static str,
    pub title: &'static str,
    pub os: Os,
    /// stealth, or device_emulation (claims Safari on a Chromium engine).
    pub kind: Kind,
    /// Name for device_emulate, e.g. "iPhone 15".
    pub device: Option<&'static str>,
    pub screen: Screen,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PresetsOutput {
    pub presets: Vec<PresetInfo>,
}

impl Command for Presets {
    const NAME: &'static str = "identity.presets";
    type Args = Empty;
    type Output = PresetsOutput;

    fn meta() -> Meta {
        Meta::new(
            "List the fingerprint presets (Windows/macOS Chrome desktops, Android phones, \
             iPhone/iPad device emulation) that identities are generated from.",
            Toolset::Stealth,
        )
    }

    async fn run(_ctx: Ctx, _args: Empty) -> Result<PresetsOutput, CmdError> {
        let presets = PRESETS
            .iter()
            .map(|p| PresetInfo {
                name: p.name,
                title: p.title,
                os: p.os,
                kind: p.kind,
                device: p.device,
                screen: p.screen,
            })
            .collect();
        Ok(PresetsOutput { presets })
    }
}

// ================================================================== identity.generate

pub struct Generate;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GenerateArgs {
    /// windows, macos, android or ios. Default: windows or macos (desktop Chrome).
    pub os: Option<Os>,
    /// Use this preset (see identity_presets) instead of a random one of the OS.
    pub preset: Option<String>,
    /// Same seed = same identity. Default: random.
    pub seed: Option<u64>,
    /// Id to save it under (default like "win-1a2b3c4d").
    pub id: Option<String>,
    /// Save to identities.json (default false = just return it).
    pub save: Option<bool>,
    /// IANA timezone override, e.g. "Europe/Berlin" (e.g. from geo_lookup of the proxy).
    pub timezone: Option<String>,
    /// Locale override, e.g. "de-DE".
    pub locale: Option<String>,
    /// Country code to pick timezone, locale and location from, e.g. "DE".
    pub country: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GenerateOutput {
    pub identity: Identity,
    /// The seed used (pass it again to get the same identity).
    pub seed: u64,
    pub saved: bool,
    /// Inconsistencies (empty for generated identities unless overrides clash).
    pub problems: Vec<String>,
}

impl Command for Generate {
    const NAME: &'static str = "identity.generate";
    type Args = GenerateArgs;
    type Output = GenerateOutput;

    fn meta() -> Meta {
        Meta::new(
            "Generate a consistent random fingerprint identity (UA + client hints + platform \
             + CPU/memory + screen + timezone/locale + GPU) from the presets; deterministic \
             by seed. save=true stores it in identities.json for stealth_fingerprint_set.",
            Toolset::Stealth,
        )
    }

    async fn run(ctx: Ctx, args: GenerateArgs) -> Result<GenerateOutput, CmdError> {
        let seed = args
            .seed
            .unwrap_or_else(|| Rng::from_time().next_u64() >> 16);
        let overrides = Overrides {
            timezone: args.timezone,
            locale: args.locale,
            country: args.country,
            latitude: args.latitude,
            longitude: args.longitude,
        };
        let mut identity = match &args.preset {
            Some(name) => {
                let p = identity::preset(name).ok_or_else(|| unknown_preset(name))?;
                if args.os.is_some_and(|os| os != p.os) {
                    return Err(CmdError::invalid_params(format!(
                        "preset {} is not os {:?}",
                        p.name,
                        args.os.unwrap()
                    )));
                }
                identity::generate_preset(p, seed, &overrides)
            }
            None => identity::generate(args.os, seed, &overrides),
        };
        if let Some(id) = args.id {
            check_name("identity id", &id)?;
            identity.id = id;
        }
        let problems = identity::problems(&identity);
        let saved = args.save.unwrap_or(false);
        if saved {
            let services = Services::from_ctx(&ctx)?;
            let copy = identity.clone();
            services.identities.update(|f| {
                f.identities.insert(copy.id.clone(), copy);
                Ok(())
            })?;
        }
        Ok(GenerateOutput {
            identity,
            seed,
            saved,
            problems,
        })
    }
}

fn unknown_preset(name: &str) -> CmdError {
    not_found(format!("no preset {name:?}"), "Call identity_presets.").with_candidates(
        PRESETS
            .iter()
            .map(|p| Value::String(p.name.to_string()))
            .collect(),
    )
}

// ================================================================== identity.list / get / remove / assign

pub struct List;

#[derive(Debug, Serialize, JsonSchema)]
pub struct IdentitySummary {
    pub id: String,
    pub preset: String,
    pub os: Os,
    pub kind: Kind,
    pub user_agent: String,
    pub timezone: String,
    pub locale: String,
    pub screen: Screen,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    pub identities: Vec<IdentitySummary>,
    /// Profile id → identity id.
    pub profiles: BTreeMap<String, String>,
}

impl Command for List {
    const NAME: &'static str = "identity.list";
    type Args = Empty;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List saved identities (identities.json) and which profile uses which.",
            Toolset::Stealth,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ListOutput, CmdError> {
        let file = Services::from_ctx(&ctx)?.identities.read()?;
        let identities = file
            .identities
            .values()
            .map(|i| IdentitySummary {
                id: i.id.clone(),
                preset: i.preset.clone(),
                os: i.os,
                kind: i.kind,
                user_agent: i.user_agent.clone(),
                timezone: i.timezone.clone(),
                locale: i.locale.clone(),
                screen: i.screen,
            })
            .collect();
        Ok(ListOutput {
            identities,
            profiles: file.profiles,
        })
    }
}

pub struct Get;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IdArgs {
    /// Identity id from identity_list.
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetOutput {
    pub identity: Identity,
}

impl Command for Get {
    const NAME: &'static str = "identity.get";
    type Args = IdArgs;
    type Output = GetOutput;

    fn meta() -> Meta {
        Meta::new(
            "Return one saved identity with all its values.",
            Toolset::Stealth,
        )
    }

    async fn run(ctx: Ctx, args: IdArgs) -> Result<GetOutput, CmdError> {
        let file = Services::from_ctx(&ctx)?.identities.read()?;
        Ok(GetOutput {
            identity: file.get(&args.id)?.clone(),
        })
    }
}

pub struct Remove;

#[derive(Debug, Serialize, JsonSchema)]
pub struct RemoveOutput {
    pub removed: bool,
}

impl Command for Remove {
    const NAME: &'static str = "identity.remove";
    type Args = IdArgs;
    type Output = RemoveOutput;

    fn meta() -> Meta {
        Meta::new(
            "Delete a saved identity (and its profile assignments).",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: IdArgs) -> Result<RemoveOutput, CmdError> {
        let (removed, unassigned) = Services::from_ctx(&ctx)?.identities.update(|f| {
            let unassigned: Vec<String> = f
                .profiles
                .iter()
                .filter(|(_, id)| **id == args.id)
                .map(|(profile, _)| profile.clone())
                .collect();
            f.profiles.retain(|_, id| *id != args.id);
            Ok((f.identities.remove(&args.id).is_some(), unassigned))
        })?;
        for profile in unassigned {
            ctx.events
                .emit(IDENTITY_ASSIGNED, json!({ "profile": profile, "id": null }));
        }
        Ok(RemoveOutput { removed })
    }
}

pub struct Assign;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignArgs {
    /// Profile id, e.g. "work".
    pub profile: String,
    /// Saved identity id; omit to remove the assignment.
    pub id: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AssignOutput {
    pub profile: String,
    pub id: Option<String>,
}

impl Command for Assign {
    const NAME: &'static str = "identity.assign";
    type Args = AssignArgs;
    type Output = AssignOutput;

    fn meta() -> Meta {
        Meta::new(
            "Give a profile a saved identity (stored in identities.json): every new tab and \
             popup of that profile gets it before its first navigation, workers included.",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: AssignArgs) -> Result<AssignOutput, CmdError> {
        check_name("profile", &args.profile)?;
        let services = Services::from_ctx(&ctx)?;
        services.identities.update(|f| {
            match &args.id {
                Some(id) => {
                    f.get(id)?;
                    f.profiles.insert(args.profile.clone(), id.clone());
                }
                None => {
                    f.profiles.remove(&args.profile);
                }
            }
            Ok(())
        })?;
        ctx.events.emit(
            IDENTITY_ASSIGNED,
            json!({ "profile": args.profile, "id": args.id }),
        );
        Ok(AssignOutput {
            profile: args.profile,
            id: args.id,
        })
    }
}

// ================================================================== stealth.fingerprint_set

pub struct FingerprintSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FingerprintSetArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// A saved identity (identity_list).
    pub identity_id: Option<String>,
    /// A full identity object (e.g. from identity_generate).
    pub identity: Option<Identity>,
    /// A preset name as-is (identity_presets).
    pub preset: Option<String>,
    /// Reload the page afterwards so the very first scripts already see the new values
    /// (default false; the patches are also applied to the current page immediately).
    pub reload: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FingerprintOutput {
    pub tab: TabId,
    pub identity_id: String,
    pub kind: Kind,
    #[serde(flatten)]
    pub report: ApplyReport,
}

impl Command for FingerprintSet {
    const NAME: &'static str = "stealth.fingerprint_set";
    type Args = FingerprintSetArgs;
    type Output = FingerprintOutput;

    fn meta() -> Meta {
        Meta::new(
            "Apply a fingerprint identity to a tab: user agent + client hints, \
             Accept-Language, timezone, locale, screen/DPR, touch, geolocation, CPU cores and \
             (by a minimal patch script) deviceMemory, webdriver and WebGL GPU strings. Give \
             identity_id, identity or preset. Check the result with stealth_check.",
            Toolset::Stealth,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &FingerprintSetArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: FingerprintSetArgs) -> Result<FingerprintOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let given = [
            args.identity_id.is_some(),
            args.identity.is_some(),
            args.preset.is_some(),
        ];
        if given.iter().filter(|g| **g).count() != 1 {
            return Err(CmdError::invalid_params(
                "give exactly one of identity_id, identity or preset",
            )
            .with_hint("identity_generate creates one; identity_presets lists presets."));
        }
        let identity = if let Some(id) = &args.identity_id {
            services.identities.read()?.get(id)?.clone()
        } else if let Some(identity) = args.identity {
            identity
        } else {
            let name = args.preset.as_deref().unwrap_or_default();
            let p = identity::preset(name).ok_or_else(|| unknown_preset(name))?;
            identity::from_preset(p, p.name)
        };
        let tab = ctx.resolve_tab(args.tab).await?;
        let (identity_id, kind) = (identity.id.clone(), identity.kind);
        let mut report =
            stealth::apply(&ctx, tab, &services.tabs, identity, Scope::Fingerprint).await?;
        if args.reload.unwrap_or(false) {
            ctx.backend
                .cdp(tab, "Page.reload".into(), json!({}))
                .await?;
            report.applied.push("Page.reload".into());
        }
        Ok(FingerprintOutput {
            tab,
            identity_id,
            kind,
            report,
        })
    }
}

// ================================================================== stealth.fingerprint_clear / device.clear

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TabArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ClearOutput {
    pub tab: TabId,
    #[serde(flatten)]
    pub report: ApplyReport,
}

pub struct FingerprintClear;

impl Command for FingerprintClear {
    const NAME: &'static str = "stealth.fingerprint_clear";
    type Args = TabArgs;
    type Output = ClearOutput;

    fn meta() -> Meta {
        Meta::new(
            "Remove the fingerprint overrides from a tab (back to the real browser values; \
             patch scripts disappear on the next page load).",
            Toolset::Stealth,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &TabArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: TabArgs) -> Result<ClearOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        let report = stealth::clear(&ctx, tab, &services.tabs, Scope::Fingerprint).await?;
        Ok(ClearOutput { tab, report })
    }
}

pub struct DeviceClear;

impl Command for DeviceClear {
    const NAME: &'static str = "device.clear";
    type Args = TabArgs;
    type Output = ClearOutput;

    fn meta() -> Meta {
        Meta::new(
            "Stop device emulation on a tab (user agent, screen size and touch back to normal).",
            Toolset::Stealth,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &TabArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: TabArgs) -> Result<ClearOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        let report = stealth::clear(&ctx, tab, &services.tabs, Scope::Device).await?;
        Ok(ClearOutput { tab, report })
    }
}

// ================================================================== device.emulate

pub struct DeviceEmulate;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceEmulateArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// "iPhone 15", "Pixel 8", "Galaxy S23", "iPad" (or any preset name).
    pub device: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeviceEmulateOutput {
    pub tab: TabId,
    pub preset: String,
    pub kind: Kind,
    #[serde(flatten)]
    pub report: ApplyReport,
}

impl Command for DeviceEmulate {
    const NAME: &'static str = "device.emulate";
    type Args = DeviceEmulateArgs;
    type Output = DeviceEmulateOutput;

    fn meta() -> Meta {
        Meta::new(
            "Emulate a phone or tablet in a tab (user agent, screen size, pixel ratio, touch) \
             for responsive testing. Keeps the real timezone and language. Not stealth: use \
             stealth_fingerprint_set for that.",
            Toolset::Stealth,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &DeviceEmulateArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: DeviceEmulateArgs) -> Result<DeviceEmulateOutput, CmdError> {
        let p = identity::preset(&args.device).ok_or_else(|| {
            not_found(
                format!("no device {:?}", args.device),
                "Use one of the candidates.",
            )
            .with_candidates(
                PRESETS
                    .iter()
                    .filter_map(|p| p.device)
                    .map(|d| Value::String(d.into()))
                    .collect(),
            )
        })?;
        let services = Services::from_ctx(&ctx)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        let identity = identity::from_preset(p, p.name);
        let report = stealth::apply(&ctx, tab, &services.tabs, identity, Scope::Device).await?;
        Ok(DeviceEmulateOutput {
            tab,
            preset: p.name.to_string(),
            kind: p.kind,
            report,
        })
    }
}

// ================================================================== stealth.check

pub struct Check;

#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckOutput {
    pub tab: TabId,
    /// The identity applied to the tab, if any (compared in "matches applied identity").
    pub identity_id: Option<String>,
    pub passed: usize,
    pub warned: usize,
    pub failed: usize,
    pub checks: Vec<stealth::Check>,
    /// The same as a table: `PASS  name  detail` per line.
    pub text: String,
}

impl Command for Check {
    const NAME: &'static str = "stealth.check";
    type Args = TabArgs;
    type Output = CheckOutput;

    fn meta() -> Meta {
        Meta::new(
            "Probe the page like a fingerprinting script: webdriver flag, UA vs client hints \
             vs platform, timezone vs Intl, languages, screen, WebGL strings, window.chrome, \
             permissions, CDP-detection tricks, worker consistency and the applied identity. \
             Returns a pass/warn/fail table.",
            Toolset::Stealth,
        )
    }

    async fn run(ctx: Ctx, args: TabArgs) -> Result<CheckOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let tab = ctx.resolve_tab(args.tab).await?;
        let applied = services.tabs.get(tab);
        // Device emulation does not try to fake everything: compare only stealth identities.
        let expected = applied
            .as_ref()
            .filter(|a| !a.device_only)
            .map(|a| &a.identity);
        let checks = stealth::probe(&ctx, tab, expected).await?;
        let count = |s: &str| checks.iter().filter(|c| c.status == s).count();
        Ok(CheckOutput {
            tab,
            identity_id: applied.map(|a| a.identity.id),
            passed: count("pass"),
            warned: count("warn"),
            failed: count("fail"),
            text: stealth::table(&checks),
            checks,
        })
    }
}

// ================================================================== proxy pools

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProxyInfo {
    /// URL with the password masked.
    pub proxy: String,
    /// null = never checked.
    pub healthy: Option<bool>,
    pub latency_ms: Option<u64>,
    /// Unix seconds of the last check.
    pub checked_at: Option<u64>,
    pub error: Option<String>,
}

fn proxy_info(e: &crate::proxy::ProxyEntry) -> ProxyInfo {
    ProxyInfo {
        proxy: e
            .url
            .parse::<ProxyConfig>()
            .map(|p| p.masked())
            .unwrap_or_else(|_| "(invalid)".into()),
        healthy: e.healthy,
        latency_ms: e.latency_ms,
        checked_at: e.checked_at,
        error: e.error.clone(),
    }
}

fn no_pool(name: &str, file: &crate::proxy::ProxiesFile) -> CmdError {
    not_found(
        format!("no proxy pool {name:?}"),
        "Create it with proxy_pool_add.",
    )
    .with_candidates(
        file.pools
            .keys()
            .map(|k| Value::String(k.clone()))
            .collect(),
    )
}

pub struct PoolAdd;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolAddArgs {
    /// Pool name, e.g. "residential-de" (created if new).
    pub pool: String,
    /// Proxy URLs: http://user:pass@host:port, socks5://user:pass@host:port,
    /// host:port or host:port:user:pass.
    pub proxies: Vec<String>,
    /// round_robin (default for new pools), random or sticky (same proxy per run id).
    pub strategy: Option<Strategy>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolAddOutput {
    pub pool: String,
    /// New proxies (duplicates are ignored).
    pub added: usize,
    pub total: usize,
    pub strategy: Strategy,
}

impl Command for PoolAdd {
    const NAME: &'static str = "proxy.pool_add";
    type Args = PoolAddArgs;
    type Output = PoolAddOutput;

    fn meta() -> Meta {
        Meta::new(
            "Add proxies to a named pool (created if new) in proxies.json, optionally setting \
             its rotation strategy (round_robin, random, sticky).",
            Toolset::Stealth,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: PoolAddArgs) -> Result<PoolAddOutput, CmdError> {
        check_name("pool", &args.pool)?;
        let urls = args
            .proxies
            .iter()
            .map(|u| parse_proxy(u).map(|p| p.url()))
            .collect::<Result<Vec<_>, _>>()?;
        let services = Services::from_ctx(&ctx)?;
        services.proxies.update(|f| {
            let pool = f.pools.entry(args.pool.clone()).or_default();
            if let Some(s) = args.strategy {
                pool.strategy = s;
            }
            let added = pool.add(urls);
            Ok(PoolAddOutput {
                pool: args.pool.clone(),
                added,
                total: pool.proxies.len(),
                strategy: pool.strategy,
            })
        })
    }
}

pub struct PoolList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolInfo {
    pub name: String,
    pub strategy: Strategy,
    pub proxies: Vec<ProxyInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolListOutput {
    pub pools: Vec<PoolInfo>,
}

impl Command for PoolList {
    const NAME: &'static str = "proxy.pool_list";
    type Args = Empty;
    type Output = PoolListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List proxy pools with their proxies (passwords masked) and last health check.",
            Toolset::Stealth,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<PoolListOutput, CmdError> {
        let file = Services::from_ctx(&ctx)?.proxies.read()?;
        let pools = file
            .pools
            .iter()
            .map(|(name, pool)| PoolInfo {
                name: name.clone(),
                strategy: pool.strategy,
                proxies: pool.proxies.iter().map(proxy_info).collect(),
            })
            .collect();
        Ok(PoolListOutput { pools })
    }
}

pub struct PoolCheck;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolCheckArgs {
    pub pool: String,
    /// host:port to open a tunnel to through each proxy (default "example.com:443").
    pub target: Option<String>,
    /// Per-proxy timeout in milliseconds (default 8000).
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolCheckOutput {
    pub pool: String,
    pub healthy: usize,
    pub unhealthy: usize,
    pub proxies: Vec<ProxyInfo>,
}

impl Command for PoolCheck {
    const NAME: &'static str = "proxy.pool_check";
    type Args = PoolCheckArgs;
    type Output = PoolCheckOutput;

    fn meta() -> Meta {
        Meta::new(
            "Health-check every proxy of a pool (parallel): open a tunnel through it to a \
             target and time it. Unhealthy proxies are skipped by rotation until the next \
             check.",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PoolCheckArgs) -> Result<PoolCheckOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let target = args.target.as_deref().unwrap_or("example.com:443");
        let (host, port) = target
            .rsplit_once(':')
            .and_then(|(h, p)| {
                Some((
                    h.trim_matches(['[', ']']).to_string(),
                    p.parse::<u16>().ok()?,
                ))
            })
            .ok_or_else(|| {
                CmdError::invalid_params(format!("target {target:?} is not host:port"))
            })?;
        let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(8000).clamp(100, 60_000));
        let file = services.proxies.read()?;
        let pool = file
            .pools
            .get(&args.pool)
            .ok_or_else(|| no_pool(&args.pool, &file))?;

        let mut tasks = tokio::task::JoinSet::new();
        for entry in &pool.proxies {
            let url = entry.url.clone();
            let host = host.clone();
            tasks.spawn(async move {
                let result = match url.parse::<ProxyConfig>() {
                    Ok(p) => forward::check(&p, &host, port, timeout).await,
                    Err(e) => Err(e.to_string()),
                };
                (url, result)
            });
        }
        let mut results = BTreeMap::new();
        while let Some(done) = tasks.join_next().await {
            if let Ok((url, result)) = done {
                results.insert(url, result);
            }
        }

        let now = unix_now();
        services.proxies.update(|f| {
            let pool = f
                .pools
                .get_mut(&args.pool)
                .ok_or_else(|| no_pool(&args.pool, &file))?;
            for entry in &mut pool.proxies {
                let Some(result) = results.get(&entry.url) else {
                    continue;
                };
                entry.checked_at = Some(now);
                match result {
                    Ok(latency) => {
                        entry.healthy = Some(true);
                        entry.latency_ms = Some(latency.as_millis() as u64);
                        entry.error = None;
                    }
                    Err(e) => {
                        entry.healthy = Some(false);
                        entry.latency_ms = None;
                        entry.error = Some(e.clone());
                    }
                }
            }
            let proxies: Vec<ProxyInfo> = pool.proxies.iter().map(proxy_info).collect();
            let healthy = proxies.iter().filter(|p| p.healthy == Some(true)).count();
            Ok(PoolCheckOutput {
                pool: args.pool.clone(),
                healthy,
                unhealthy: proxies.len() - healthy,
                proxies,
            })
        })
    }
}

pub struct PoolRemove;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PoolRemoveArgs {
    pub pool: String,
    /// One proxy to remove (its URL, masked or not); omit to delete the whole pool.
    pub proxy: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PoolRemoveOutput {
    pub removed: bool,
    /// Proxies left in the pool (0 when the pool was deleted).
    pub remaining: usize,
}

impl Command for PoolRemove {
    const NAME: &'static str = "proxy.pool_remove";
    type Args = PoolRemoveArgs;
    type Output = PoolRemoveOutput;

    fn meta() -> Meta {
        Meta::new(
            "Remove one proxy from a pool, or the whole pool.",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PoolRemoveArgs) -> Result<PoolRemoveOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        services.proxies.update(|f| {
            let Some(proxy) = &args.proxy else {
                let removed = f.pools.remove(&args.pool).is_some();
                return Ok(PoolRemoveOutput {
                    removed,
                    remaining: 0,
                });
            };
            let missing = no_pool(&args.pool, f);
            let pool = f.pools.get_mut(&args.pool).ok_or(missing)?;
            // Match by full URL, normalized URL or the masked form from proxy_pool_list.
            let wanted = proxy.parse::<ProxyConfig>().ok().map(|p| p.url());
            let url = pool
                .proxies
                .iter()
                .find(|e| {
                    Some(&e.url) == wanted.as_ref()
                        || e.url == *proxy
                        || e.url
                            .parse::<ProxyConfig>()
                            .is_ok_and(|p| p.masked() == *proxy)
                })
                .map(|e| e.url.clone());
            let removed = url.is_some_and(|u| pool.remove(&u));
            Ok(PoolRemoveOutput {
                removed,
                remaining: pool.proxies.len(),
            })
        })
    }
}

/// Pick a proxy from a pool (updating its rotation state).
fn pick_from_pool(
    services: &Services,
    pool: &str,
    run: Option<&str>,
) -> Result<ProxyConfig, CmdError> {
    let url = services.proxies.update(|f| {
        let missing = no_pool(pool, f);
        let p = f.pools.get_mut(pool).ok_or(missing)?;
        p.pick(run, &mut Rng::from_time()).ok_or_else(|| {
            CmdError::new(
                ErrorCode::NotFound,
                format!("pool {pool:?} has no healthy proxy"),
            )
            .with_retry(bmcp_core::Retry::No)
            .with_hint("Add proxies (proxy_pool_add) or re-check them (proxy_pool_check).")
        })
    })?;
    parse_proxy(&url)
}

/// `proxy` URL, or a pick from `pool`, or none (direct).
fn proxy_from_args(
    services: &Services,
    proxy: Option<&str>,
    pool: Option<&str>,
    run: Option<&str>,
) -> Result<Option<ProxyConfig>, CmdError> {
    match (proxy, pool) {
        (Some(_), Some(_)) => Err(CmdError::invalid_params("give proxy or pool, not both")),
        (Some(url), None) => parse_proxy(url).map(Some),
        (None, Some(pool)) => pick_from_pool(services, pool, run).map(Some),
        (None, None) => Ok(None),
    }
}

pub struct Next;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NextArgs {
    pub pool: String,
    /// Run id for sticky pools (same run → same proxy).
    pub run: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct NextOutput {
    /// The picked proxy, password masked.
    pub proxy: String,
}

impl Command for Next {
    const NAME: &'static str = "proxy.next";
    type Args = NextArgs;
    type Output = NextOutput;

    fn meta() -> Meta {
        Meta::new(
            "Pick the next proxy of a pool by its rotation strategy (skips unhealthy ones). \
             Returns it with the password masked; proxy_forward_start pool=... uses it.",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: NextArgs) -> Result<NextOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let proxy = pick_from_pool(&services, &args.pool, args.run.as_deref())?;
        Ok(NextOutput {
            proxy: proxy.masked(),
        })
    }
}

// ================================================================== proxy forwarders

pub struct ForwardStart;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardStartArgs {
    /// Upstream proxy URL (with user:pass).
    pub proxy: Option<String>,
    /// Or: pick the upstream from this pool.
    pub pool: Option<String>,
    /// Run id for sticky pools.
    pub run: Option<String>,
    /// Or: a proxy reference such as "lease:POOL/RUN/ROW" (from proxy_lease) or "pool:NAME".
    pub proxy_id: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ForwarderInfo {
    /// Id for proxy_forward_stop, e.g. "fwd-1".
    pub id: String,
    /// Local proxy without auth for Chromium, e.g. "socks5://127.0.0.1:53121".
    pub socks5: String,
    /// The same port as an HTTP proxy.
    pub http: String,
    /// Upstream, password masked.
    pub upstream: String,
}

fn forwarder_info(id: &str, f: &Forwarder) -> ForwarderInfo {
    ForwarderInfo {
        id: id.to_string(),
        socks5: f.socks5_url(),
        http: f.http_url(),
        upstream: f.upstream().masked(),
    }
}

impl Command for ForwardStart {
    const NAME: &'static str = "proxy.forward_start";
    type Args = ForwardStartArgs;
    type Output = ForwarderInfo;

    fn meta() -> Meta {
        Meta::new(
            "Start a local proxy on 127.0.0.1 (SOCKS5 and HTTP, no auth) that forwards through \
             an authenticated upstream proxy — Chromium cannot do SOCKS5 auth itself. Returns \
             the local address to configure as the browser proxy.",
            Toolset::Stealth,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: ForwardStartArgs) -> Result<ForwarderInfo, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let upstream = match &args.proxy_id {
            Some(_) if args.proxy.is_some() || args.pool.is_some() => {
                return Err(CmdError::invalid_params(
                    "give proxy, pool or proxy_id, not several",
                ));
            }
            Some(text) => services.upstream(&parse_ref(text)?, None)?,
            None => proxy_from_args(
                &services,
                args.proxy.as_deref(),
                args.pool.as_deref(),
                args.run.as_deref(),
            )?,
        }
        .ok_or_else(|| {
            CmdError::invalid_params("give proxy, pool or a proxy_id that is not direct")
        })?;
        let forwarder = Forwarder::start(upstream)
            .await
            .map_err(|e| CmdError::internal(format!("cannot listen on 127.0.0.1: {e}")))?;
        let id = services.next_forwarder_id();
        let info = forwarder_info(&id, &forwarder);
        services.forwarders().insert(id, forwarder);
        Ok(info)
    }
}

pub struct ForwardStop;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForwardStopArgs {
    /// Forwarder id from proxy_forward_start / proxy_forward_list.
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ForwardStopOutput {
    pub stopped: bool,
}

impl Command for ForwardStop {
    const NAME: &'static str = "proxy.forward_stop";
    type Args = ForwardStopArgs;
    type Output = ForwardStopOutput;

    fn meta() -> Meta {
        Meta::new(
            "Stop a local proxy forwarder (closes its open connections).",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ForwardStopArgs) -> Result<ForwardStopOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        // Dropping the forwarder stops it.
        let stopped = services.forwarders().remove(&args.id).is_some();
        Ok(ForwardStopOutput { stopped })
    }
}

pub struct ForwardList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ForwardListOutput {
    pub forwarders: Vec<ForwarderInfo>,
}

impl Command for ForwardList {
    const NAME: &'static str = "proxy.forward_list";
    type Args = Empty;
    type Output = ForwardListOutput;

    fn meta() -> Meta {
        Meta::new("List running local proxy forwarders.", Toolset::Stealth)
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ForwardListOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let forwarders = services
            .forwarders()
            .iter()
            .map(|(id, f)| forwarder_info(id, f))
            .collect();
        Ok(ForwardListOutput { forwarders })
    }
}

// ================================================================== geo.lookup

pub struct GeoLookup;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeoLookupArgs {
    /// Look up the exit IP of this proxy URL…
    pub proxy: Option<String>,
    /// …or of a proxy picked from this pool. Neither = this machine's IP.
    pub pool: Option<String>,
    /// Run id for sticky pools.
    pub run: Option<String>,
    /// Lookup endpoint (http:// only, ip-api.com JSON format). Default: ip-api.com.
    pub url: Option<String>,
    /// Timeout in milliseconds (default 10000).
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GeoLookupOutput {
    /// The proxy used (masked), or null for a direct lookup.
    pub via: Option<String>,
    pub geo: GeoInfo,
}

impl Command for GeoLookup {
    const NAME: &'static str = "geo.lookup";
    type Args = GeoLookupArgs;
    type Output = GeoLookupOutput;

    fn meta() -> Meta {
        Meta::new(
            "Find where an IP is (country, city, timezone, coordinates) — directly or through a \
             proxy — and suggest the locale/Accept-Language. Feed timezone/country into \
             identity_generate to fit a fingerprint to the proxy's IP.",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: GeoLookupArgs) -> Result<GeoLookupOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let via = proxy_from_args(
            &services,
            args.proxy.as_deref(),
            args.pool.as_deref(),
            args.run.as_deref(),
        )?;
        let url = args.url.as_deref().unwrap_or(geo::DEFAULT_URL);
        let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(10_000).clamp(100, 60_000));
        let info = geo::lookup(via.as_ref(), url, timeout).await.map_err(|e| {
            CmdError::new(ErrorCode::Internal, format!("geo lookup failed: {e}"))
                .with_retry(bmcp_core::Retry::AfterWait)
        })?;
        Ok(GeoLookupOutput {
            via: via.map(|p| p.masked()),
            geo: info,
        })
    }
}

// ================================================================== proxy.rules_set / rules_test

fn parse_ref(text: &str) -> Result<ProxyRef, CmdError> {
    ProxyRef::parse(text).map_err(|e| {
        CmdError::invalid_params(e).with_hint(
            "Use direct, fwd-N (proxy_forward_list), pool:NAME (proxy_pool_list), \
             lease:POOL/RUN/ROW (proxy_lease) or a proxy URL.",
        )
    })
}

pub struct RulesSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesSetArgs {
    /// Profile id, e.g. "work".
    pub profile: String,
    /// Checked in order; the first match wins. `match`: host glob ("*.example.com" also
    /// matches example.com), URL glob ("http://example.com/api/*") or "re:REGEX" (searched in
    /// the URL, case-insensitive). `via`: direct, fwd-N, pool:NAME, lease:POOL/RUN/ROW or a
    /// proxy URL. Note: for https URLs only scheme://host:port/ is visible to the rules.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Where everything else goes (default "direct"). No rules + direct = remove the routing.
    pub default: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesSetOutput {
    pub profile: String,
    /// The stored rules (passwords masked).
    pub rules: Vec<Rule>,
    pub default: String,
    /// The Chromium `proxy` preference for the profile's request context (the browser applies
    /// it; it is also sent as a proxy.routes_changed event).
    pub pref: Value,
    /// The generated PAC script (null when no script is needed).
    pub pac_script: Option<String>,
    /// Local forwarders the routes go through.
    pub forwarders: Vec<String>,
}

impl Command for RulesSet {
    const NAME: &'static str = "proxy.rules_set";
    type Args = RulesSetArgs;
    type Output = RulesSetOutput;

    fn meta() -> Meta {
        Meta::new(
            "Split routing for a profile: send matching sites through given proxies (or \
             direct) and the rest to a default. Generates a PAC script (FindProxyForURL) and \
             the browser proxy preference; proxies go through local forwarders, never with a \
             fallback to direct. Masked proxy URLs (user:***@) as returned by proxy_rules_get \
             keep their stored password. Test a URL with proxy_rules_test.",
            Toolset::Stealth,
        )
        .act()
        .secret()
    }

    async fn run(ctx: Ctx, args: RulesSetArgs) -> Result<RulesSetOutput, CmdError> {
        check_name("profile", &args.profile)?;
        let services = Services::from_ctx(&ctx)?;
        let stored = services.proxies.read()?.routes.get(&args.profile).cloned();
        let set = RuleSet {
            rules: args.rules,
            default: args.default.unwrap_or_else(|| "direct".into()),
        };
        // Masked proxy URLs from proxy_rules_get / profile_list keep the stored password.
        let set = match &stored {
            Some(stored) => keep_stored_passwords(set, stored),
            None => set,
        };
        // Resolves first: nothing is stored when a route cannot work (e.g. an empty pool).
        let (set, proxy) = services.set_routes(&args.profile, set).await?;
        ctx.events.emit(
            ROUTES_CHANGED,
            json!({ "profile": args.profile, "pref": proxy.pref }),
        );
        let masked = set.masked();
        Ok(RulesSetOutput {
            profile: args.profile,
            rules: masked.rules,
            default: masked.default,
            pref: proxy.pref,
            pac_script: proxy.pac_script,
            forwarders: proxy.forwarders,
        })
    }
}

/// Put the stored URL back for every route that is the masked form (`user:***@`) of a stored
/// one, so a UI can send back what `proxy.rules_get` showed without knowing the passwords.
fn keep_stored_passwords(mut set: RuleSet, stored: &RuleSet) -> RuleSet {
    let stored_vias = stored
        .rules
        .iter()
        .map(|r| r.via.as_str())
        .chain([stored.default.as_str()]);
    let real: BTreeMap<String, String> = stored_vias
        .filter_map(|via| {
            let masked = ProxyRef::parse(via).ok()?.to_string();
            (masked != via).then(|| (masked, via.to_string()))
        })
        .collect();
    let restore = |via: &mut String| {
        if let Some(original) = real.get(via.as_str()) {
            *via = original.clone();
        }
    };
    for rule in &mut set.rules {
        restore(&mut rule.via);
    }
    restore(&mut set.default);
    set
}

pub struct RulesGet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesGetArgs {
    /// One profile; omit for every profile that has routing.
    pub profile: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileRoutes {
    pub profile: String,
    /// Checked in order (passwords masked).
    pub rules: Vec<Rule>,
    /// Where everything else goes (the profile's proxy; "direct" = none).
    pub default: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesGetOutput {
    /// Profiles with a proxy or split routing (a profile without any is direct).
    pub routes: Vec<ProfileRoutes>,
}

impl Command for RulesGet {
    const NAME: &'static str = "proxy.rules_get";
    type Args = RulesGetArgs;
    type Output = RulesGetOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read the proxy routing of one profile or of all: the split rules \
             (proxy_rules_set) in order and the default route (its proxy), passwords masked.",
            Toolset::Stealth,
        )
    }

    async fn run(ctx: Ctx, args: RulesGetArgs) -> Result<RulesGetOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let file = services.proxies.read()?;
        let routes = file
            .routes
            .iter()
            .filter(|(profile, _)| args.profile.as_ref().is_none_or(|p| p == *profile))
            .map(|(profile, set)| {
                let masked = set.masked();
                ProfileRoutes {
                    profile: profile.clone(),
                    rules: masked.rules,
                    default: masked.default,
                }
            })
            .collect();
        Ok(RulesGetOutput { routes })
    }
}

pub struct RulesTest;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RulesTestArgs {
    pub profile: String,
    /// Full URL, e.g. "https://www.example.com/page".
    pub url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RulesTestOutput {
    pub profile: String,
    /// What the PAC script sees (https URLs are cut to scheme://host:port/).
    pub pac_url: String,
    pub host: String,
    /// 1-based number of the matching rule; null = the default route.
    pub rule: Option<usize>,
    /// The matching rule's pattern.
    pub matched: Option<String>,
    /// The route (passwords masked): direct, fwd-N, pool:NAME, lease:..., or a proxy URL.
    pub via: String,
    /// The upstream proxy behind the route when it is already known (masked); null for
    /// direct or a pool the profile has not used yet.
    pub upstream: Option<String>,
}

impl Command for RulesTest {
    const NAME: &'static str = "proxy.rules_test";
    type Args = RulesTestArgs;
    type Output = RulesTestOutput;

    fn meta() -> Meta {
        Meta::new(
            "Show which route a URL takes under a profile's proxy rules (proxy_rules_set): \
             the matching rule and the proxy, evaluated like the PAC script does.",
            Toolset::Stealth,
        )
    }

    async fn run(ctx: Ctx, args: RulesTestArgs) -> Result<RulesTestOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let file = services.proxies.read()?;
        let set = file.routes.get(&args.profile).cloned().unwrap_or_default();
        let parsed = set
            .parse()
            .map_err(|e| CmdError::internal(format!("stored rules: {e}")))?;
        let r = route::route(&parsed, &args.url).map_err(CmdError::invalid_params)?;
        // The upstream, without picking or leasing anything.
        let upstream = match &r.via {
            ProxyRef::Direct => None,
            ProxyRef::Url(p) => Some(p.masked()),
            ProxyRef::Forwarder(id) => services.forwarders().get(id).map(|f| f.upstream().masked()),
            ProxyRef::Pool(pool) => file
                .pools
                .get(pool)
                .and_then(|p| p.leased(&routing::profile_run(&args.profile), 0))
                .and_then(|u| u.parse::<ProxyConfig>().ok())
                .map(|p| p.masked()),
            ProxyRef::Lease { pool, run, row } => file
                .pools
                .get(pool)
                .and_then(|p| p.leased(run, *row))
                .and_then(|u| u.parse::<ProxyConfig>().ok())
                .map(|p| p.masked()),
        };
        Ok(RulesTestOutput {
            profile: args.profile,
            pac_url: r.pac_url,
            host: r.host,
            rule: r.rule.map(|i| i + 1),
            matched: r.rule.map(|i| set.rules[i].pattern.clone()),
            via: r.via.to_string(),
            upstream,
        })
    }
}

// ================================================================== identity.fit_to_proxy

pub struct FitToProxy;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FitToProxyArgs {
    /// Saved identity (identity_list).
    pub identity_id: String,
    /// The proxy whose exit IP to fit: fwd-N, pool:NAME, lease:POOL/RUN/ROW or a proxy URL
    /// (direct = this machine's IP).
    pub proxy_id: String,
    /// For pool:NAME: use the proxy this profile is routed through (its lease) instead of
    /// a fresh pick.
    pub profile: Option<String>,
    /// Save the changed identity (default true).
    pub save: Option<bool>,
    /// Lookup endpoint (http:// only, ip-api.com JSON format). Default: ip-api.com.
    pub url: Option<String>,
    /// Timeout in milliseconds (default 10000).
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FitToProxyOutput {
    pub identity_id: String,
    /// The proxy the lookup went through (masked), null = direct.
    pub via: Option<String>,
    pub geo: GeoInfo,
    /// What changed (empty = it already fit). The device (UA, platform, screen) never does.
    pub changes: Vec<Change>,
    pub saved: bool,
    pub identity: Identity,
}

impl Command for FitToProxy {
    const NAME: &'static str = "identity.fit_to_proxy";
    type Args = FitToProxyArgs;
    type Output = FitToProxyOutput;

    fn meta() -> Meta {
        Meta::new(
            "Fit a saved identity to a proxy's exit IP: geo lookup through the proxy, then set \
             timezone, locale + languages (Accept-Language) and geolocation (city with small \
             jitter) to match the country/region; keeps UA, platform and hardware. Reports \
             what changed; re-apply with stealth_fingerprint_set.",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: FitToProxyArgs) -> Result<FitToProxyOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let mut identity = services.identities.read()?.get(&args.identity_id)?.clone();
        let via = parse_ref(&args.proxy_id)?;
        if let Some(p) = &args.profile {
            check_name("profile", p)?;
        }
        let holder = args.profile.as_deref().map(routing::profile_run);
        let upstream = services.upstream(&via, holder.as_deref())?;
        let url = args.url.as_deref().unwrap_or(geo::DEFAULT_URL);
        let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(10_000).clamp(100, 60_000));
        let info = geo::lookup(upstream.as_ref(), url, timeout)
            .await
            .map_err(|e| {
                CmdError::new(ErrorCode::Internal, format!("geo lookup failed: {e}"))
                    .with_retry(bmcp_core::Retry::AfterWait)
            })?;
        let changes = fit::fit_to_geo(&mut identity, &info);
        let saved = args.save.unwrap_or(true) && !changes.is_empty();
        if saved {
            let copy = identity.clone();
            services.identities.update(|f| {
                f.identities.insert(copy.id.clone(), copy);
                Ok(())
            })?;
        }
        Ok(FitToProxyOutput {
            identity_id: args.identity_id,
            via: upstream.map(|p| p.masked()),
            geo: info,
            changes,
            saved,
            identity,
        })
    }
}

// ================================================================== proxy.lease / release

fn check_run(run: &str) -> Result<(), CmdError> {
    if route::valid_name(run) {
        Ok(())
    } else {
        Err(CmdError::invalid_params(format!(
            "run_id {run:?} is invalid: use 1-64 chars of a-z, A-Z, 0-9, '-', '_' or '.'"
        )))
    }
}

pub struct Lease;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeaseArgs {
    pub pool: String,
    /// The mass run, e.g. "run-42".
    pub run_id: String,
    /// Row number in the run.
    pub row: u64,
    /// Give the row a different proxy than it has now (e.g. its IP got blocked). Default
    /// false: sticky, the row keeps its proxy.
    pub rotate: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct LeaseOutput {
    pub pool: String,
    pub run_id: String,
    pub row: u64,
    /// The leased proxy, password masked.
    pub proxy: String,
    /// Reference for proxy_rules_set (via/default), identity_fit_to_proxy and
    /// proxy_forward_start: "lease:POOL/RUN/ROW".
    pub proxy_id: String,
    /// True when the row already had this proxy.
    pub reused: bool,
    /// Active leases on this proxy (including this one).
    pub load: usize,
}

impl Command for Lease {
    const NAME: &'static str = "proxy.lease";
    type Args = LeaseArgs;
    type Output = LeaseOutput;

    fn meta() -> Meta {
        Meta::new(
            "Lease a proxy of a pool to one row of a mass run: sticky per (run_id, row) — the \
             row keeps its proxy across retries — or rotate=true for a different one. New \
             leases go to the least-used healthy proxy. Release with proxy_release.",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: LeaseArgs) -> Result<LeaseOutput, CmdError> {
        check_name("pool", &args.pool)?;
        check_run(&args.run_id)?;
        let services = Services::from_ctx(&ctx)?;
        let (proxy, leased) = services.lease(
            &args.pool,
            &args.run_id,
            args.row,
            args.rotate.unwrap_or(false),
        )?;
        Ok(LeaseOutput {
            proxy: proxy.masked(),
            proxy_id: route::lease_ref(&args.pool, &args.run_id, args.row),
            reused: leased.reused,
            load: leased.load,
            pool: args.pool,
            run_id: args.run_id,
            row: args.row,
        })
    }
}

pub struct Release;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleaseArgs {
    pub pool: String,
    pub run_id: String,
    /// One row; omit to release every row of the run (when the run ends).
    pub row: Option<u64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ReleaseOutput {
    /// How many leases were released.
    pub released: usize,
}

impl Command for Release {
    const NAME: &'static str = "proxy.release";
    type Args = ReleaseArgs;
    type Output = ReleaseOutput;

    fn meta() -> Meta {
        Meta::new(
            "Release proxy leases of a mass run (one row, or the whole run).",
            Toolset::Stealth,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ReleaseArgs) -> Result<ReleaseOutput, CmdError> {
        let services = Services::from_ctx(&ctx)?;
        let released = services.release(&args.pool, &args.run_id, args.row)?;
        Ok(ReleaseOutput { released })
    }
}
