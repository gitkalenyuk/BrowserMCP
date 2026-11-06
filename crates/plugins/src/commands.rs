//! The registry commands of this crate; thin wrappers over [`PluginsService`].
//!
//! | Command | What |
//! |---|---|
//! | `plugins.list/install/enable/disable/remove/reload/logs` | `data/plugins/<name>` |
//! | `plugins.palette` / `plugins.palette_run` | Ctrl+K entries contributed by plugins |
//! | `userscripts.list/get/save/delete/enable` | `data/userscripts/*.ts` |
//! | `userscripts.inject` | run JS in an isolated world of a tab (what userscripts use) |
//! | `trigger.webhook_set/list/remove` | webhook triggers (`POST /v1/hooks/{id}`) |
//! | `trigger.webhook_fire` | hidden: what the webhook route calls |

use std::path::PathBuf;
use std::sync::Arc;

use bmcp_core::{CmdError, Command, Ctx, Meta, TabId, Toolset, command::Empty};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::plugins::{LogLine, PluginInfo};
use crate::userscripts::UserscriptInfo;
use crate::webhooks::{HookCreated, HookInfo};
use crate::{Inner, PluginsService};

fn service(ctx: &Ctx) -> Result<Arc<Inner>, CmdError> {
    ctx.extensions
        .get::<PluginsService>()
        .map(|s| s.inner.clone())
        .ok_or_else(|| CmdError::internal("the plugin service is not running"))
}

// ------------------------------------------------------------------ plugins.*

pub struct PluginsList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct PluginsListOutput {
    pub plugins: Vec<PluginInfo>,
}

