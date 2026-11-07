//! `api.info` and `api.config`: how AI clients and automation tools connect to this browser
//! (Settings → Connections & API).
//!
//! * [`ApiEndpoint`] — where the API listens and how many commands each toolset has; put into
//!   the registry extensions by [`crate::serve_on`] once the socket is bound.
//! * [`CdpLaunch`] — what the app did with Chromium's remote debugging port at launch.
//! * [`ConnectionsConfig`] — `data/config/connections.json`: the CDP port setting the app reads
//!   at the next start (`--cdp-port` on the command line wins).
//!
//! The CDP port and a stealth identity are never combined: an open debugging port is easy for
//! sites and local software to detect. `api.config` refuses to turn the port on while a
//! profile has an identity, and the app refuses to open it at startup in that case.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use bmcp_core::command::Empty;
use bmcp_core::{CmdError, Command, Ctx, DataDir, ErrorCode, Meta, Registry, Toolset};
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

/// What the app did with the CDP port at launch. Inserted by the app; absent = closed.
#[derive(Debug, Clone, Default)]
pub struct CdpLaunch {
    /// The port Chromium listens on, if any.
    pub port: Option<u16>,
    /// A port was asked for (flag or setting) but refused because of a stealth identity.
    pub refused: bool,
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
    pub restart_needed: bool,
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
            restart_needed: effective != launch.port,
            stealth_profiles: stealth,
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
        let launch = ctx
            .extensions
            .get::<CdpLaunch>()
            .map(|l| (*l).clone())
            .unwrap_or_default();
        let config = match &data {
            Some(data) => ConnectionsConfig::load(&data.config())?,
            None => ConnectionsConfig::default(),
        };
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
            cdp: CdpInfo::new(&config, &launch, stealth_profiles(&ctx)?),
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
             port for Playwright/Puppeteer at the next start, and its port. Refused while a \
             profile has a stealth identity. Takes effect after a restart; returns the saved \
             settings and the CDP state.",
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
        let launch = ctx
            .extensions
            .get::<CdpLaunch>()
            .map(|l| (*l).clone())
            .unwrap_or_default();
        Ok(ApiConfigOutput {
            cdp: CdpInfo::new(&config, &launch, stealth),
            config,
        })
    }
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
            refused: false,
        };
        assert!(CdpInfo::new(&on, &closed, vec![]).restart_needed);
        assert!(!CdpInfo::new(&on, &open, vec![]).restart_needed);
        assert!(CdpInfo::new(&ConnectionsConfig::default(), &open, vec![]).restart_needed);
        // Stealth keeps it closed whatever the setting says: nothing a restart would change.
        assert!(!CdpInfo::new(&on, &closed, vec!["shop".into()]).restart_needed);
        // Refused at launch, identity removed since: a restart opens it.
        let refused = CdpLaunch {
            port: None,
            refused: true,
        };
        assert!(CdpInfo::new(&on, &refused, vec![]).restart_needed);
        assert!(!CdpInfo::new(&on, &refused, vec!["shop".into()]).restart_needed);
    }

    #[test]
    fn toolsets_list_every_public_toolset_with_defaults() {
        let list = toolsets(None);
        assert!(list.iter().all(|t| t.name != "hidden"));
        assert!(list.iter().any(|t| t.name == "core" && t.default));
        assert!(list.iter().any(|t| t.name == "net" && !t.default));
    }

    async fn call(
        registry: &Registry,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, CmdError> {
        let ctx = Ctx::new(
            bmcp_core::Caller::Rest,
            bmcp_core::testing::FakeBackend::new(),
            bmcp_core::EventBus::new(),
        );
        registry.call(ctx, name, args).await
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

        // A profile with a stealth identity: the port cannot be turned on.
        call(
            &registry,
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

    #[test]
    fn low_cdp_ports_are_refused() {
        assert!(check_cdp_port(80).is_err());
        assert!(check_cdp_port(9222).is_ok());
    }
}
