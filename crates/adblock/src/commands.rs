//! The commands: `adblock.update_lists`, `adblock.toggle`, `adblock.stats`, `adblock.check`,
//! `adblock.apply`, `simplify.site`, `simplify.get`, `simplify.list`, `simplify.reader`.

use std::collections::BTreeMap;

use bmcp_core::command::Empty;
use bmcp_core::{CmdError, Command, Ctx, Meta, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::ResourceType;
use crate::gate::{Evaluation, RequestInfo};
use crate::interim::{self, ApplyReport, InterimStats};
use crate::policy::{
    AdblockSettings, ContentSettings, ResolvedPolicy, SitePolicy, normalize_pattern,
};
use crate::reader::{self, ReaderOutput};
use crate::service::{AdblockService, EngineStatus, UpdateReport};
use crate::stats::TabStats;

/// Default for `max_chars` of `simplify.reader`.
const READER_DEFAULT_CHARS: usize = 4000;

/// Adblock settings as returned by the commands.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SettingsView {
    /// Filter lists are applied (site policies apply regardless).
    pub enabled: bool,
    /// Sites where filter lists are not applied.
    pub allowlist: Vec<String>,
    /// Enabled filter lists.
    pub lists: Vec<String>,
}

impl From<&AdblockSettings> for SettingsView {
    fn from(s: &AdblockSettings) -> Self {
        SettingsView {
            enabled: s.enabled,
            allowlist: s.allowlist.iter().cloned().collect(),
            lists: s.lists.clone(),
        }
    }
}

// ------------------------------------------------------------------ adblock.update_lists

pub struct UpdateLists;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateListsArgs {
    /// Download even if the server says nothing changed.
    pub force: Option<bool>,
    /// Replace the enabled lists, e.g. ["easylist", "easyprivacy", "ublock"].
    pub lists: Option<Vec<String>>,
}

impl Command for UpdateLists {
    const NAME: &'static str = "adblock.update_lists";
    type Args = UpdateListsArgs;
    type Output = UpdateReport;

    fn meta() -> Meta {
        Meta::new(
            "Download/update the filter lists (EasyList, EasyPrivacy, optional uBlock filters) \
             with ETag/Last-Modified checks and recompile the adblock engine. Returns the \
             status of each list.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: UpdateListsArgs) -> Result<UpdateReport, CmdError> {
        AdblockService::from_ctx(&ctx)
            .update_lists(args.force.unwrap_or(false), args.lists)
            .await
    }
}

// ------------------------------------------------------------------ adblock.toggle

pub struct Toggle;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToggleArgs {
    /// Site to switch (e.g. "example.com", also covers subdomains). Omit to switch globally.
    pub host: Option<String>,
    /// true = block ads (removes the site from the allowlist); false = allow ads.
    pub on: bool,
}

impl Command for Toggle {
    const NAME: &'static str = "adblock.toggle";
    type Args = ToggleArgs;
    type Output = SettingsView;

    fn meta() -> Meta {
        Meta::new(
            "Switch ad blocking on/off globally, or for one site (allowlist). Returns the \
             adblock settings.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ToggleArgs) -> Result<SettingsView, CmdError> {
        let settings = AdblockService::from_ctx(&ctx).toggle(args.host.as_deref(), args.on)?;
        Ok(SettingsView::from(&settings))
    }
}

// ------------------------------------------------------------------ adblock.stats

pub struct Stats;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatsArgs {
    /// Only this tab. Default: every tab.
    pub tab: Option<TabId>,
    /// Reset the counters after reading them.
    pub reset: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TabEntry {
    pub tab: TabId,
    #[serde(flatten)]
    pub stats: TabStats,
    /// Overhead of the interim CDP interception, when `adblock.apply` is on for this tab.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interception: Option<InterimStats>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StatsOutput {
    pub engine: EngineStatus,
    pub settings: SettingsView,
    pub tabs: Vec<TabEntry>,
    /// Sum over all tabs (only when no `tab` was given).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<TabStats>,
}

impl Command for Stats {
    const NAME: &'static str = "adblock.stats";
    type Args = StatsArgs;
    type Output = StatsOutput;

