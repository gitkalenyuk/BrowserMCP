//! Startup & tabs (`data/config/startup.json`, `startup.config`).
//!
//! * what opens when the browser starts: the last session (`data/runtime/session.json`, the
//!   default), the new tab page, or a list of pages — [`plan`] turns that plus the command
//!   line into what the CEF layer opens;
//! * the home page and whether the toolbar shows a Home button;
//! * the page a new tab shows (the built-in new tab page unless a URL is set);
//! * where new tabs go in the tab strip, and whether closing the last tab closes the window.
//!
//! The command line still wins: `--url` opens that page, `--no-restore` / `--headless` never
//! restore a session.

use bmcp_core::{CmdError, Command, Ctx, Meta, NEW_TAB_URL, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{BrowserSettings, SETTINGS_CHANGED, urls::normalize_page_url};

/// Most pages "open specific pages" may list.
pub const MAX_PAGES: usize = 20;

/// What the browser opens when it starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnStartup {
    /// Reopen the tabs of the last session (each window, active tab, split view).
    #[default]
    Restore,
    /// One window with the new tab page.
    NewTab,
    /// The pages in `pages`, each in its own tab.
    Pages,
}

/// Where a new tab (Ctrl+T, the + button, `tabs.open`) is put in the tab strip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NewTabPosition {
    /// After the last tab (like Chrome's Ctrl+T).
    #[default]
    End,
    /// Right after the tab shown in the window.
    AfterActive,
}

/// Contents of `startup.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StartupConfig {
    #[serde(default)]
    pub on_startup: OnStartup,
    /// The pages for `on_startup: "pages"`.
    #[serde(default)]
    pub pages: Vec<String>,
    /// Page of the Home button; `null` = the new tab page.
    #[serde(default)]
    pub home_page: Option<String>,
    /// Show the Home button in the toolbar.
    #[serde(default)]
    pub show_home_button: bool,
    /// What a new tab shows; `null` = the built-in new tab page (`bmcp://ui/newtab.html`).
    #[serde(default)]
    pub new_tab_url: Option<String>,
    #[serde(default)]
    pub new_tab_position: NewTabPosition,
    /// Closing the last tab of a window keeps the window with a new tab page (default: the
    /// window closes, like Chrome).
    #[serde(default)]
    pub keep_window_on_last_tab: bool,
}

impl StartupConfig {
    /// The URL a new tab opens.
    pub fn new_tab_page(&self) -> &str {
        self.new_tab_url.as_deref().unwrap_or(NEW_TAB_URL)
    }
}

/// What the CEF layer opens at startup ([`plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartPlan {
    /// Reopen the saved session (when there is one; otherwise `first_url` opens).
    pub restore: bool,
    /// The first window's first tab.
    pub first_url: String,
    /// More tabs to open once the browser is up (in this order).
    pub more_urls: Vec<String>,
}

/// Combine the startup settings with the command line (pure).
///
/// * `cli_url`: `--url` when it was given (it always opens, the session is restored around
///   it as before);
/// * `may_restore`: false for `--no-restore`, `--headless` and `--dump-ax`.
pub fn plan(config: &StartupConfig, cli_url: Option<&str>, may_restore: bool) -> StartPlan {
    let restore = may_restore && config.on_startup == OnStartup::Restore;
    if let Some(url) = cli_url {
        return StartPlan {
            restore,
            first_url: url.to_string(),
            more_urls: Vec::new(),
        };
    }
    let pages: Vec<String> = match config.on_startup {
        OnStartup::Pages => config.pages.clone(),
        _ => Vec::new(),
    };
    match pages.split_first() {
        Some((first, more)) => StartPlan {
            restore,
            first_url: first.clone(),
            more_urls: more.to_vec(),
        },
        // Restore with nothing saved yet, the new tab page, or "pages" with an empty list.
        None => StartPlan {
            restore,
            first_url: config.new_tab_page().to_string(),
            more_urls: Vec::new(),
        },
    }
}

// --------------------------------------------------------------------------- startup.config

pub struct Config;

/// Every field is optional; only the given ones change.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigArgs {
    /// restore (last session) | new_tab | pages.
    pub on_startup: Option<OnStartup>,
    /// Pages for on_startup = "pages" (replaces the list; "example.com" becomes https://…).
    pub pages: Option<Vec<String>>,
    /// Home button page; "" = the new tab page.
    pub home_page: Option<String>,
    pub show_home_button: Option<bool>,
    /// What a new tab shows; "" = the built-in new tab page.
    pub new_tab_url: Option<String>,
    /// end | after_active.
    pub new_tab_position: Option<NewTabPosition>,
    /// true: closing the last tab keeps the window (with a new tab page).
    pub keep_window_on_last_tab: Option<bool>,
    /// Back to the defaults first.
    pub reset: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigOutput {
    /// The settings in force (saved in data/config/startup.json).
    pub config: StartupConfig,
    /// The URL a new tab opens (new_tab_url or the built-in page).
    pub new_tab_page: String,
}