impl Command for PluginsList {
    const NAME: &'static str = "plugins.list";
    type Args = Empty;
    type Output = PluginsListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List installed plugins (data/plugins/<name>) with manifest, permissions, status \
             (running, exited, crashed, disabled, error), pid and restarts.",
            Toolset::Script,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<PluginsListOutput, CmdError> {
        let inner = service(&ctx)?;
        // Folders copied in by hand show up without a reload.
        inner.rescan_plugins();
        Ok(PluginsListOutput {
            plugins: inner.list_plugins(),
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginNameArgs {
    /// Plugin name (its folder in data/plugins).
    pub name: String,
}

pub struct PluginsInstall;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginsInstallArgs {
    /// Folder with plugin.json and the main script (absolute, or relative to the browser's
    /// working directory). It is copied to data/plugins/<name>.
    pub path: String,
    /// Start it right away (default true).
    pub enable: Option<bool>,
    /// Overwrite an installed plugin of the same name (default false).
    pub replace: Option<bool>,
}

impl Command for PluginsInstall {
    const NAME: &'static str = "plugins.install";
    type Args = PluginsInstallArgs;
    type Output = PluginInfo;

    fn meta() -> Meta {
        Meta::new(
            "Install a plugin from a folder (plugin.json + main.ts) into data/plugins and start \
             it. The plugin may only call the commands in its manifest's permissions; check \
             `warnings` for permissions that amount to full access.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PluginsInstallArgs) -> Result<PluginInfo, CmdError> {
        service(&ctx)?.install(
            &PathBuf::from(&args.path),
            args.enable.unwrap_or(true),
            args.replace.unwrap_or(false),
        )
    }
}

pub struct PluginsEnable;

impl Command for PluginsEnable {
    const NAME: &'static str = "plugins.enable";
    type Args = PluginNameArgs;
    type Output = PluginInfo;

    fn meta() -> Meta {
        Meta::new(
            "Enable a plugin and start its script (remembered across restarts).",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PluginNameArgs) -> Result<PluginInfo, CmdError> {
        service(&ctx)?.set_enabled(&args.name, true)
    }
}

pub struct PluginsDisable;

impl Command for PluginsDisable {
    const NAME: &'static str = "plugins.disable";
    type Args = PluginNameArgs;
    type Output = PluginInfo;

    fn meta() -> Meta {
        Meta::new(
            "Disable a plugin: stop its process and revoke its connection token.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PluginNameArgs) -> Result<PluginInfo, CmdError> {
        service(&ctx)?.set_enabled(&args.name, false)
    }
}

pub struct PluginsRemove;

#[derive(Debug, Serialize, JsonSchema)]
pub struct RemovedOutput {
    /// False when there was nothing to remove.
    pub removed: bool,
}

impl Command for PluginsRemove {
    const NAME: &'static str = "plugins.remove";
    type Args = PluginNameArgs;
    type Output = RemovedOutput;

    fn meta() -> Meta {
        Meta::new(
            "Stop a plugin and delete its folder from data/plugins.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PluginNameArgs) -> Result<RemovedOutput, CmdError> {
        Ok(RemovedOutput {
            removed: service(&ctx)?.remove(&args.name)?,
        })
    }
}

pub struct PluginsReload;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginsReloadArgs {
    /// Plugin to reload; omit to rescan data/plugins and restart every plugin.
    pub name: Option<String>,
}

impl Command for PluginsReload {
    const NAME: &'static str = "plugins.reload";
    type Args = PluginsReloadArgs;
    type Output = PluginsListOutput;

    fn meta() -> Meta {
        Meta::new(
            "Re-read plugin.json from disk and restart the plugin (after editing its files); \
             without a name, rescan data/plugins and restart all.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PluginsReloadArgs) -> Result<PluginsListOutput, CmdError> {
        Ok(PluginsListOutput {
            plugins: service(&ctx)?.reload(args.name.as_deref())?,
        })
    }
}

pub struct PluginsLogs;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginsLogsArgs {
    pub name: String,
    /// Newest lines to return (default 100, at most 500 are kept).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PluginsLogsOutput {
    pub name: String,
    /// Oldest first. `stream` is stdout (console.log), stderr (console.error, uncaught errors)
    /// or system (started, exited, restarting…).
    pub lines: Vec<LogLine>,
}

impl Command for PluginsLogs {
    const NAME: &'static str = "plugins.logs";
    type Args = PluginsLogsArgs;
    type Output = PluginsLogsOutput;

    fn meta() -> Meta {
        Meta::new(
            "The last output lines of a plugin's script (console.log/error, crashes, restarts).",
            Toolset::Script,
        )
    }

    async fn run(ctx: Ctx, args: PluginsLogsArgs) -> Result<PluginsLogsOutput, CmdError> {
        let lines = service(&ctx)?.logs(&args.name, args.limit.unwrap_or(100))?;
        Ok(PluginsLogsOutput {
            name: args.name,
            lines,
        })
    }
}

pub struct PluginsPalette;

#[derive(Debug, Serialize, JsonSchema)]
pub struct PaletteCommand {
    pub plugin: String,
    pub id: String,
    pub title: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PluginsPaletteOutput {
    pub commands: Vec<PaletteCommand>,
}

impl Command for PluginsPalette {
    const NAME: &'static str = "plugins.palette";
    type Args = Empty;
    type Output = PluginsPaletteOutput;

    fn meta() -> Meta {
        Meta::new(
            "Ctrl+K palette entries contributed by enabled plugins (ui.palette in plugin.json). \
             Run one with plugins.palette_run.",
            Toolset::Ui,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<PluginsPaletteOutput, CmdError> {
        let commands = service(&ctx)?
            .palette()
            .into_iter()
            .map(|(plugin, entry)| PaletteCommand {
                plugin,
                id: entry.id,
                title: entry.title,
            })
            .collect();
        Ok(PluginsPaletteOutput { commands })
    }
}

pub struct PluginsPaletteRun;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaletteRunArgs {
    pub plugin: String,
    /// Entry id from plugins.palette.
    pub id: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PaletteRunOutput {
    /// False when the plugin is not running (the event was sent anyway).
    pub delivered: bool,
}

impl Command for PluginsPaletteRun {
    const NAME: &'static str = "plugins.palette_run";
    type Args = PaletteRunArgs;
    type Output = PaletteRunOutput;

    fn meta() -> Meta {
        Meta::new(
            "Run a plugin's palette entry: sends plugin.command {plugin, id} to that plugin.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: PaletteRunArgs) -> Result<PaletteRunOutput, CmdError> {
        Ok(PaletteRunOutput {
            delivered: service(&ctx)?.palette_run(&args.plugin, &args.id)?,
        })
    }
}

// ------------------------------------------------------------------ userscripts.*

pub struct UserscriptsList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct UserscriptsListOutput {
    pub userscripts: Vec<UserscriptInfo>,
}

impl Command for UserscriptsList {
    const NAME: &'static str = "userscripts.list";
    type Args = Empty;
    type Output = UserscriptsListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List userscripts (data/userscripts/*.ts and those of running plugins) with their \
             parsed @match/@include/@exclude/@run-at header and last run.",
            Toolset::Script,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<UserscriptsListOutput, CmdError> {
        Ok(UserscriptsListOutput {
            userscripts: service(&ctx)?.list_userscripts(),
        })
    }
}

pub struct UserscriptsSave;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserscriptsSaveArgs {
    /// 1-64 chars of A-Z a-z 0-9 _ - (no .ts); overwrites.
    pub name: String,
    /// TypeScript starting with a header, e.g. `// @match https://*.example.com/*` and
    /// `// @run-at document-end`. It runs once per matching page load with inputs.tab (tab id)
    /// and inputs.url; change the page with browser.userscripts.inject({tab, js}).
    pub code: String,
}

impl Command for UserscriptsSave {
    const NAME: &'static str = "userscripts.save";
    type Args = UserscriptsSaveArgs;
    type Output = UserscriptInfo;

    fn meta() -> Meta {
        Meta::new(
            "Create or overwrite a userscript in data/userscripts/<name>.ts (header with \
             @match is required). It runs on the next matching page load.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: UserscriptsSaveArgs) -> Result<UserscriptInfo, CmdError> {
        service(&ctx)?.save_userscript(&args.name, &args.code)
    }
}

pub struct UserscriptsGet;

#[derive(Debug, Serialize, JsonSchema)]
pub struct UserscriptsGetOutput {
    pub name: String,
    pub code: String,
}

impl Command for UserscriptsGet {
    const NAME: &'static str = "userscripts.get";
    type Args = UserscriptNameArgs;
    type Output = UserscriptsGetOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read the source of a userscript in data/userscripts (plugins' userscripts are \
             part of the plugin and not listed here).",
            Toolset::Script,
        )
    }

    async fn run(ctx: Ctx, args: UserscriptNameArgs) -> Result<UserscriptsGetOutput, CmdError> {
        let code = service(&ctx)?.userscript_code(&args.name)?;
        Ok(UserscriptsGetOutput {
            name: args.name,
            code,
        })
    }
}

pub struct UserscriptsDelete;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserscriptNameArgs {
    pub name: String,
}

impl Command for UserscriptsDelete {
    const NAME: &'static str = "userscripts.delete";
    type Args = UserscriptNameArgs;
    type Output = RemovedOutput;

    fn meta() -> Meta {
        Meta::new("Delete a userscript file.", Toolset::Script).act()
    }

    async fn run(ctx: Ctx, args: UserscriptNameArgs) -> Result<RemovedOutput, CmdError> {
        Ok(RemovedOutput {
            removed: service(&ctx)?.delete_userscript(&args.name)?,
        })
    }
}

pub struct UserscriptsEnable;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserscriptsEnableArgs {
    pub name: String,
    /// false = switch it off (default true).
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UserscriptsEnableOutput {
    pub name: String,
    pub enabled: bool,
}

impl Command for UserscriptsEnable {
    const NAME: &'static str = "userscripts.enable";
    type Args = UserscriptsEnableArgs;
    type Output = UserscriptsEnableOutput;

    fn meta() -> Meta {
        Meta::new(
            "Switch a userscript on or off (enabled: false) without deleting it.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(
        ctx: Ctx,
        args: UserscriptsEnableArgs,
    ) -> Result<UserscriptsEnableOutput, CmdError> {
        let enabled = args.enabled.unwrap_or(true);
        service(&ctx)?.enable_userscript(&args.name, enabled)?;
        Ok(UserscriptsEnableOutput {
            name: args.name,
            enabled,
        })
    }
}

pub struct UserscriptsInject;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserscriptsInjectArgs {
    /// Tab id (a userscript gets it as inputs.tab). Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Body of an async function run in the page's main frame, in an isolated world (same
    /// DOM, own globals). `return` a JSON value to get it back.
    pub js: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UserscriptsInjectOutput {
    pub tab: TabId,
    /// What the function returned (null for nothing).
    pub value: Value,
}

impl Command for UserscriptsInject {
    const NAME: &'static str = "userscripts.inject";
    type Args = UserscriptsInjectArgs;
    type Output = UserscriptsInjectOutput;

    fn meta() -> Meta {
        Meta::new(
            "Run JavaScript in an isolated world of a tab (shared DOM, separate globals; the \
             page cannot see it) and return its JSON result. Used by userscripts to change \
             pages, e.g. add a banner.",
            Toolset::Script,
        )
        .mutates_tab()
        .act()
    }

    fn target_tab(args: &UserscriptsInjectArgs) -> Option<TabId> {
        args.tab
    }

    async fn run(
        ctx: Ctx,
        args: UserscriptsInjectArgs,
    ) -> Result<UserscriptsInjectOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let value = crate::userscripts::inject(&ctx, tab, &args.js).await?;
        Ok(UserscriptsInjectOutput { tab, value })
    }
}

// ------------------------------------------------------------------ trigger.webhook_*

pub struct WebhookSet;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookSetArgs {
    /// Saved script (data/scripts/<name>.ts) to run on every call.
    pub script: String,
    /// Shared secret (at least 16 chars) the sender signs bodies with; default: random.
    pub secret: Option<String>,
    /// The JSON object body becomes the script's inputs (default true); false = the raw body
    /// arrives as inputs.body.
    pub inputs_from_body: Option<bool>,
    /// Fixed inputs added to every call (the body's keys win).
    pub inputs: Option<Map<String, Value>>,
    /// Replace the hook with this id (default: a new random id).
    pub id: Option<String>,
}

impl Command for WebhookSet {
    const NAME: &'static str = "trigger.webhook_set";
    type Args = WebhookSetArgs;
    type Output = HookCreated;

    fn meta() -> Meta {
        Meta::new(
            "Create or replace a webhook trigger: POST /v1/hooks/<id> with header \
             X-BMCP-Signature: sha256=<hex HMAC-SHA256 of the body with the secret> runs the \
             script as a one-row run and answers {run_id}. Returns the path and the secret. The \
             API listens on 127.0.0.1 only: outside services need a tunnel that rewrites Host.",
            Toolset::Run,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: WebhookSetArgs) -> Result<HookCreated, CmdError> {
        service(&ctx)?.set_hook(
            args.id,
            args.script,
            args.secret,
            args.inputs_from_body.unwrap_or(true),
            args.inputs.unwrap_or_default(),
        )
    }
}

pub struct WebhookList;

#[derive(Debug, Serialize, JsonSchema)]
pub struct WebhookListOutput {
    /// Without secrets.
    pub hooks: Vec<HookInfo>,
}

impl Command for WebhookList {
    const NAME: &'static str = "trigger.webhook_list";
    type Args = Empty;
    type Output = WebhookListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List webhook triggers (path, script, last firing and run id; secrets are not shown).",
            Toolset::Run,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<WebhookListOutput, CmdError> {
        Ok(WebhookListOutput {
            hooks: service(&ctx)?.hooks.list(),
        })
    }
}

pub struct WebhookRemove;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookIdArgs {
    pub id: String,
}

impl Command for WebhookRemove {
    const NAME: &'static str = "trigger.webhook_remove";
    type Args = WebhookIdArgs;
    type Output = RemovedOutput;

    fn meta() -> Meta {
        Meta::new("Delete a webhook trigger.", Toolset::Run).act()
    }

    async fn run(ctx: Ctx, args: WebhookIdArgs) -> Result<RemovedOutput, CmdError> {
        Ok(RemovedOutput {
            removed: service(&ctx)?.hooks.remove(&args.id)?,
        })
    }
}

pub struct WebhookFire;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookFireArgs {
    pub id: String,
    /// The X-BMCP-Signature header value.
    pub signature: String,
    /// The raw request body.
    pub body: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct WebhookFireOutput {
    /// See run.status / run.results.
    pub run_id: String,
    pub script: String,
}

impl Command for WebhookFire {
    const NAME: &'static str = bmcp_api::rest::WEBHOOK_COMMAND;
    type Args = WebhookFireArgs;
    type Output = WebhookFireOutput;

    fn meta() -> Meta {
        Meta::new(
            "Internal: behind POST /v1/hooks/{id}. Checks the signature and starts the run.",
            Toolset::Hidden,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: WebhookFireArgs) -> Result<WebhookFireOutput, CmdError> {
        let (run_id, script) = service(&ctx)?
            .fire_hook(&ctx, &args.id, &args.signature, &args.body)
            .await?;
        Ok(WebhookFireOutput { run_id, script })
    }
}