    fn meta() -> Meta {
        Meta::new(
            "Counters of blocked/allowed requests per tab (by reason, type and host), engine \
             status, and the overhead of adblock_apply interception.",
            Toolset::Data,
        )
    }

    async fn run(ctx: Ctx, args: StatsArgs) -> Result<StatsOutput, CmdError> {
        let service = AdblockService::from_ctx(&ctx);
        let interception = |tab: TabId| {
            let sessions = service.sessions.lock().unwrap_or_else(|e| e.into_inner());
            sessions
                .get(&tab)
                .map(|s| s.shared.stats(!s.task.is_finished()))
        };
        let (tabs, total) = match args.tab {
            Some(tab) => (
                vec![TabEntry {
                    tab,
                    stats: service.gate.stats.tab(tab),
                    interception: interception(tab),
                }],
                None,
            ),
            None => {
                let (per_tab, total) = service.gate.stats.all();
                let mut ids: Vec<TabId> = per_tab.keys().copied().collect();
                // Tabs with interception on but no requests yet are listed too.
                ids.extend(
                    service
                        .sessions
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .keys()
                        .copied(),
                );
                ids.sort();
                ids.dedup();
                let tabs = ids
                    .into_iter()
                    .map(|tab| TabEntry {
                        tab,
                        stats: per_tab.get(&tab).cloned().unwrap_or_default(),
                        interception: interception(tab),
                    })
                    .collect();
                (tabs, Some(total))
            }
        };
        if args.reset.unwrap_or(false) {
            service.gate.stats.reset(args.tab);
        }
        Ok(StatsOutput {
            engine: service.engine_status(),
            settings: SettingsView::from(&service.gate.state().settings),
            tabs,
            total,
        })
    }
}

// ------------------------------------------------------------------ adblock.check

pub struct Check;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckArgs {
    /// Request URL to test.
    pub url: String,
    /// Page that makes the request (default: the URL itself, i.e. first party).
    pub source_url: Option<String>,
    /// Resource type: script, image, stylesheet, font, media, xhr, sub_frame, main_frame,
    /// websocket, ping, other (default other).
    #[serde(rename = "type")]
    pub resource_type: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CheckOutput {
    pub resource_type: ResourceType,
    pub engine_loaded: bool,
    #[serde(flatten)]
    pub evaluation: Evaluation,
}

impl Command for Check {
    const NAME: &'static str = "adblock.check";
    type Args = CheckArgs;
    type Output = CheckOutput;

    fn meta() -> Meta {
        Meta::new(
            "Debug: would this request be blocked? Returns the decision (allow/block/redirect, \
             reason, rule), the filter-list verdict and the page's site policy. Not counted \
             in the stats.",
            Toolset::Data,
        )
    }

    async fn run(ctx: Ctx, args: CheckArgs) -> Result<CheckOutput, CmdError> {
        let resource_type = match args.resource_type.as_deref() {
            None => ResourceType::Other,
            Some(name) => ResourceType::parse(name).ok_or_else(|| {
                CmdError::invalid_params(format!("unknown resource type '{name}'"))
                    .with_hint("Use script, image, stylesheet, font, media, xhr, sub_frame, …")
            })?,
        };
        let service = AdblockService::from_ctx(&ctx);
        // Uses lists already on disk; never downloads (that is adblock_update_lists).
        let engine = service.ensure_engine(false).await?;
        let evaluation = service.gate.evaluate(&RequestInfo {
            tab: None,
            url: &args.url,
            source_url: args.source_url.as_deref().unwrap_or(""),
            resource_type,
        });
        Ok(CheckOutput {
            resource_type,
            engine_loaded: engine.loaded,
            evaluation,
        })
    }
}

// ------------------------------------------------------------------ adblock.apply

pub struct Apply;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// true = start blocking in this tab, false = stop.
    pub on: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApplyOutput {
    pub tab: TabId,
    /// Interception is running for the tab.
    pub on: bool,
    /// It was already running (nothing changed).
    pub already: bool,
    pub engine: EngineStatus,
    /// What was set up (only when just switched on).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ApplyReport>,
    /// Counters of the session that was stopped / is running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interception: Option<InterimStats>,
}

impl Command for Apply {
    const NAME: &'static str = "adblock.apply";
    type Args = ApplyArgs;
    type Output = ApplyOutput;

