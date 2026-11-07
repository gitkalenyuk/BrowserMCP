//! `browser.languages_get` and `browser.languages_set`.

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::{LanguageSettings, normalize_list, normalize_tag};
use crate::service::{LanguagesService, ProfileLanguage};

/// The browser's languages, as `browser.languages_get` and `browser.languages_set` return them.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LanguagesInfo {
    /// Website languages in effect, most preferred first (sent as `Accept-Language` and
    /// `navigator.languages` by every profile without a language of its own).
    pub web: Vec<String>,
    /// True when `web` is the automatic list (the interface language, then English) because
    /// no list was set.
    pub web_automatic: bool,
    /// Chromium's own locale as saved; `null` = the Windows display language.
    pub browser_locale: Option<String>,
    /// The locale this run of the browser started with (`null` = the Windows display
    /// language). Differs from `browser_locale` until the browser is restarted.
    pub browser_locale_active: Option<String>,
    /// True when `browser_locale` changes after a restart.
    pub restart_required: bool,
    /// Locales that can be chosen: the `locales/*.pak` files next to the executable (the
    /// portable package ships `en-US` and `uk` by default).
    pub browser_locales: Vec<String>,
    /// Spellcheck dictionaries in use (empty = spellcheck off): the saved list, else what
    /// Chromium uses by default.
    pub spellcheck: Vec<String>,
    /// True when no list was saved (`spellcheck` shows Chromium's own choice).
    pub spellcheck_default: bool,
    /// Can this Chromium build use them? `null` = unknown (no browser attached).
    pub spellcheck_supported: Option<bool>,
    /// The interface language the UI reported (`en`, `uk`); `null` = never reported.
    pub ui_language: Option<String>,
    /// Profiles that keep their own language (their stealth identity, fitted to the proxy's
    /// country, or `profile.create {accept_language}`): the global list does not apply to them.
    pub own_language: Vec<ProfileLanguage>,
}

async fn info(service: &LanguagesService) -> Result<LanguagesInfo, CmdError> {
    let report = match service.backend() {
        Some(backend) => backend.languages().await?,
        None => Default::default(),
    };
    Ok(describe(service, report))
}

fn describe(service: &LanguagesService, report: crate::BackendLanguages) -> LanguagesInfo {
    let settings = service.settings();
    let active = service.started_locale().map(str::to_string);
    LanguagesInfo {
        web: service.effective_web(),
        web_automatic: settings.web.is_empty(),
        restart_required: !same_locale(settings.browser_locale.as_deref(), active.as_deref()),
        browser_locale: settings.browser_locale,
        browser_locale_active: active,
        browser_locales: service.available_locales(),
        spellcheck_default: settings.spellcheck.is_none(),
        spellcheck: settings
            .spellcheck
            .or(report.spellcheck_current)
            .unwrap_or_default(),
        spellcheck_supported: report.spellcheck_supported,
        ui_language: settings.ui_language,
        own_language: report.own_language,
    }
}

fn same_locale(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        (None, None) => true,
        _ => false,
    }
}

// ------------------------------------------------------------------ browser.languages_get

pub struct LanguagesGet;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguagesGetArgs {}

impl Command for LanguagesGet {
    const NAME: &'static str = "browser.languages_get";
    type Args = LanguagesGetArgs;
    type Output = LanguagesInfo;

    fn meta() -> Meta {
        Meta::new(
            "Read the browser's languages: website languages (Accept-Language and \
             navigator.languages, most preferred first), Chromium's own locale (menus, \
             dialogs, error pages; changes after a restart) with the locales available, \
             spellcheck dictionaries, and the profiles that keep their own language \
             (stealth identities).",
            Toolset::Profile,
        )
    }

    async fn run(ctx: Ctx, _args: LanguagesGetArgs) -> Result<LanguagesInfo, CmdError> {
        info(&*LanguagesService::from_ctx(&ctx)?).await
    }
}

// ------------------------------------------------------------------ browser.languages_set

