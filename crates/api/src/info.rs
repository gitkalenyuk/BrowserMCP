//! `api.info` and `api.config`: how AI clients and automation tools connect to this browser
//! (Settings → Connections & API).
//!
//! * [`ApiEndpoint`] — where the API listens and how many commands each toolset has; put into
//!   the registry extensions by [`crate::serve_on`] once the socket is bound.
//! * [`CdpLaunch`] (from bmcp-core) — what the app did with Chromium's remote debugging port
//!   at launch.
//! * [`ConnectionsConfig`] — `data/config/connections.json`: the CDP port setting the app reads
//!   at the next start (`--cdp-port` on the command line wins).
//!
//! The CDP port and a stealth identity are never combined: an open debugging port is easy for
//! sites and local software to detect. `api.config` refuses to turn the port on while a
//! profile has an identity, the app refuses to open it at startup in that case, and while it
//! is open the identity commands refuse to assign one ([`CdpLaunch::check_stealth_allowed`]).
//!
//! Only a person in the browser UI may turn the port on (`api.config` from an MCP agent or a
//! script is refused): the UI asks for confirmation first, an agent would not.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

pub use bmcp_core::CdpLaunch;
use bmcp_core::command::Empty;
use bmcp_core::{Caller, CmdError, Command, Ctx, DataDir, ErrorCode, Meta, Registry, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mcp::DEFAULT_TOOLSETS;

/// File name of the connection settings in `data/config/`.
pub const CONNECTIONS_FILE: &str = "connections.json";
/// The usual Chrome DevTools port (what Playwright/Puppeteer examples use).
pub const DEFAULT_CDP_PORT: u16 = 9222;

pub fn register(registry: &mut Registry) {
    registry.add::<ApiInfo>().add::<ApiConfig>();
}

// ---------------------------------------------------------------------------------------------
// Runtime facts (extensions)
// ---------------------------------------------------------------------------------------------

/// Where the API listens and what it serves. Inserted by [`crate::serve_on`].
#[derive(Debug, Clone)]
pub struct ApiEndpoint {
    pub addr: SocketAddr,
    /// Commands per toolset (every toolset, also the empty ones).
    pub toolsets: Vec<(Toolset, usize)>,
}

impl ApiEndpoint {
    pub fn new(addr: SocketAddr, registry: &Registry) -> Self {
        let toolsets = Toolset::ALL
            .into_iter()
            .map(|t| (t, registry.list(Some(&[t])).len()))
            .collect();
        ApiEndpoint { addr, toolsets }
    }
}

// ---------------------------------------------------------------------------------------------
// connections.json
// ---------------------------------------------------------------------------------------------

/// `data/config/connections.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConnectionsConfig {
    /// Open Chromium's remote debugging (CDP) port at startup, for Playwright / Puppeteer.
    #[serde(default)]
    pub cdp_enabled: bool,
    /// The CDP port (1024-65535, default 9222).
    #[serde(default = "default_cdp_port")]
    pub cdp_port: u16,
}

fn default_cdp_port() -> u16 {
    DEFAULT_CDP_PORT
}

impl Default for ConnectionsConfig {
    fn default() -> Self {
        ConnectionsConfig {
            cdp_enabled: false,
            cdp_port: DEFAULT_CDP_PORT,
        }
    }
}

impl ConnectionsConfig {
    pub fn path(config_dir: &Path) -> PathBuf {
        config_dir.join(CONNECTIONS_FILE)
    }

    /// The stored settings (defaults when there is no file).
    pub fn load(config_dir: &Path) -> Result<Self, CmdError> {
        bmcp_core::config_file::load(&Self::path(config_dir))
    }

    pub fn save(&self, config_dir: &Path) -> Result<(), CmdError> {
        bmcp_core::config_file::save(&Self::path(config_dir), self)
    }

    /// The CDP port to ask for at startup: `--cdp-port` wins, else the saved setting.
    pub fn startup_cdp_port(&self, cli: Option<u16>) -> Option<u16> {
        cli.or(self.cdp_enabled.then_some(self.cdp_port))
    }
}

fn check_cdp_port(port: u16) -> Result<(), CmdError> {
    if port < 1024 {
        return Err(CmdError::invalid_params(format!(
            "cdp_port must be 1024-65535, got {port}"
        )));
    }
    Ok(())
}

