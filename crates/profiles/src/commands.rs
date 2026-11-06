//! `profile.*`, `cookies.*` and `permissions.*` commands (toolset `profile`).

use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, ProfileId, TabId, Toolset};
use bmcp_data::DataStore;
use bmcp_identity::{ProxyRef, Services};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::backend::CookieTarget;
use crate::cookies::CookieInfo;
use crate::permissions::{PermissionKind, PermissionRule, PermissionStore};
use crate::prefs::Prefs;
use crate::service::{ProfileConfig, ProfilesService};

fn check_id(id: &str) -> Result<(), CmdError> {
    ProfileId::new(id).map(|_| ()).map_err(|_| {
        CmdError::invalid_params(format!(
            "profile id {id:?} is invalid: use 1-64 chars of a-z, 0-9, '-' or '_'"
        ))
    })
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// The proxy part of a profile as shown by `profile.list` / `profile.create`.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct ProxyView {
    /// Default route of the profile's proxy routing (password masked); null = direct.
    pub proxy: Option<String>,
    /// Split routing rules (proxy_rules_set) besides the default.
    pub proxy_rules: usize,
    /// What Chromium was told: "direct", "fixed_servers" or "pac_script" (null = not applied
    /// yet).
    pub proxy_mode: Option<String>,
    /// The fixed proxy server Chromium uses: the local forwarder (socks5://127.0.0.1:p).
    pub proxy_server: Option<String>,
}

impl ProxyView {
    fn of(services: Option<&Services>, applied: Option<&Prefs>, id: &str) -> ProxyView {
        let routes = services.and_then(|s| s.routes(id).ok()).unwrap_or_default();
        let masked = routes.masked();
        ProxyView {
            proxy: (masked.default != "direct").then_some(masked.default),
            proxy_rules: routes.rules.len(),
            proxy_mode: applied.map(|p| p.proxy_mode().to_string()),
            proxy_server: applied.and_then(|p| p.proxy_server().map(str::to_string)),
        }
    }
}

// ------------------------------------------------------------------------------- profile.*

/// One profile in `profile.list`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ProfileEntry {
    pub id: String,
    #[serde(flatten)]
    pub proxy: ProxyView,
    pub accept_language: Option<String>,
    /// Identity applied to its new tabs (identity_assign).
    pub identity: Option<String>,
    /// Settings stored in profiles.json.
    pub stored: bool,
    /// The browser has it loaded (it has, or had, tabs).
    pub open: bool,
    /// In memory only.
    pub incognito: bool,
    pub tabs: Vec<TabId>,
    /// The profile the browser started with.
    pub launch: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    pub profiles: Vec<ProfileEntry>,
}

pub struct List;

impl Command for List {
    const NAME: &'static str = "profile.list";
    type Args = bmcp_core::command::Empty;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List browser profiles: stored settings (proxy, language), assigned identity, and \
             which are open with which tabs (incl. in-memory/incognito ones).",
            Toolset::Profile,
        )
    }

    async fn run(ctx: Ctx, _args: bmcp_core::command::Empty) -> Result<ListOutput, CmdError> {
        let service = ProfilesService::from_ctx(&ctx)?;
        let services = service.identities().ok();
        let stored = service.store.read()?.profiles;
        let assigned = services
            .as_ref()
            .and_then(|s| s.identities.read().ok())
            .map(|f| f.profiles)
            .unwrap_or_default();
        let open = match service.backend() {
            Ok(backend) => backend.open_profiles().await?,
            Err(_) => Vec::new(),
        };
        let mut ids: std::collections::BTreeSet<(String, bool)> = service
            .known_profiles()?
            .into_iter()
            .map(|k| (k, false))
            .collect();
        ids.extend(open.iter().map(|p| (p.id.clone(), p.incognito)));
        ids.insert((service.launch_profile.clone(), false));
        let profiles = ids
            .into_iter()
            .map(|(id, incognito)| {
                let config = stored.get(&id);
                let live = open.iter().find(|p| p.id == id && p.incognito == incognito);
                ProfileEntry {
                    proxy: ProxyView::of(services.as_deref(), service.applied(&id).as_ref(), &id),
                    accept_language: config.and_then(|c| c.accept_language.clone()),
                    identity: assigned.get(&id).cloned(),
                    stored: config.is_some(),
                    open: live.is_some(),
                    incognito,
                    tabs: live.map(|p| p.tabs.clone()).unwrap_or_default(),
                    launch: id == service.launch_profile && !incognito,
                    id,
                }
            })
            .collect();
        Ok(ListOutput { profiles })
    }
}