pub struct LanguagesSet;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguagesSetArgs {
    /// Website languages, most preferred first, e.g. ["uk", "en"]. Applied at once to every
    /// profile without a language of its own (profiles with a stealth identity keep theirs).
    /// [] = automatic (the interface language, then English).
    pub web: Option<Vec<String>>,
    /// Chromium's own locale, e.g. "uk" or "en-US" (one of browser_locales from
    /// browser.languages_get); "system" = the Windows display language. Takes effect after
    /// the browser restarts.
    pub browser_locale: Option<String>,
    /// Spellcheck dictionaries, e.g. ["uk", "en-US"]; [] = spellcheck off.
    pub spellcheck: Option<Vec<String>>,
    /// The browser UI's interface language ("en" or "uk"), set by the UI: the first language
    /// of the automatic website list.
    pub ui_language: Option<String>,
}

impl Command for LanguagesSet {
    const NAME: &'static str = "browser.languages_set";
    type Args = LanguagesSetArgs;
    type Output = LanguagesInfo;

    fn meta() -> Meta {
        Meta::new(
            "Change the browser's languages (fields left out stay): web = the languages \
             websites see (Accept-Language, navigator.languages), applied at once to every \
             profile without its own language — stealth identities keep theirs; \
             browser_locale = Chromium's own locale, after a restart; spellcheck = \
             dictionaries. Saved in data/config/languages.json. Returns the same as \
             browser.languages_get.",
            Toolset::Profile,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: LanguagesSetArgs) -> Result<LanguagesInfo, CmdError> {
        let service = LanguagesService::from_ctx(&ctx)?;
        let before = service.settings();
        let mut next = before.clone();
        apply(&mut next, args, &service.available_locales())?;
        if next == before {
            return info(&service).await;
        }
        let web_before = service.effective_web();
        service.store(next.clone())?;
        let web = service.effective_web();
        let report = match service.backend() {
            Some(backend) if web != web_before || next.spellcheck != before.spellcheck => {
                backend
                    .apply_languages(web, next.spellcheck.clone())
                    .await?
            }
            Some(backend) => backend.languages().await?,
            None => Default::default(),
        };
        Ok(describe(&service, report))
    }
}

/// Validate `args` and write them into `settings`. `locales` = the Chromium locales present.
pub fn apply(
    settings: &mut LanguageSettings,
    args: LanguagesSetArgs,
    locales: &[String],
) -> Result<(), CmdError> {
    if let Some(web) = args.web {
        settings.web = normalize_list(&web).map_err(CmdError::invalid_params)?;
    }
    if let Some(spellcheck) = args.spellcheck {
        settings.spellcheck = Some(normalize_list(&spellcheck).map_err(CmdError::invalid_params)?);
    }
    if let Some(ui) = args.ui_language {
        settings.ui_language = Some(normalize_tag(&ui).ok_or_else(|| {
            CmdError::invalid_params(format!("ui_language {ui:?} is not a language tag"))
        })?);
    }
    if let Some(locale) = args.browser_locale {
        settings.browser_locale = browser_locale(&locale, locales)?;
    }
    Ok(())
}

/// `system` → `None`; otherwise one of `locales` in its own spelling (`UK` → `uk`).
fn browser_locale(locale: &str, locales: &[String]) -> Result<Option<String>, CmdError> {
    let locale = locale.trim();
    if locale.is_empty() || locale.eq_ignore_ascii_case("system") {
        return Ok(None);
    }
    let wanted = locale.replace('_', "-");
    match locales.iter().find(|l| l.eq_ignore_ascii_case(&wanted)) {
        Some(found) => Ok(Some(found.clone())),
        None if locales.is_empty() => Err(CmdError::invalid_params(
            "no Chromium locales were found next to the browser (locales/*.pak)",
        )),
        None => Err(CmdError::invalid_params(format!(
            "locale {locale:?} is not shipped with this browser; available: {}",
            locales.join(", ")
        ))),
    }
}
