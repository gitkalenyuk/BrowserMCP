//! `hints.show`, `hints.hide`, `hints.config`, `clipboard.write`.

use bmcp_core::{CmdError, Command, Ctx, Meta, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::{HintsConfig, Keys, Mode, normalize_site};
use crate::labels::check_alphabet;
use crate::service::{HintsService, ShowOutput};

// ------------------------------------------------------------------ hints.show

pub struct Show;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// What a picked hint does: click (default), newtab, focus or copy.
    pub mode: Option<Mode>,
}

impl Command for Show {
    const NAME: &'static str = "hints.show";
    type Args = ShowArgs;
    type Output = ShowOutput;

    fn meta() -> Meta {
        Meta::new(
            "Show Vim-style link hints: a short label (as, sd…) next to every interactive \
             element in view. Typing a label in the page clicks it (or opens it in a new tab, \
             focuses it, copies its link — see mode); Esc cancels. Returns the labels with \
             the elements they point at.",
            Toolset::Ui,
        )
        .mutates_tab()
        .act()
        .palette("Show link hints")
    }

    fn target_tab(args: &ShowArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: ShowArgs) -> Result<ShowOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let service = HintsService::from_ctx(&ctx);
        service.show(&ctx, tab, args.mode.unwrap_or_default()).await
    }
}

// ------------------------------------------------------------------ hints.hide

pub struct Hide;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HideArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HideOutput {
    pub tab: TabId,
    /// True when hints were on screen.
    pub hidden: bool,
}

impl Command for Hide {
    const NAME: &'static str = "hints.hide";
    type Args = HideArgs;
    type Output = HideOutput;

    fn meta() -> Meta {
        Meta::new("Take the link hints off the page.", Toolset::Ui).mutates_tab()
    }

    fn target_tab(args: &HideArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(ctx: Ctx, args: HideArgs) -> Result<HideOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let hidden = HintsService::from_ctx(&ctx).hide(&ctx, tab).await;
        Ok(HideOutput { tab, hidden })
    }
}

// ------------------------------------------------------------------ hints.config

pub struct Config;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigArgs {
    /// Turn the trigger keys in pages on or off (hints.show always works).
    pub enabled: Option<bool>,
    /// Trigger keys, e.g. {"click": "f", "newtab": "F", "copy": "y"}. Modes left out get
    /// their default (click f, newtab F, none for focus/copy); null turns a key off.
    pub keys: Option<Keys>,
    /// Replace the list of sites where the trigger keys are off.
    pub disabled_sites: Option<Vec<String>>,
    /// Turn the trigger keys off on one site (host name or URL; subdomains included).
    pub disable_site: Option<String>,
    /// Turn them back on for one site.
    pub enable_site: Option<String>,
    /// Letters for labels (2+ different lower-case letters/digits). Default "asdfghjkl".
    pub alphabet: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigOutput {
    /// The settings now in effect (saved to data/config/hints.json).
    pub config: HintsConfig,
}

impl Command for Config {
    const NAME: &'static str = "hints.config";
    type Args = ConfigArgs;
    type Output = ConfigOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read or change link-hint settings: enabled, trigger keys (default f = click, \
             F = new tab), sites where the keys are off, label alphabet. No arguments = just \
             read them.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ConfigArgs) -> Result<ConfigOutput, CmdError> {
        let service = HintsService::from_ctx(&ctx);
        let mut config = service.config();
        let before = config.clone();
        apply(&mut config, args)?;
        if config != before {
            let open: Vec<TabId> = ctx
                .backend
                .list_tabs()
                .await?
                .iter()
                .map(|t| t.id)
                .collect();
            service.set_config(&ctx, config.clone(), &open).await?;
        }
        Ok(ConfigOutput { config })
    }
}

/// Apply the changes in `args` to `config`, validating them.
fn apply(config: &mut HintsConfig, args: ConfigArgs) -> Result<(), CmdError> {
    if let Some(enabled) = args.enabled {
        config.enabled = enabled;
    }
    if let Some(keys) = args.keys {
        keys.check().map_err(CmdError::invalid_params)?;
        config.keys = keys;
    }
    if let Some(sites) = args.disabled_sites {
        config.disabled_sites = sites.iter().map(|s| normalize_site(s)).collect();
    }
    if let Some(site) = args.disable_site {
        let site = normalize_site(&site);
        if !site.is_empty() && !config.disabled_sites.contains(&site) {
            config.disabled_sites.push(site);
        }
    }
    if let Some(site) = args.enable_site {
        let site = normalize_site(&site);
        config.disabled_sites.retain(|s| *s != site);
    }
    config.disabled_sites.retain(|s| !s.is_empty());
    if let Some(alphabet) = args.alphabet {
        check_alphabet(&alphabet).map_err(CmdError::invalid_params)?;
        config.alphabet = alphabet;
    }
    Ok(())
}

// ------------------------------------------------------------------ clipboard.write

pub struct ClipboardWrite;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClipboardWriteArgs {
    /// Tab whose page does the copy (the clipboard needs a focused page). Default: the most
    /// recently opened tab.
    pub tab: Option<TabId>,
    /// Text to put on the clipboard.
    pub text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ClipboardWriteOutput {
    pub tab: TabId,
    /// How it was written: clipboard_api or exec_command.
    pub method: String,
}

impl Command for ClipboardWrite {
    const NAME: &'static str = "clipboard.write";
    type Args = ClipboardWriteArgs;
    type Output = ClipboardWriteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Put text on the system clipboard (done from inside a page with a user gesture; \
             the browser window should be focused).",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ClipboardWriteArgs) -> Result<ClipboardWriteOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let hints = HintsService::from_ctx(&ctx).ensure(&ctx, tab).await?;
        let method = hints.copy(&ctx, &args.text).await?;
        Ok(ClipboardWriteOutput { tab, method })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_changes_are_validated() {
        let mut config = HintsConfig::default();
        apply(
            &mut config,
            ConfigArgs {
                enabled: Some(false),
                disable_site: Some("https://www.GitHub.com/notifications".into()),
                ..ConfigArgs::default()
            },
        )
        .unwrap();
        assert!(!config.enabled);
        assert_eq!(config.disabled_sites, ["github.com"]);
        apply(
            &mut config,
            ConfigArgs {
                enable_site: Some("github.com".into()),
                ..ConfigArgs::default()
            },
        )
        .unwrap();
        assert!(config.disabled_sites.is_empty());

        let bad_alphabet = ConfigArgs {
            alphabet: Some("a".into()),
            ..ConfigArgs::default()
        };
        assert!(apply(&mut config, bad_alphabet).is_err());
        let bad_keys = ConfigArgs {
            keys: Some(Keys {
                click: Some("g".into()),
                newtab: Some("g".into()),
                ..Keys::default()
            }),
            ..ConfigArgs::default()
        };
        assert!(apply(&mut config, bad_keys).is_err());
    }
}