pub struct Create;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateArgs {
    /// Profile id: 1-64 chars of a-z, 0-9, '-', '_'. Creating an existing id updates it.
    pub id: String,
    /// Proxy for all of its traffic (the default of its proxy routing; split rules from
    /// proxy_rules_set stay): socks5://user:pass@host:port, http://user:pass@host:port,
    /// host:port:user:pass, pool:NAME (a pool proxy that stays the same for this profile),
    /// fwd-N or direct. Always through a local forwarder (passwords never reach the engine).
    /// "" or "direct" removes it.
    pub proxy: Option<String>,
    /// Hosts that skip a fixed proxy, e.g. "<local>;*.lan" (with split rules use a
    /// {match, via: "direct"} rule instead).
    pub proxy_bypass: Option<String>,
    /// Saved identity id (identity_list) applied to every new tab of this profile. "" removes it.
    pub identity: Option<String>,
    /// Accept-Language list, e.g. "de-DE,de,en". Default: the identity's.
    pub accept_language: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CreateOutput {
    pub id: String,
    /// True when it did not exist before.
    pub created: bool,
    #[serde(flatten)]
    pub proxy: ProxyView,
    pub identity: Option<String>,
    /// Folder of its cookies/cache.
    pub data_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

impl Command for Create {
    const NAME: &'static str = "profile.create";
    type Args = CreateArgs;
    type Output = CreateOutput;

    fn meta() -> Meta {
        Meta::new(
            "Create (or update) a browser profile: own cookies/cache folder, optional proxy \
             (through a local forwarder; WebRTC is then limited to the proxy; same setting as \
             the default of proxy_rules_set), identity applied to each new tab, \
             Accept-Language. Open tabs in it with tabs_open {profile}.",
            Toolset::Profile,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: CreateArgs) -> Result<CreateOutput, CmdError> {
        check_id(&args.id)?;
        let service = ProfilesService::from_ctx(&ctx)?;
        let services = service.identities()?;
        let empty_to_none = |v: &Option<String>| v.clone().filter(|s| !s.trim().is_empty());
        // Check everything before anything is written: no half-created profile.
        if let Some(proxy) = empty_to_none(&args.proxy) {
            ProxyRef::parse(&proxy).map_err(|e| {
                CmdError::invalid_params(format!("proxy: {e}")).with_hint(
                    "Use http://user:pass@host:port, socks5://user:pass@host:port, \
                     host:port:user:pass, pool:NAME, fwd-N or direct.",
                )
            })?;
        }
        if let Some(identity) = empty_to_none(&args.identity) {
            services.identities.read()?.get(&identity)?;
        }
        // The proxy resolves first (e.g. an empty pool fails here, nothing is stored).
        if let Some(proxy) = &args.proxy {
            service.set_proxy(&args.id, Some(proxy)).await?;
        }
        if let Some(identity) = &args.identity {
            services.identities.update(|f| {
                if identity.trim().is_empty() {
                    f.profiles.remove(&args.id);
                } else {
                    f.profiles.insert(args.id.clone(), identity.clone());
                }
                Ok(())
            })?;
        }
        let created = service.store.update(|file| {
            let created = !file.profiles.contains_key(&args.id);
            let entry = file
                .profiles
                .entry(args.id.clone())
                .or_insert_with(|| ProfileConfig {
                    created_ms: now_ms(),
                    ..Default::default()
                });
            if args.proxy_bypass.is_some() {
                entry.proxy_bypass = empty_to_none(&args.proxy_bypass);
            }
            if args.accept_language.is_some() {
                entry.accept_language = empty_to_none(&args.accept_language);
            }
            Ok(created)
        })?;
        let prefs = service.apply(&args.id).await?;
        let config = service
            .store
            .read()?
            .profiles
            .get(&args.id)
            .cloned()
            .unwrap_or_default();
        let warning =
            (config.proxy_bypass.is_some() && prefs.proxy_mode() == "pac_script").then(|| {
                "proxy_bypass only applies to a fixed proxy; this profile has split rules \
                 (proxy_rules_set): add {match, via: \"direct\"} rules instead"
                    .to_string()
            });
        let identity = services
            .identities
            .read()
            .ok()
            .and_then(|f| f.profiles.get(&args.id).cloned());
        Ok(CreateOutput {
            proxy: ProxyView::of(Some(&services), Some(&prefs), &args.id),
            identity,
            data_dir: service.profile_dir(&args.id).display().to_string(),
            warning,
            created,
            id: args.id,
        })
    }
}

pub struct Delete;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteArgs {
    pub id: String,
    /// Keep its folder (cookies, cache, history of the engine). Default false: deleted.
    pub keep_data: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DeleteOutput {
    pub id: String,
    /// Its settings were stored (and are now removed).
    pub removed: bool,
    /// Its data folder was deleted.
    pub data_deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

impl Command for Delete {
    const NAME: &'static str = "profile.delete";
    type Args = DeleteArgs;
    type Output = DeleteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Delete a profile: its settings, proxy routing (and pool leases), identity \
             assignment and (unless keep_data) its cookies/cache folder. Close its tabs first.",
            Toolset::Profile,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: DeleteArgs) -> Result<DeleteOutput, CmdError> {
        check_id(&args.id)?;
        let service = ProfilesService::from_ctx(&ctx)?;
        if args.id == service.launch_profile {
            return Err(CmdError::invalid_params(format!(
                "{:?} is the profile the browser runs with; it cannot be deleted",
                args.id
            )));
        }
        if let Ok(backend) = service.backend() {
            backend.forget(args.id.clone()).await.map_err(|e| {
                CmdError::from(e).with_hint("Close the profile's tabs (profile_list shows them).")
            })?;
        }
        service.forget_applied(&args.id);
        let removed = service
            .store
            .update(|f| Ok(f.profiles.remove(&args.id).is_some()))?;
        if let Ok(services) = service.identities() {
            // Routing, leases of its pool routes, identity assignment (the identity stays).
            services.forget_profile(&args.id, false)?;
        }
        let mut warning = None;
        let mut data_deleted = false;
        let dir = service.profile_dir(&args.id);
        if !args.keep_data.unwrap_or(false) && dir.exists() {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => data_deleted = true,
                Err(e) => {
                    warning = Some(format!(
                        "could not delete {} ({e}); the engine may still hold files until restart",
                        dir.display()
                    ))
                }
            }
        }
        Ok(DeleteOutput {
            id: args.id,
            removed,
            data_deleted,
            warning,
        })
    }
}

// ------------------------------------------------------------------------------- cookies.*

/// Which profile's cookie jar.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JarArgs {
    /// Profile id; default: the profile the browser started with.
    pub profile: Option<String>,
    /// The in-memory (incognito) profile of that name.
    pub incognito: Option<bool>,
}

impl JarArgs {
    fn target(&self) -> Result<CookieTarget, CmdError> {
        target(&self.profile, self.incognito)
    }
}

fn target(profile: &Option<String>, incognito: Option<bool>) -> Result<CookieTarget, CmdError> {
    if let Some(p) = profile {
        check_id(p)?;
    }
    Ok(CookieTarget {
        profile: profile.clone(),
        incognito: incognito.unwrap_or(false),
    })
}

pub struct CookiesGet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CookiesGetArgs {
    /// Profile id; default: the profile the browser started with.
    pub profile: Option<String>,
    /// The in-memory (incognito) profile of that name.
    pub incognito: Option<bool>,
    /// Only cookies of this domain and its subdomains, e.g. "example.com".
    pub domain: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CookiesOutput {
    pub cookies: Vec<CookieInfo>,
}

impl Command for CookiesGet {
    const NAME: &'static str = "cookies.get";
    type Args = CookiesGetArgs;
    type Output = CookiesOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read a profile's cookies (values included), optionally of one domain.",
            Toolset::Profile,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: CookiesGetArgs) -> Result<CookiesOutput, CmdError> {
        let backend = ProfilesService::from_ctx(&ctx)?.backend()?;
        let cookies = backend
            .cookies(target(&args.profile, args.incognito)?, args.domain)
            .await?;
        Ok(CookiesOutput { cookies })
    }
}

pub struct CookiesSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CookiesSetArgs {
    /// Profile id; default: the profile the browser started with.
    pub profile: Option<String>,
    /// The in-memory (incognito) profile of that name.
    pub incognito: Option<bool>,
    /// Cookies to set (same shape as cookies_get returns; path defaults to "/").
    pub cookies: Vec<CookieInfo>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CookiesSetOutput {
    pub set: usize,
    /// Names of cookies the store refused (bad domain, secure cookie on http, …).
    pub failed: Vec<String>,
}

impl Command for CookiesSet {
    const NAME: &'static str = "cookies.set";
    type Args = CookiesSetArgs;
    type Output = CookiesSetOutput;

    fn meta() -> Meta {
        Meta::new("Set cookies in a profile's cookie jar.", Toolset::Profile).secret()
    }

    async fn run(ctx: Ctx, args: CookiesSetArgs) -> Result<CookiesSetOutput, CmdError> {
        let backend = ProfilesService::from_ctx(&ctx)?.backend()?;
        let names: Vec<String> = args
            .cookies
            .iter()
            .map(|c| format!("{}@{}", c.name, c.domain))
            .collect();
        let results = backend
            .set_cookies(target(&args.profile, args.incognito)?, args.cookies)
            .await?;
        let failed: Vec<String> = names
            .into_iter()
            .zip(&results)
            .filter(|(_, ok)| !**ok)
            .map(|(n, _)| n)
            .collect();
        Ok(CookiesSetOutput {
            set: results.len() - failed.len(),
            failed,
        })
    }
}

pub struct CookiesDelete;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CookiesDeleteArgs {
    /// Profile id; default: the profile the browser started with.
    pub profile: Option<String>,
    /// The in-memory (incognito) profile of that name.
    pub incognito: Option<bool>,
    /// Only this domain and its subdomains. Without domain and name: ALL cookies.
    pub domain: Option<String>,
    /// Only cookies with this name.
    pub name: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CookiesDeleteOutput {
    pub deleted: usize,
}

impl Command for CookiesDelete {
    const NAME: &'static str = "cookies.delete";
    type Args = CookiesDeleteArgs;
    type Output = CookiesDeleteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Delete cookies of a profile: of one domain and/or with one name, or all.",
            Toolset::Profile,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: CookiesDeleteArgs) -> Result<CookiesDeleteOutput, CmdError> {
        let backend = ProfilesService::from_ctx(&ctx)?.backend()?;
        let deleted = backend
            .delete_cookies(
                target(&args.profile, args.incognito)?,
                args.domain,
                args.name,
            )
            .await?;
        Ok(CookiesDeleteOutput { deleted })
    }
}

pub struct CookiesApplyPending;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApplyPendingOutput {
    /// Cookies now in the cookie jar (and removed from pending_cookies).
    pub applied: usize,
    /// Cookies the jar refused (kept in pending_cookies).
    pub failed: usize,
}

impl Command for CookiesApplyPending {
    const NAME: &'static str = "cookies.apply_pending";
    type Args = JarArgs;
    type Output = ApplyPendingOutput;

    fn meta() -> Meta {
        Meta::new(
            "Move cookies waiting from import_chrome (cookies JSON) into a profile's cookie jar. \
             Runs by itself at startup and after each import.",
            Toolset::Profile,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: JarArgs) -> Result<ApplyPendingOutput, CmdError> {
        let backend = ProfilesService::from_ctx(&ctx)?.backend()?;
        let store = DataStore::from_ctx(&ctx)?;
        apply_pending(&store, backend.as_ref(), args.target()?).await
    }
}

/// Push `pending_cookies` into `target`'s cookie jar and delete the accepted ones.
pub async fn apply_pending(
    store: &DataStore,
    backend: &dyn crate::backend::ProfileBackend,
    target: CookieTarget,
) -> Result<ApplyPendingOutput, CmdError> {
    let pending = store
        .with(|c| bmcp_data::import::list_pending_cookies(c))
        .map_err(|e| CmdError::internal(e.to_string()))?;
    if pending.is_empty() {
        return Ok(ApplyPendingOutput {
            applied: 0,
            failed: 0,
        });
    }
    let cookies = pending.iter().map(CookieInfo::from).collect();
    let results = backend.set_cookies(target, cookies).await?;
    let accepted: Vec<i64> = pending
        .iter()
        .zip(&results)
        .filter(|(_, ok)| **ok)
        .map(|(c, _)| c.id)
        .collect();
    store
        .with(|c| bmcp_data::import::delete_pending_cookies(c, &accepted))
        .map_err(|e| CmdError::internal(e.to_string()))?;
    Ok(ApplyPendingOutput {
        applied: accepted.len(),
        failed: pending.len() - accepted.len(),
    })
}

// --------------------------------------------------------------------------- permissions.*

/// The permission rules (shared with the browser's permission handler).
fn permission_store(ctx: &Ctx) -> Result<PermissionStore, CmdError> {
    ctx.extensions
        .get::<PermissionStore>()
        .map(|s| (*s).clone())
        .ok_or_else(|| {
            CmdError::new(
                ErrorCode::NotImplemented,
                "permissions need the real browser",
            )
        })
}

pub struct PermissionsSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PermissionsSetArgs {
    /// Site origin, e.g. "https://meet.example.com", or "*" for every site.
    pub origin: String,
    /// camera, microphone, screen_capture, geolocation, notifications, clipboard, midi,
    /// storage_access, local_fonts, idle_detection, window_management, file_system,
    /// multiple_downloads, pointer_lock, keyboard_lock, local_network, sensors, other.
    pub kind: PermissionKind,
    /// true = allow, false = deny, null/omitted = remove the rule (back to: deny).
    pub allow: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PermissionsOutput {
    /// All rules. Requests without a rule are denied.
    pub rules: Vec<PermissionRule>,
}

impl Command for PermissionsSet {
    const NAME: &'static str = "permissions.set";
    type Args = PermissionsSetArgs;
    type Output = PermissionsOutput;

    fn meta() -> Meta {
        Meta::new(
            "Allow or deny a permission (camera, microphone, geolocation, notifications, …) for \
             a site; without a rule every permission request of a web page is denied.",
            Toolset::Profile,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PermissionsSetArgs) -> Result<PermissionsOutput, CmdError> {
        let store = permission_store(&ctx)?;
        if args.origin.trim().is_empty() {
            return Err(CmdError::invalid_params("origin must not be empty"));
        }
        store.set(&args.origin, args.kind, args.allow)?;
        Ok(PermissionsOutput {
            rules: store.rules(),
        })
    }
}

pub struct PermissionsList;

impl Command for PermissionsList {
    const NAME: &'static str = "permissions.list";
    type Args = bmcp_core::command::Empty;
    type Output = PermissionsOutput;

    fn meta() -> Meta {
        Meta::new(
            "List the stored site permission rules (everything else is denied).",
            Toolset::Profile,
        )
    }

    async fn run(
        ctx: Ctx,
        _args: bmcp_core::command::Empty,
    ) -> Result<PermissionsOutput, CmdError> {
        Ok(PermissionsOutput {
            rules: permission_store(&ctx)?.rules(),
        })
    }
}