impl Command for Config {
    const NAME: &'static str = "startup.config";
    type Args = ConfigArgs;
    type Output = ConfigOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read or change the startup & tab settings (data/config/startup.json): on_startup \
             (restore | new_tab | pages) with pages, home_page and show_home_button, \
             new_tab_url, new_tab_position (end | after_active), keep_window_on_last_tab; reset \
             goes back to the defaults. No arguments = read. Startup choices apply on the next \
             start; the rest at once.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ConfigArgs) -> Result<ConfigOutput, CmdError> {
        let settings = BrowserSettings::from_ctx(&ctx)?;
        let changes = args.reset.is_some()
            || args.on_startup.is_some()
            || args.pages.is_some()
            || args.home_page.is_some()
            || args.show_home_button.is_some()
            || args.new_tab_url.is_some()
            || args.new_tab_position.is_some()
            || args.keep_window_on_last_tab.is_some();
        let config = if changes {
            let (config, ()) = settings.files().startup.update(|c| apply(c, args))?;
            ctx.events.emit(
                SETTINGS_CHANGED,
                serde_json::json!({ "area": "startup", "config": config }),
            );
            config
        } else {
            settings.startup()
        };
        Ok(ConfigOutput {
            new_tab_page: config.new_tab_page().to_string(),
            config,
        })
    }
}

/// Apply `args` to `config`, validating every URL (pure).
pub fn apply(config: &mut StartupConfig, args: ConfigArgs) -> Result<(), CmdError> {
    if args.reset == Some(true) {
        *config = StartupConfig::default();
    }
    if let Some(on) = args.on_startup {
        config.on_startup = on;
    }
    if let Some(pages) = args.pages {
        let pages: Vec<String> = pages
            .iter()
            .filter(|p| !p.trim().is_empty())
            .map(|p| normalize_page_url(p))
            .collect::<Result<_, _>>()?;
        if pages.len() > MAX_PAGES {
            return Err(CmdError::invalid_params(format!(
                "at most {MAX_PAGES} startup pages, got {}",
                pages.len()
            )));
        }
        config.pages = pages;
    }
    if let Some(home) = args.home_page {
        config.home_page = optional_url(&home)?;
    }
    if let Some(show) = args.show_home_button {
        config.show_home_button = show;
    }
    if let Some(url) = args.new_tab_url {
        config.new_tab_url = optional_url(&url)?.filter(|u| u != NEW_TAB_URL);
    }
    if let Some(position) = args.new_tab_position {
        config.new_tab_position = position;
    }
    if let Some(keep) = args.keep_window_on_last_tab {
        config.keep_window_on_last_tab = keep;
    }
    if config.on_startup == OnStartup::Pages && config.pages.is_empty() {
        return Err(CmdError::invalid_params(
            "on_startup \"pages\" needs at least one page in `pages`",
        ));
    }
    Ok(())
}

/// "" → `None` (the default), anything else a checked URL.
fn optional_url(text: &str) -> Result<Option<String>, CmdError> {
    if text.trim().is_empty() {
        Ok(None)
    } else {
        normalize_page_url(text).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(on: OnStartup, pages: &[&str]) -> StartupConfig {
        StartupConfig {
            on_startup: on,
            pages: pages.iter().map(|p| p.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn plan_restore_falls_back_to_the_new_tab_page() {
        let p = plan(&StartupConfig::default(), None, true);
        assert!(p.restore);
        assert_eq!(p.first_url, NEW_TAB_URL);
        // --no-restore / headless.
        assert!(!plan(&StartupConfig::default(), None, false).restore);
    }

    #[test]
    fn plan_pages_opens_them_all_and_never_restores() {
        let c = config(
            OnStartup::Pages,
            &["https://a.test/", "https://b.test/", "https://c.test/"],
        );
        let p = plan(&c, None, true);
        assert!(!p.restore);
        assert_eq!(p.first_url, "https://a.test/");
        assert_eq!(p.more_urls, vec!["https://b.test/", "https://c.test/"]);
    }

    #[test]
    fn plan_new_tab_uses_the_custom_new_tab_page() {
        let mut c = config(OnStartup::NewTab, &[]);
        c.new_tab_url = Some("https://start.test/".into());
        let p = plan(&c, None, true);
        assert!(!p.restore);
        assert_eq!(p.first_url, "https://start.test/");
    }

    #[test]
    fn plan_cli_url_wins_but_restore_still_follows_the_setting() {
        let c = config(OnStartup::Pages, &["https://a.test/"]);
        let p = plan(&c, Some("https://cli.test/"), true);
        assert_eq!(p.first_url, "https://cli.test/");
        assert!(p.more_urls.is_empty());
        assert!(!p.restore);
        assert!(plan(&StartupConfig::default(), Some("https://cli.test/"), true).restore);
    }

    #[test]
    fn apply_checks_urls_and_needs_pages_for_pages() {
        let mut c = StartupConfig::default();
        apply(
            &mut c,
            ConfigArgs {
                on_startup: Some(OnStartup::Pages),
                pages: Some(vec!["example.com".into(), " ".into()]),
                home_page: Some("https://home.test".into()),
                new_tab_url: Some(NEW_TAB_URL.into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.pages, vec!["https://example.com/"]);
        assert_eq!(c.home_page.as_deref(), Some("https://home.test/"));
        // The built-in page is stored as "no custom page".
        assert_eq!(c.new_tab_url, None);

        let err = apply(
            &mut c.clone(),
            ConfigArgs {
                pages: Some(vec![]),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.message.contains("at least one page"), "{err:?}");

        let err = apply(
            &mut c.clone(),
            ConfigArgs {
                home_page: Some("javascript:alert(1)".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.message.contains("javascript"), "{err:?}");

        apply(
            &mut c,
            ConfigArgs {
                home_page: Some(String::new()),
                reset: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c.home_page, None);
        apply(
            &mut c,
            ConfigArgs {
                reset: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(c, StartupConfig::default());
    }
}
