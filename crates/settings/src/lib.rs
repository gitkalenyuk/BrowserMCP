//! BrowserMCP browser settings (Settings → Startup & tabs, Search, Downloads, Site settings,
//! Data & storage, About). No CEF dependency.
//!
//! | Module | Commands | File |
//! |---|---|---|
//! | [`startup`] | `startup.config` | `data/config/startup.json` |
//! | [`search`] | `search.config`, `search.url` | `data/config/search.json` |
//! | [`downloads`] | `downloads.config` | `data/config/downloads.json` |
//! | [`zoom`] | `zoom.list`, `zoom.reset` | `data/config/zoom.json` (kept by the CEF layer) |
//! | [`clear`] | `browsing_data.clear` | — |
//! | [`usage`] | `browsing_data.usage` | — |
//! | [`about`] | `system.about` | — |
//!
//! Wiring (the app): [`BrowserSettings::load`] once before the browser starts, hand it to the
//! CEF layer (downloads folder, new tab position, last-tab behaviour, the startup plan) and put
//! the same instance into the registry extensions; also an [`about::AppInfo`] once the API
//! listens. Changes are announced on the bus as [`SETTINGS_CHANGED`] `{area, …}` so the
//! browser UI (address bar engine, new tab page, Home button) follows at once.

pub mod about;
pub mod clear;
pub mod downloads;
mod file;
pub mod search;
pub mod startup;
pub mod urls;
pub mod usage;
pub mod zoom;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bmcp_core::{CmdError, Ctx, DataDir, ErrorCode, Registry};

pub use about::AppInfo;
pub use downloads::DownloadsConfig;
use file::ConfigFile;
pub use search::{SearchConfig, SearchEngine};
pub use startup::{NewTabPosition, OnStartup, StartPlan, StartupConfig};

/// Bus topic after a settings change: `{area: "startup" | "search" | "downloads", …}`.
pub const SETTINGS_CHANGED: &str = "settings.changed";

/// Register the commands (their state comes from [`BrowserSettings`] in the extensions).
pub fn register(registry: &mut Registry) {
    registry
        .add::<startup::Config>()
        .add::<search::Config>()
        .add::<search::Url>()
        .add::<downloads::Config>()
        .add::<zoom::List>()
        .add::<zoom::Reset>()
        .add::<clear::Clear>()
        .add::<usage::Usage>()
        .add::<about::About>();
}

/// The settings files, loaded once and shared by the browser and the commands (cheap to clone).
#[derive(Clone)]
pub struct BrowserSettings {
    inner: Arc<Files>,
}

pub(crate) struct Files {
    data_root: PathBuf,
    pub(crate) startup: ConfigFile<StartupConfig>,
    pub(crate) search: ConfigFile<SearchConfig>,
    pub(crate) downloads: ConfigFile<DownloadsConfig>,
}

impl BrowserSettings {
    /// Read `startup.json`, `search.json` and `downloads.json` from `data/config`.
    pub fn load(data: &DataDir) -> Self {
        let config = data.config();
        BrowserSettings {
            inner: Arc::new(Files {
                data_root: data.root().to_path_buf(),
                startup: ConfigFile::load(config.join("startup.json")),
                search: ConfigFile::load(config.join("search.json")),
                downloads: ConfigFile::load(config.join("downloads.json")),
            }),
        }
    }

    /// The instance the app put into the registry extensions.
    pub fn from_ctx(ctx: &Ctx) -> Result<BrowserSettings, CmdError> {
        ctx.extensions
            .get::<BrowserSettings>()
            .map(|s| (*s).clone())
            .ok_or_else(|| {
                CmdError::new(ErrorCode::NotImplemented, "browser settings are not loaded")
            })
    }

    pub fn data_root(&self) -> &Path {
        &self.inner.data_root
    }

    pub fn startup(&self) -> StartupConfig {
        self.inner.startup.get()
    }

    pub fn search(&self) -> SearchConfig {
        self.inner.search.get()
    }

    pub fn downloads(&self) -> DownloadsConfig {
        self.inner.downloads.get()
    }

    /// Where a download of a tab (of mass run `run`, if any) goes.
    pub fn download_dir(&self, run: Option<&str>) -> PathBuf {
        downloads::download_dir(&self.downloads(), self.data_root(), run)
    }

    /// The URL a new tab opens.
    pub fn new_tab_page(&self) -> String {
        self.startup().new_tab_page().to_string()
    }

    /// The files, for the commands of this crate.
    pub(crate) fn files(&self) -> &Files {
        &self.inner
    }
}

#[cfg(test)]
mod tests;