    fn meta() -> Meta {
        Meta::new(
            "Enforce adblock + site policies in a tab now: cosmetic CSS in an isolated world; \
             requests are blocked by the browser's network hook for every tab (report.network \
             = \"cef\"), or by CDP Fetch interception of this tab where that hook is missing \
             (\"cdp\"). Downloads the filter lists on first use. Reload the page to see the \
             full effect.",
            Toolset::Data,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &ApplyArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: ApplyArgs) -> Result<ApplyOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let service = AdblockService::from_ctx(&ctx);
        // A finished session (its tab navigated away from CDP or closed) counts as off.
        let existing = {
            let mut sessions = service.sessions.lock().unwrap_or_else(|e| e.into_inner());
            match sessions.get(&tab) {
                Some(s) if s.task.is_finished() => {
                    sessions.remove(&tab);
                    None
                }
                Some(s) => Some(s.shared.stats(true)),
                None => None,
            }
        };

        if !args.on {
            let session = service
                .sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&tab);
            let stats = session.as_ref().map(|s| s.shared.stats(false));
            let already = session.is_none();
            if let Some(session) = session {
                interim::stop(session).await;
            }
            return Ok(ApplyOutput {
                tab,
                on: false,
                already,
                engine: service.engine_status(),
                report: None,
                interception: stats,
            });
        }

        if let Some(stats) = existing {
            return Ok(ApplyOutput {
                tab,
                on: true,
                already: true,
                engine: service.engine_status(),
                report: None,
                interception: Some(stats),
            });
        }
        // Downloads the lists the first time; site policies work even if that fails.
        let engine = match service.ensure_engine(true).await {
            Ok(engine) => engine,
            Err(err) => {
                tracing::warn!(%err, "adblock: no filter lists; applying site policies only");
                service.engine_status()
            }
        };
        let share = bmcp_cdpshare::CdpShare::from_ctx(&ctx);
        let (session, report) =
            interim::start(ctx.backend.clone(), share, service.gate.clone(), tab)
                .await
                .map_err(CmdError::from)?;
        let stats = session.shared.stats(true);
        let replaced = service
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(tab, session);
        // Two concurrent `apply` calls: keep the newest session, stop the other one.
        if let Some(old) = replaced {
            old.task.abort();
        }
        Ok(ApplyOutput {
            tab,
            on: true,
            already: false,
            engine,
            report: Some(report),
            interception: Some(stats),
        })
    }
}

// ------------------------------------------------------------------ simplify.site

pub struct Site;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SiteArgs {
    /// Host pattern: "example.com" (with subdomains) or "*" for every site.
    pub host: String,
    /// Allow JavaScript.
    pub js: Option<bool>,
    /// Allow images.
    pub images: Option<bool>,
    /// Allow web fonts.
    pub fonts: Option<bool>,
    /// Allow audio/video.
    pub media: Option<bool>,
    /// Allow requests to other sites.
    pub third_party: Option<bool>,
    /// Allow CSS animations and transitions.
    pub animations: Option<bool>,
    /// Forget the stored policy of this host first (then apply the given fields).
    pub clear: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SiteOutput {
    /// Normalized pattern the policy is stored under.
    pub pattern: String,
    /// Stored fields for this pattern (unset = inherited).
    pub stored: SitePolicy,
    /// Effective policy for the host itself, after merging less specific patterns.
    pub effective: ResolvedPolicy,
    pub content_settings: ContentSettings,
}

impl Command for Site {
    const NAME: &'static str = "simplify.site";
    type Args = SiteArgs;
    type Output = SiteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Set what a site may load: js, images, fonts, media, third_party, animations \
             (true = allow, false = block; omitted = unchanged). Saved in simplify.json; takes \
             effect on the next request/navigation.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SiteArgs) -> Result<SiteOutput, CmdError> {
        let update = SitePolicy {
            js: args.js,
            images: args.images,
            fonts: args.fonts,
            media: args.media,
            third_party: args.third_party,
            animations: args.animations,
        };
        let service = AdblockService::from_ctx(&ctx);
        let (pattern, stored) =
            service.set_policy(&args.host, update, args.clear.unwrap_or(false))?;
        let host = if pattern == "*" { "" } else { pattern.as_str() };
        let effective = service.gate.state().policies.effective(host).0.resolve();
        Ok(SiteOutput {
            pattern,
            stored,
            effective,
            content_settings: effective.content_settings(),
        })
    }
}

// ------------------------------------------------------------------ simplify.get

pub struct Get;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetArgs {
    /// Host (or URL) of a page, e.g. "news.example.com".
    pub host: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetOutput {
    pub host: String,
    /// Patterns that apply, least specific first.
    pub matched: Vec<String>,
    /// Merged policy (true = allowed).
    pub policy: ResolvedPolicy,
    /// CEF content settings this maps to (applied by the CEF hook later).
    pub content_settings: ContentSettings,
    /// Filter lists apply on this host (adblock on and not allowlisted).
    pub adblock: bool,
}

impl Command for Get {
    const NAME: &'static str = "simplify.get";
    type Args = GetArgs;
    type Output = GetOutput;