/// Profiles with a stealth identity (`identity.assign`), sorted.
fn stealth_profiles(ctx: &Ctx) -> Result<Vec<String>, CmdError> {
    match bmcp_identity::Services::from_ctx(ctx) {
        Ok(services) => Ok(services.identities.read()?.profiles.into_keys().collect()),
        // No identity storage (schema dump, tests): nobody uses stealth.
        Err(_) => Ok(Vec::new()),
    }
}

fn config_dir(ctx: &Ctx) -> Result<PathBuf, CmdError> {
    ctx.extensions
        .get::<DataDir>()
        .map(|d| d.config())
        .ok_or_else(|| CmdError::internal("the data folder is not known (no DataDir extension)"))
}

// ---------------------------------------------------------------------------------------------
// Outputs
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApiUrls {
    /// MCP over Streamable HTTP, e.g. "http://127.0.0.1:47800/mcp". Add `?toolsets=core,net`
    /// to list more toolsets.
    pub mcp: String,
    /// REST base, e.g. "http://127.0.0.1:47800/v1" (POST /v1/cmd/{name}).
    pub rest: String,
    /// JSON-RPC 2.0 over WebSocket + events.
    pub ws: String,
    /// OpenAPI 3.1 description of the REST API.
    pub openapi: String,
    /// GET → {ok, name, version}.
    pub health: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ToolsetInfo {
    /// Name for `?toolsets=`, e.g. "net".
    pub name: String,
    /// Commands in it.
    pub commands: usize,
    /// Listed by MCP when the URL names no toolsets.
    pub default: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CdpInfo {
    /// The port Chromium listens on now (null = closed).
    pub active_port: Option<u16>,
    /// A port was asked for at launch but refused because a profile has a stealth identity.
    pub refused: bool,
    /// The saved setting (connections.json) used at the next start.
    pub enabled: bool,
    pub port: u16,
    /// Profiles with a stealth identity: while there are any, the port stays closed.
    pub stealth_profiles: Vec<String>,
    /// The saved setting differs from what is running: restart the browser to apply it.
    /// Always false when `from_flag` (the flag decides at every start).
    pub restart_needed: bool,
    /// The port was asked for with `--cdp-port`: the saved setting is not used while the
    /// browser is started with that flag.
    pub from_flag: bool,
    /// connections.json could not be read (the defaults are shown); fix or delete the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_error: Option<String>,
}

impl CdpInfo {
    fn new(config: &ConnectionsConfig, launch: &CdpLaunch, stealth: Vec<String>) -> Self {
        let wanted = config.startup_cdp_port(None);
        let effective = if stealth.is_empty() { wanted } else { None };
        CdpInfo {
            active_port: launch.port,
            refused: launch.refused,
            enabled: config.cdp_enabled,
            port: config.cdp_port,
            // With `--cdp-port` the next start (with the same flag) opens the same port again,
            // so the saved setting changes nothing a restart would show.
            restart_needed: !launch.from_cli && effective != launch.port,
            from_flag: launch.from_cli,
            stealth_profiles: stealth,
            config_error: None,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApiInfoOutput {
    /// BrowserMCP version.
    pub version: String,
    /// The API server is listening (false only while starting).
    pub listening: bool,
    /// The API port (default 47800, `--api-port`).
    pub port: Option<u16>,
    pub urls: Option<ApiUrls>,
    /// `data/runtime/endpoint.json`: how tools (and `browsermcp mcp-stdio`) find this instance.
    pub endpoint_file: Option<String>,
    /// The file exists (written once the API listens, removed on exit).
    pub endpoint_present: bool,
    /// Full path of browsermcp.exe (for the stdio MCP command).
    pub exe: Option<String>,
    /// The portable data folder.
    pub data_dir: Option<String>,
    /// MCP toolsets with their command counts.
    pub toolsets: Vec<ToolsetInfo>,
    pub cdp: CdpInfo,
}

// ---------------------------------------------------------------------------------------------
// api.info
// ---------------------------------------------------------------------------------------------

/// `api.info`.
pub struct ApiInfo;

impl Command for ApiInfo {
    const NAME: &'static str = "api.info";
    type Args = Empty;
    type Output = ApiInfoOutput;

    fn meta() -> Meta {
        Meta::new(
            "How to connect to this browser: API port, MCP / REST / WebSocket / OpenAPI URLs, \
             endpoint.json, the exe path (for MCP over stdio), MCP toolsets with command \
             counts, and the CDP port for Playwright/Puppeteer (running now, saved setting, \
             profiles whose stealth identity keeps it closed).",
            Toolset::System,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<ApiInfoOutput, CmdError> {
        let endpoint = ctx.extensions.get::<ApiEndpoint>();
        let data = ctx.extensions.get::<DataDir>();
        let launch = CdpLaunch::of(&ctx.extensions);
        // A broken connections.json must not hide the URLs and commands: show the defaults
        // and say what is wrong.
        let (config, config_error) = match &data {
            Some(data) => match ConnectionsConfig::load(&data.config()) {
                Ok(config) => (config, None),
                Err(err) => (ConnectionsConfig::default(), Some(err.message)),
            },
            None => (ConnectionsConfig::default(), None),
        };
        let mut cdp = CdpInfo::new(&config, &launch, stealth_profiles(&ctx)?);
        cdp.config_error = config_error;
        let endpoint_file = data.as_ref().map(|d| d.runtime().join("endpoint.json"));
        Ok(ApiInfoOutput {
            version: env!("CARGO_PKG_VERSION").into(),
            listening: endpoint.is_some(),
            port: endpoint.as_ref().map(|e| e.addr.port()),
            urls: endpoint.as_ref().map(|e| urls(e.addr)),
            endpoint_present: endpoint_file.as_ref().is_some_and(|f| f.is_file()),
            endpoint_file: endpoint_file.map(|f| f.display().to_string()),
            exe: std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string()),
            data_dir: data.as_ref().map(|d| d.root().display().to_string()),
            toolsets: toolsets(endpoint.as_deref()),
            cdp,
        })
    }
}

fn urls(addr: SocketAddr) -> ApiUrls {
    ApiUrls {
        mcp: format!("http://{addr}/mcp"),
        rest: format!("http://{addr}/v1"),
        ws: format!("ws://{addr}/v1/ws"),
        openapi: format!("http://{addr}/openapi.json"),
        health: format!("http://{addr}/v1/health"),
    }
}

/// Every toolset a client may ask for (not `hidden`), with its count when the API listens.
fn toolsets(endpoint: Option<&ApiEndpoint>) -> Vec<ToolsetInfo> {
    Toolset::ALL
        .into_iter()
        .filter(|t| *t != Toolset::Hidden)
        .map(|t| ToolsetInfo {
            name: t.as_str().into(),
            commands: endpoint
                .and_then(|e| e.toolsets.iter().find(|(x, _)| *x == t))
                .map_or(0, |(_, n)| *n),
            default: DEFAULT_TOOLSETS.contains(&t),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// api.config
// ---------------------------------------------------------------------------------------------

/// `api.config`.
pub struct ApiConfig;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiConfigArgs {
    /// Open the CDP port at the next start (refused while a profile has a stealth identity).
    /// Only the browser UI may turn it on (a person confirms it there); agents may turn it off.
    pub cdp_enabled: Option<bool>,
    /// CDP port, 1024-65535 (default 9222).
    pub cdp_port: Option<u16>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApiConfigOutput {
    /// The saved settings.
    pub config: ConnectionsConfig,
    pub cdp: CdpInfo,
}

impl Command for ApiConfig {
    const NAME: &'static str = "api.config";
    type Args = ApiConfigArgs;
    type Output = ApiConfigOutput;

    fn meta() -> Meta {
        Meta::new(
            "Change the connection settings (data/config/connections.json): open the CDP \
             port for Playwright/Puppeteer at the next start, and its port. Turning the port \
             on is only allowed from the browser UI (Settings → Connections & API) and is \
             refused while a profile has a stealth identity. Takes effect after a restart; \
             returns the saved settings and the CDP state.",
            Toolset::System,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ApiConfigArgs) -> Result<ApiConfigOutput, CmdError> {
        let dir = config_dir(&ctx)?;
        let mut config = ConnectionsConfig::load(&dir)?;
        if let Some(port) = args.cdp_port {
            check_cdp_port(port)?;
            config.cdp_port = port;
        }
        let stealth = stealth_profiles(&ctx)?;
        if let Some(enabled) = args.cdp_enabled {
            if enabled && !config.cdp_enabled {
                check_may_open_port(&ctx.caller)?;
            }
            if enabled && !stealth.is_empty() {
                return Err(CmdError::new(
                    ErrorCode::InvalidParams,
                    format!(
                        "the CDP port cannot be opened while these profiles use a stealth \
                         identity: {}",
                        stealth.join(", ")
                    ),
                )
                .with_hint("Remove their identity (identity_assign without id) first."));
            }
            config.cdp_enabled = enabled;
        }
        config.save(&dir)?;
        let launch = CdpLaunch::of(&ctx.extensions);
        Ok(ApiConfigOutput {
            cdp: CdpInfo::new(&config, &launch, stealth),
            config,
        })
    }
}

/// Opening the debugging port gives any local program full control of the browser, so a
/// person decides it in the UI (after a warning); agents and scripts may only close it.
fn check_may_open_port(caller: &Caller) -> Result<(), CmdError> {
    if matches!(caller, Caller::Ui) {
        return Ok(());
    }
    Err(CmdError::new(
        ErrorCode::NeedsApproval,
        "only a person in the browser UI may turn the CDP port on",
    )
    .with_hint("Ask the user to turn it on in Settings → Connections & API (it needs a restart)."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_port_prefers_the_flag_then_the_setting() {
        let off = ConnectionsConfig::default();
        assert_eq!(off.startup_cdp_port(None), None);
        assert_eq!(off.startup_cdp_port(Some(9333)), Some(9333));
        let on = ConnectionsConfig {
            cdp_enabled: true,
            cdp_port: 9229,
        };
        assert_eq!(on.startup_cdp_port(None), Some(9229));
        assert_eq!(on.startup_cdp_port(Some(9333)), Some(9333));
    }

    #[test]
    fn config_file_defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            ConnectionsConfig::load(dir.path()).unwrap(),
            ConnectionsConfig::default()
        );
        // Older or hand-written files may leave fields out.
        std::fs::write(dir.path().join(CONNECTIONS_FILE), r#"{"cdp_enabled":true}"#).unwrap();
        let loaded = ConnectionsConfig::load(dir.path()).unwrap();
        assert_eq!(loaded.cdp_port, DEFAULT_CDP_PORT);
        assert!(loaded.cdp_enabled);
    }

    #[test]
    fn restart_needed_compares_the_setting_with_the_launch() {
        let on = ConnectionsConfig {
            cdp_enabled: true,
            cdp_port: 9222,
        };
        let closed = CdpLaunch::default();
        let open = CdpLaunch {
            port: Some(9222),
            ..Default::default()
        };
        assert!(CdpInfo::new(&on, &closed, vec![]).restart_needed);
        assert!(!CdpInfo::new(&on, &open, vec![]).restart_needed);
        assert!(CdpInfo::new(&ConnectionsConfig::default(), &open, vec![]).restart_needed);
        // Stealth keeps it closed whatever the setting says: nothing a restart would change.
        assert!(!CdpInfo::new(&on, &closed, vec!["shop".into()]).restart_needed);
        // Refused at launch, identity removed since: a restart opens it.
        let refused = CdpLaunch {
            refused: true,
            ..Default::default()
        };
        assert!(CdpInfo::new(&on, &refused, vec![]).restart_needed);
        assert!(!CdpInfo::new(&on, &refused, vec!["shop".into()]).restart_needed);
    }

    #[test]
    fn a_port_from_the_flag_never_needs_a_restart() {
        // `--cdp-port` with the setting off (the e2e harness): the same flag reopens it.
        let flag = CdpLaunch {
            port: Some(9333),
            from_cli: true,
            ..Default::default()
        };
        let info = CdpInfo::new(&ConnectionsConfig::default(), &flag, vec![]);
        assert!(!info.restart_needed);
        assert!(info.from_flag);
        let on = ConnectionsConfig {
            cdp_enabled: true,
            cdp_port: 9222,
        };
        assert!(!CdpInfo::new(&on, &flag, vec![]).restart_needed);
    }

    #[test]
    fn only_the_ui_may_open_the_port() {
        assert!(check_may_open_port(&Caller::Ui).is_ok());
        for caller in [
            Caller::Rest,
            Caller::Ws { conn: 1 },
            Caller::Mcp {
                session: "s".into(),
                agent: None,
            },
            Caller::Keymap,
        ] {
            let err = check_may_open_port(&caller).unwrap_err();
            assert_eq!(err.code, ErrorCode::NeedsApproval, "{caller:?}");
        }
    }

    #[test]
    fn toolsets_list_every_public_toolset_with_defaults() {
        let list = toolsets(None);
        assert!(list.iter().all(|t| t.name != "hidden"));
        assert!(list.iter().any(|t| t.name == "core" && t.default));
        assert!(list.iter().any(|t| t.name == "net" && !t.default));
    }

    async fn call_as(
        registry: &Registry,
        caller: Caller,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, CmdError> {
        let ctx = Ctx::new(
            caller,
            bmcp_core::testing::FakeBackend::new(),
            bmcp_core::EventBus::new(),
        );
        registry.call(ctx, name, args).await
    }

    /// A call from Settings (the browser UI).
    async fn call(
        registry: &Registry,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, CmdError> {
        call_as(registry, Caller::Ui, name, args).await
    }

    #[tokio::test]
    async fn info_and_config_through_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::from_root(dir.path());
        let mut registry = Registry::new();
        register(&mut registry);
        registry.extensions().insert(data.clone());
        bmcp_identity::install(&registry, data.config());

        // Before the server listens: no URLs, but the toolsets and settings are there.
        let info = call(&registry, "api.info", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(info["listening"], false);
        assert_eq!(info["urls"], serde_json::Value::Null);
        assert_eq!(info["cdp"]["enabled"], false);
        assert_eq!(info["cdp"]["port"], 9222);
        assert!(
            info["endpoint_file"]
                .as_str()
                .unwrap()
                .ends_with("endpoint.json")
        );

        let addr: SocketAddr = "127.0.0.1:47811".parse().unwrap();
        registry
            .extensions()
            .insert(ApiEndpoint::new(addr, &registry));
        let info = call(&registry, "api.info", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(info["port"], 47811);
        assert_eq!(info["urls"]["mcp"], "http://127.0.0.1:47811/mcp");
        assert_eq!(
            info["urls"]["openapi"],
            "http://127.0.0.1:47811/openapi.json"
        );
        let system = info["toolsets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "system")
            .unwrap();
        assert_eq!(system["commands"], 2, "api.info + api.config");

        // An agent may not open the port (no confirmation dialog for it), only a person may.
        let err = call_as(
            &registry,
            Caller::Rest,
            "api.config",
            serde_json::json!({ "cdp_enabled": true }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::NeedsApproval);
        assert!(!ConnectionsConfig::load(&data.config()).unwrap().cdp_enabled);

        let saved = call(
            &registry,
            "api.config",
            serde_json::json!({ "cdp_enabled": true, "cdp_port": 9229 }),
        )
        .await
        .unwrap();
        assert_eq!(saved["config"]["cdp_port"], 9229);
        assert_eq!(saved["cdp"]["restart_needed"], true);
        assert_eq!(
            ConnectionsConfig::load(&data.config()).unwrap().cdp_port,
            9229
        );
        let err = call(
            &registry,
            "api.config",
            serde_json::json!({ "cdp_port": 80 }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidParams);

        // Agents may close it again, and repeat "on" once it is on.
        call_as(
            &registry,
            Caller::Rest,
            "api.config",
            serde_json::json!({ "cdp_enabled": true }),
        )
        .await
        .unwrap();
        // A profile with a stealth identity: the port cannot be turned on.
        call_as(
            &registry,
            Caller::Rest,
            "api.config",
            serde_json::json!({ "cdp_enabled": false }),
        )
        .await
        .unwrap();
        let services = registry
            .extensions()
            .get::<bmcp_identity::Services>()
            .unwrap();
        services
            .identities
            .update(|f| {
                f.profiles.insert("shop".into(), "win-1".into());
                Ok(())
            })
            .unwrap();
        let err = call(
            &registry,
            "api.config",
            serde_json::json!({ "cdp_enabled": true }),
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("shop"), "{}", err.message);
        let info = call(&registry, "api.info", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(info["cdp"]["stealth_profiles"], serde_json::json!(["shop"]));
    }

    #[tokio::test]
    async fn a_broken_settings_file_still_shows_the_urls() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::from_root(dir.path());
        std::fs::create_dir_all(data.config()).unwrap();
        std::fs::write(ConnectionsConfig::path(&data.config()), "{not json").unwrap();
        let mut registry = Registry::new();
        register(&mut registry);
        registry.extensions().insert(data.clone());
        let addr: SocketAddr = "127.0.0.1:47812".parse().unwrap();
        registry
            .extensions()
            .insert(ApiEndpoint::new(addr, &registry));
        let info = call(&registry, "api.info", serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(info["urls"]["mcp"], "http://127.0.0.1:47812/mcp");
        assert_eq!(info["cdp"]["enabled"], false);
        let error = info["cdp"]["config_error"].as_str().unwrap();
        assert!(error.contains("connections.json"), "{error}");
        // Saving still refuses to overwrite what the user wrote.
        assert!(
            call(
                &registry,
                "api.config",
                serde_json::json!({ "cdp_port": 9333 })
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn low_cdp_ports_are_refused() {
        assert!(check_cdp_port(80).is_err());
        assert!(check_cdp_port(9222).is_ok());
    }
}
