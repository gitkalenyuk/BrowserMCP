//! `browsing_data.clear`: Settings → Data & storage → Clear browsing data.
//!
//! Every part uses the mechanism that really owns the data:
//!
//! | part | how |
//! |---|---|
//! | history | `browsermcp.sqlite` visits in the time range (shared by every profile) |
//! | downloads | the downloads list in `browsermcp.sqlite` (files stay on disk) |
//! | cookies | the profile's CEF cookie manager; with a time range: cookies of the sites visited in it |
//! | cache | CDP `Network.clearBrowserCache` in a tab of the profile (always the whole cache) |
//! | site data | CDP `Storage.clearDataForOrigin` (local storage, IndexedDB, service workers, cache storage, …) for the sites visited in the time range, plus, for "all time", every site with cookies or IndexedDB data |
//!
//! Cache and site data need a page of that profile for CDP: an open tab of the profile, or an
//! off-screen `about:blank` tab that is closed again afterwards. One part failing does not
//! stop the others; failures come back in `errors`.

use std::path::Path;

use bmcp_core::{
    BackendResult, BrowserBackend, CmdError, Command, Ctx, Meta, OpenTabOptions, TabId, Toolset,
    WindowTarget,
};
use bmcp_profiles::{CookieTarget, ProfilesService};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What `Storage.clearDataForOrigin` removes for "site data" (cookies are their own part).
pub const STORAGE_TYPES: &str =
    "file_systems,indexeddb,local_storage,shader_cache,websql,service_workers,cache_storage";

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClearArgs {
    /// Only data from this time on (Unix ms); omitted = all time.
    pub since: Option<i64>,
    /// Profile whose cookies, cache and site data are cleared (default: the one the browser
    /// started with). History and downloads are shared by all profiles.
    pub profile: Option<String>,
    /// Browsing history (visits in the time range).
    #[serde(default)]
    pub history: bool,
    /// The downloads list (the files stay on disk).
    #[serde(default)]
    pub downloads: bool,
    /// Cookies (signs you out of sites).
    #[serde(default)]
    pub cookies: bool,
    /// Cached images and files (always the whole cache).
    #[serde(default)]
    pub cache: bool,
    /// Site data: local storage, IndexedDB, service workers, cache storage, file systems.
    #[serde(default)]
    pub site_data: bool,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct ClearOutput {
    /// The profile that was cleared.
    pub profile: String,
    /// History visits removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_visits: Option<usize>,
    /// Downloads list entries removed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<usize>,
    /// Cookies deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookies: Option<usize>,
    /// The HTTP cache was cleared.
    pub cache: bool,
    /// Sites whose stored data was cleared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_data_origins: Option<usize>,
    /// Parts that failed: {part, error}.
    pub errors: Vec<PartError>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PartError {
    /// history | downloads | cookies | cache | site_data
    pub part: String,
    pub error: String,
}

pub struct Clear;

impl Command for Clear {
    const NAME: &'static str = "browsing_data.clear";
    type Args = ClearArgs;
    type Output = ClearOutput;

    fn meta() -> Meta {
        Meta::new(
            "Clear browsing data: history, downloads (list), cookies, cache, site_data (local \
             storage, IndexedDB, service workers…), each true/false; since = only from this \
             time (Unix ms, omitted = all time); profile = whose cookies/cache/site data \
             (default: the launch profile). Returns counts per part and {errors} for parts \
             that failed.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ClearArgs) -> Result<ClearOutput, CmdError> {
        if !(args.history || args.downloads || args.cookies || args.cache || args.site_data) {
            return Err(CmdError::invalid_params(
                "choose at least one of history, downloads, cookies, cache, site_data",
            ));
        }
        let profiles = ProfilesService::from_ctx(&ctx).ok();
        let launch = profiles
            .as_ref()
            .map(|s| s.launch_profile.clone())
            .unwrap_or_else(|| bmcp_core::ProfileId::DEFAULT.to_string());
        let profile = args
            .profile
            .clone()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| launch.clone());
        let mut out = ClearOutput {
            profile: profile.clone(),
            ..Default::default()
        };
        let fail = |out: &mut ClearOutput, part: &str, err: String| {
            tracing::warn!(part, %err, "clearing browsing data failed");
            out.errors.push(PartError {
                part: part.into(),
                error: err,
            });
        };
        let store = bmcp_data::store::DataStore::from_ctx(&ctx);

        // The sites visited in the range, read before the history goes.
        let mut visited = Vec::new();
        if (args.cookies || args.site_data)
            && let Ok(store) = &store
        {
            let since = args.since;
            match store
                .run(move |c| bmcp_data::history::visited_origins(c, since))
                .await
            {
                Ok(origins) => visited = origins,
                Err(err) => tracing::warn!(%err, "visited sites not read"),
            }
        }

        if args.history {
            match &store {
                Ok(store) => {
                    let filter = bmcp_data::history::VisitFilter {
                        since: args.since,
                        ..Default::default()
                    };
                    match store
                        .run(move |c| bmcp_data::history::delete_visits(c, &filter))
                        .await
                    {
                        Ok(deleted) => {
                            out.history_visits = Some(deleted.visits);
                            bmcp_data::events::history_changed(&ctx, "delete");
                        }
                        Err(err) => fail(&mut out, "history", err.to_string()),
                    }
                }
                Err(err) => fail(&mut out, "history", err.message.clone()),
            }
        }
        if args.downloads {
            match &store {
                Ok(store) => {
                    let since = args.since;
                    match store
                        .run(move |c| bmcp_data::downloads::clear(c, since))
                        .await
                    {
                        Ok(n) => out.downloads = Some(n),
                        Err(err) => fail(&mut out, "downloads", err.to_string()),
                    }
                }
                Err(err) => fail(&mut out, "downloads", err.message.clone()),
            }
        }

        let target = CookieTarget {
            profile: Some(profile.clone()),
            incognito: false,
        };
        let cookie_backend = profiles.as_ref().map(|s| s.backend());
        // Sites with cookies count as "sites with data" for an all-time site data clear.
        let mut cookie_origins = Vec::new();
        if args.site_data
            && args.since.is_none()
            && let Some(Ok(backend)) = &cookie_backend
            && let Ok(cookies) = backend.cookies(target.clone(), None).await
        {
            cookie_origins = cookies
                .iter()
                .flat_map(|c| origins_of_domain(&c.domain))
                .collect();
        }
        if args.cookies {
            match &cookie_backend {
                Some(Ok(backend)) => {
                    match clear_cookies(&**backend, &target, args.since, &visited).await {
                        Ok(n) => out.cookies = Some(n),
                        Err(err) => fail(&mut out, "cookies", err),
                    }
                }
                Some(Err(err)) => fail(&mut out, "cookies", err.message.clone()),
                None => fail(&mut out, "cookies", "profiles are not available".into()),
            }
        }

        if args.cache || args.site_data {
            let mut origins = visited.clone();
            if args.since.is_none() {
                origins.extend(cookie_origins);
                if let Some(service) = &profiles {
                    origins.extend(indexeddb_origins(&service.profile_dir(&profile)));
                }
            }
            origins.sort();
            origins.dedup();
            match with_profile_tab(&*ctx.backend, &profile, &launch, |tab| {
                clear_in_tab(
                    &*ctx.backend,
                    tab,
                    args.cache,
                    args.site_data.then_some(&origins),
                )
            })
            .await
            {
                Ok((cache, site)) => {
                    if args.cache {
                        match cache {
                            Ok(()) => out.cache = true,
                            Err(err) => fail(&mut out, "cache", err),
                        }
                    }
                    if args.site_data {
                        match site {
                            Ok(n) => out.site_data_origins = Some(n),
                            Err(err) => fail(&mut out, "site_data", err),
                        }
                    }
                }
                Err(err) => {
                    for (on, part) in [(args.cache, "cache"), (args.site_data, "site_data")] {
                        if on {
                            fail(&mut out, part, err.to_string());
                        }
                    }
                }
            }
        }
        tracing::info!(?out, "browsing data cleared");
        Ok(out)
    }
}

/// Delete all cookies of the profile, or (with a time range) those of the visited sites.
async fn clear_cookies(
    backend: &dyn bmcp_profiles::ProfileBackend,
    target: &CookieTarget,
    since: Option<i64>,
    visited: &[String],
) -> Result<usize, String> {
    if since.is_none() {
        return backend
            .delete_cookies(target.clone(), None, None)
            .await
            .map_err(|e| e.to_string());
    }
    let mut hosts: Vec<String> = visited.iter().filter_map(|o| host_of_origin(o)).collect();
    hosts.sort();
    hosts.dedup();
    let mut deleted = 0;
    for host in hosts {
        deleted += backend
            .delete_cookies(target.clone(), Some(host), None)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(deleted)
}

/// Run `work` with a tab of `profile` (a tab without a profile name runs in `launch`, the
/// profile the browser started with): an open one, or a temporary off-screen tab.
async fn with_profile_tab<'a, T, F, Fut>(
    backend: &'a dyn BrowserBackend,
    profile: &str,
    launch: &str,
    work: F,
) -> BackendResult<T>
where
    F: FnOnce(TabId) -> Fut,
    Fut: std::future::Future<Output = T> + 'a,
{
    let existing = backend
        .list_tabs()
        .await?
        .into_iter()
        .find(|t| !t.incognito && t.profile.as_deref().unwrap_or(launch) == profile);
    if let Some(tab) = existing {
        return Ok(work(tab.id).await);
    }
    let options = OpenTabOptions {
        background: true,
        profile: Some(profile.to_string()),
        window: WindowTarget::Offscreen,
        ..Default::default()
    };
    let tab = backend.open_tab_with("about:blank".into(), options).await?;
    let result = work(tab).await;
    if let Err(err) = backend.close_tab(tab).await {
        tracing::warn!(%tab, %err, "temporary tab for clearing data not closed");
    }
    Ok(result)
}

/// The CDP part: cache and/or site data of `origins`. Returns (cache result, origins done).
async fn clear_in_tab(
    backend: &dyn BrowserBackend,
    tab: TabId,
    cache: bool,
    origins: Option<&Vec<String>>,
) -> (Result<(), String>, Result<usize, String>) {
    let cache_result = if cache {
        backend
            .cdp(
                tab,
                "Network.clearBrowserCache".into(),
                serde_json::json!({}),
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    } else {
        Ok(())
    };
    let site_result = match origins {
        None => Ok(0),
        Some(origins) => {
            let mut done = 0;
            let mut last_error = None;
            for origin in origins {
                let params = serde_json::json!({ "origin": origin, "storageTypes": STORAGE_TYPES });
                match backend
                    .cdp(tab, "Storage.clearDataForOrigin".into(), params)
                    .await
                {
                    Ok(_) => done += 1,
                    Err(err) => last_error = Some(format!("{origin}: {err}")),
                }
            }
            match last_error {
                Some(err) if done == 0 && !origins.is_empty() => Err(err),
                _ => Ok(done),
            }
        }
    };
    (cache_result, site_result)
}

/// `.example.com` / `example.com` → its https and http origins.
pub fn origins_of_domain(domain: &str) -> Vec<String> {
    let host = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Vec::new();
    }
    vec![format!("https://{host}"), format!("http://{host}")]
}

/// `https://a.test:8080` → `a.test`.
pub fn host_of_origin(origin: &str) -> Option<String> {
    bmcp_core::zoom::host_of(origin)
}

/// Origins with IndexedDB data in a profile folder (`IndexedDB/https_example.com_0.indexeddb.leveldb`,
/// port 0 = the default port). Newer Chromium keeps some of it in unnamed buckets; those are
/// reached through the visited sites instead.
pub fn indexeddb_origins(profile_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(profile_dir.join("IndexedDB")) else {
        return Vec::new();
    };
    let mut origins: Vec<String> = entries
        .flatten()
        .filter_map(|e| origin_from_indexeddb_name(&e.file_name().to_string_lossy()))
        .collect();
    origins.sort();
    origins.dedup();
    origins
}

/// `https_example.com_0.indexeddb.leveldb` → `https://example.com`; `http_a.test_8080…` →
/// `http://a.test:8080`.
pub fn origin_from_indexeddb_name(name: &str) -> Option<String> {
    let stem = name
        .strip_suffix(".indexeddb.leveldb")
        .or_else(|| name.strip_suffix(".indexeddb.blob"))?;
    let (scheme, rest) = stem.split_once('_')?;
    let (host, port) = rest.rsplit_once('_')?;
    if !matches!(scheme, "http" | "https") || host.is_empty() {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    Some(if port == 0 {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_from_cookie_domains_and_indexeddb_folders() {
        assert_eq!(
            origins_of_domain(".Example.com"),
            ["https://example.com", "http://example.com"]
        );
        assert!(origins_of_domain(" ").is_empty());
        assert_eq!(
            origin_from_indexeddb_name("https_mail.test_0.indexeddb.leveldb").as_deref(),
            Some("https://mail.test")
        );
        assert_eq!(
            origin_from_indexeddb_name("http_a.test_8080.indexeddb.blob").as_deref(),
            Some("http://a.test:8080")
        );
        assert_eq!(
            origin_from_indexeddb_name("chrome-extension_x_0.indexeddb.leveldb"),
            None
        );
        assert_eq!(origin_from_indexeddb_name("README"), None);
        assert_eq!(
            host_of_origin("https://a.test:8080").as_deref(),
            Some("a.test")
        );
    }
}