    fn meta() -> Meta {
        Meta::new(
            "Effective simplification policy of a host (merged from matching patterns), its \
             CEF content settings and whether ad blocking applies there.",
            Toolset::Data,
        )
    }

    async fn run(ctx: Ctx, args: GetArgs) -> Result<GetOutput, CmdError> {
        let host = normalize_pattern(&args.host).map_err(CmdError::invalid_params)?;
        let state = AdblockService::from_ctx(&ctx).gate.state();
        let (policy, matched) = state.policies.effective(&host);
        let policy = policy.resolve();
        Ok(GetOutput {
            adblock: state.settings.enabled && !state.settings.is_allowlisted(&host),
            host,
            matched,
            content_settings: policy.content_settings(),
            policy,
        })
    }
}

// ------------------------------------------------------------------ simplify.list

pub struct List;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    /// Stored policies by host pattern.
    pub sites: BTreeMap<String, SitePolicy>,
    pub adblock: SettingsView,
}

impl Command for List {
    const NAME: &'static str = "simplify.list";
    type Args = Empty;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "All stored site simplification policies, plus the adblock settings (on/off, \
             allowlist, lists).",
            Toolset::Data,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ListOutput, CmdError> {
        let state = AdblockService::from_ctx(&ctx).gate.state();
        Ok(ListOutput {
            sites: state.policies.sites.clone(),
            adblock: SettingsView::from(&state.settings),
        })
    }
}

// ------------------------------------------------------------------ simplify.reader

pub struct Reader;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReaderArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Also replace the page with a clean readable version (dark/light aware).
    pub apply: Option<bool>,
    /// Max characters of text returned (default 4000).
    pub max_chars: Option<usize>,
}

impl Command for Reader {
    const NAME: &'static str = "simplify.reader";
    type Args = ReaderArgs;
    type Output = ReaderOutput;

    fn meta() -> Meta {
        Meta::new(
            "Reader mode (Mozilla Readability): the page's main article as {title, byline, \
             text, length}. With apply=true the page is replaced by a clean readable version.",
            Toolset::Core,
        )
        .mutates_tab()
        .palette("Reader mode")
    }

    fn target_tab(args: &ReaderArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: ReaderArgs) -> Result<ReaderOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let max = args.max_chars.unwrap_or(READER_DEFAULT_CHARS).max(100);
        reader::read(&ctx, tab, args.apply.unwrap_or(false), max).await
    }
}
