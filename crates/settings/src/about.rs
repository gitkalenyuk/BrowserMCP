//! `system.about`: versions, folders, the API endpoints and the license files (Settings →
//! About, "Copy diagnostics").

use std::path::{Path, PathBuf};

use bmcp_core::{CmdError, Command, Ctx, DataDir, Meta, Toolset};
use schemars::JsonSchema;
use serde::Serialize;

/// What only the app knows, put into the registry extensions at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppInfo {
    /// e.g. `156.0.15+gcf671c0+chromium-156.0.8078.80`.
    pub cef_version: String,
    /// e.g. `156.0.8078.80`.
    pub chromium_version: String,
    /// Folder of browsermcp.exe.
    pub exe_dir: PathBuf,
    pub api_port: u16,
    pub mcp_url: String,
    pub rest_url: String,
    pub ws_url: String,
    /// Unix ms when the browser started.
    pub started_ms: i64,
}

/// `156.0.15+gcf671c0+chromium-156.0.8078.80` → `156.0.8078.80`.
pub fn chromium_from_cef(cef: &str) -> String {
    cef.split_once("chromium-")
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct LicenseFile {
    /// File or folder name, e.g. `CREDITS.html`.
    pub name: String,
    pub path: String,
}

/// License and credits files shipped next to the exe (`LICENSE*`, `LICENCE*`, `NOTICE*`,
/// `THIRD_PARTY*`, `CREDITS*`, a `licenses` folder), sorted by name.
pub fn license_files(exe_dir: &Path) -> Vec<LicenseFile> {
    const PREFIXES: &[&str] = &[
        "license",
        "licence",
        "notice",
        "third_party",
        "third-party",
        "credits",
    ];
    let mut found: Vec<LicenseFile> = std::fs::read_dir(exe_dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    let lower = name.to_ascii_lowercase();
                    PREFIXES
                        .iter()
                        .any(|p| lower.starts_with(p))
                        .then(|| LicenseFile {
                            path: e.path().display().to_string(),
                            name,
                        })
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ApiInfo {
    /// 127.0.0.1 port of MCP / REST / WebSocket.
    pub port: u16,
    pub mcp: String,
    pub rest: String,
    pub ws: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AboutOutput {
    pub name: String,
    /// BrowserMCP version.
    pub version: String,
    pub cef_version: String,
    pub chromium_version: String,
    /// e.g. "windows x86_64".
    pub os: String,
    pub pid: u32,
    /// Seconds since the browser started.
    pub uptime_secs: u64,
    pub exe_dir: String,
    pub data_dir: String,
    pub config_dir: String,
    pub logs_dir: String,
    pub api: ApiInfo,
    pub licenses: Vec<LicenseFile>,
}

pub struct About;

impl Command for About {
    const NAME: &'static str = "system.about";
    type Args = bmcp_core::command::Empty;
    type Output = AboutOutput;

    fn meta() -> Meta {
        Meta::new(
            "About this browser: BrowserMCP, CEF and Chromium versions, OS, process id and \
             uptime, the exe / data / config / logs folders, the API endpoints (MCP, REST, \
             WebSocket) and the license files next to the exe.",
            Toolset::System,
        )
    }

    async fn run(ctx: Ctx, _args: bmcp_core::command::Empty) -> Result<AboutOutput, CmdError> {
        let app = ctx
            .extensions
            .get::<AppInfo>()
            .map(|a| (*a).clone())
            .unwrap_or_default();
        let data = ctx
            .extensions
            .get::<DataDir>()
            .map(|d| (*d).clone())
            .unwrap_or_else(|| DataDir::from_root(app.exe_dir.join("data")));
        let uptime_ms = (bmcp_data::store::now_ms() - app.started_ms).max(0);
        Ok(AboutOutput {
            name: "BrowserMCP".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            cef_version: app.cef_version.clone(),
            chromium_version: app.chromium_version.clone(),
            os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            pid: std::process::id(),
            uptime_secs: if app.started_ms > 0 {
                uptime_ms as u64 / 1000
            } else {
                0
            },
            exe_dir: app.exe_dir.display().to_string(),
            data_dir: data.root().display().to_string(),
            config_dir: data.config().display().to_string(),
            logs_dir: data.logs().display().to_string(),
            api: ApiInfo {
                port: app.api_port,
                mcp: app.mcp_url.clone(),
                rest: app.rest_url.clone(),
                ws: app.ws_url.clone(),
            },
            licenses: license_files(&app.exe_dir),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_version_comes_from_the_cef_string() {
        assert_eq!(
            chromium_from_cef("156.0.15+gcf671c0+chromium-156.0.8078.80"),
            "156.0.8078.80"
        );
        assert_eq!(chromium_from_cef("garbage"), "");
    }

    #[test]
    fn finds_license_files_next_to_the_exe() {
        let tmp = tempfile::tempdir().unwrap();
        for name in [
            "CREDITS.html",
            "LICENSE.txt",
            "libcef.dll",
            "browsermcp.exe",
        ] {
            std::fs::write(tmp.path().join(name), "x").unwrap();
        }
        std::fs::create_dir(tmp.path().join("THIRD_PARTY")).unwrap();
        let names: Vec<String> = license_files(tmp.path())
            .into_iter()
            .map(|l| l.name)
            .collect();
        assert_eq!(names, ["CREDITS.html", "LICENSE.txt", "THIRD_PARTY"]);
    }
}
