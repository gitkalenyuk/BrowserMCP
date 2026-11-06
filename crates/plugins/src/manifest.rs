//! `plugin.json`: what a plugin is and what it may do.
//!
//! ```json
//! {
//!   "name": "hello-plugin",
//!   "version": "0.1.0",
//!   "description": "Logs page loads",
//!   "main": "main.ts",
//!   "permissions": ["tabs.list", "page.*"],
//!   "events": ["tab.updated"],
//!   "userscripts": ["banner.user.ts"],
//!   "ui": { "palette": [{ "id": "count-tabs", "title": "Hello: count tabs" }] }
//! }
//! ```
//!
//! * `name` — 1-64 chars `a-z 0-9 _ -`, starting with a letter or digit; also the folder name
//!   in `data/plugins/`;
//! * `main` — the long-lived TypeScript entry, relative to the plugin folder;
//! * `permissions` — command globs the plugin may call (`*` matches anything, see
//!   [`bmcp_api::access::glob_match`]); nothing else is allowed;
//! * `events` — event topic globs it may receive (`plugin.command` for its own palette
//!   entries is always delivered);
//! * `userscripts` — userscript files in the plugin folder, run on matching page loads with
//!   the plugin's permissions;
//! * `ui.palette` — Ctrl+K entries; choosing one sends `plugin.command {plugin, id}`.

use std::path::{Component, Path};

use bmcp_api::access::glob_match;
use bmcp_core::CmdError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File name of the manifest inside a plugin folder.
pub const MANIFEST_FILE: &str = "plugin.json";

/// A parsed and validated `plugin.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Entry script, relative to the plugin folder.
    pub main: String,
    /// Command globs, e.g. `["page.*", "net.rule_add"]`.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Event topic globs, e.g. `["tab.updated"]`.
    #[serde(default)]
    pub events: Vec<String>,
    /// Userscript files (with `// @match` headers), relative to the plugin folder.
    #[serde(default)]
    pub userscripts: Vec<String>,
    #[serde(default)]
    pub ui: Ui,
}

/// What the plugin adds to the browser UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Ui {
    /// Ctrl+K palette entries.
    #[serde(default, alias = "palette_commands", alias = "commands")]
    pub palette: Vec<PaletteEntry>,
}

/// One Ctrl+K entry of a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaletteEntry {
    /// Sent back to the plugin as `plugin.command {plugin, id}`.
    pub id: String,
    /// Shown in the palette.
    pub title: String,
}

/// Commands that let a plugin escape its permissions (run unrestricted code, install other
/// plugins, read secrets…). Granting a glob that covers one of them is reported as a warning.
const POWERFUL: &[&str] = &[
    "script.run",
    "script.save",
    "run.start",
    "trigger.set",
    "trigger.webhook_set",
    "plugins.install",
    "userscripts.save",
    "ai.session_start",
    "term.open",
    "passwords.get",
];

impl Manifest {
    /// Parse and validate `plugin.json` text.
    pub fn parse(text: &str) -> Result<Manifest, CmdError> {
        let manifest: Manifest = serde_json::from_str(text)
            .map_err(|e| CmdError::invalid_params(format!("bad {MANIFEST_FILE}: {e}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Read `<dir>/plugin.json` and check that `main` and the userscripts exist.
    pub fn load(dir: &Path) -> Result<Manifest, CmdError> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| CmdError::invalid_params(format!("{}: {e}", path.display())))?;
        let manifest = Manifest::parse(&text)?;
        for file in std::iter::once(&manifest.main).chain(&manifest.userscripts) {
            if !dir.join(file).is_file() {
                return Err(CmdError::invalid_params(format!(
                    "plugin {}: {file} not found in {}",
                    manifest.name,
                    dir.display()
                )));
            }
        }
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), CmdError> {
        validate_name(&self.name)?;
        if self.version.trim().is_empty() {
            return Err(invalid(&self.name, "`version` is empty"));
        }
        for file in std::iter::once(&self.main).chain(&self.userscripts) {
            if !is_inside(file) {
                return Err(invalid(
                    &self.name,
                    &format!("{file:?} must be a relative path inside the plugin folder"),
                ));
            }
            if !(file.ends_with(".ts") || file.ends_with(".js")) {
                return Err(invalid(
                    &self.name,
                    &format!("{file:?} must be a .ts or .js file"),
                ));
            }
        }
        for glob in self.permissions.iter().chain(&self.events) {
            if !valid_glob(glob) {
                return Err(invalid(
                    &self.name,
                    &format!(
                        "bad glob {glob:?}: use dotted names with `*`, e.g. \"page.*\" or \
                         \"tab.updated\""
                    ),
                ));
            }
        }
        for entry in &self.ui.palette {
            if entry.id.is_empty() || entry.id.len() > 64 || entry.title.trim().is_empty() {
                return Err(invalid(
                    &self.name,
                    "every ui.palette entry needs an `id` (1-64 chars) and a `title`",
                ));
            }
        }
        Ok(())
    }

    /// May this plugin call `command`? (Same rule the API enforces.)
    pub fn allows_command(&self, command: &str) -> bool {
        self.permissions.iter().any(|g| glob_match(g, command))
    }

    /// Human-readable warnings about permissions that amount to full access.
    pub fn warnings(&self) -> Vec<String> {
        POWERFUL
            .iter()
            .filter(|cmd| self.allows_command(cmd))
            .map(|cmd| {
                format!(
                    "permission for {cmd} lets this plugin act without restrictions (it can \
                     start unrestricted code or read secrets)"
                )
            })
            .collect()
    }
}

/// Plugin (and folder) names: `a-z 0-9 _ -`, 1-64 chars, not starting with `-`/`_`.
pub fn validate_name(name: &str) -> Result<(), CmdError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(CmdError::invalid_params(format!(
            "bad plugin name {name:?}: use 1-64 chars of a-z 0-9 _ - (e.g. \"hello-plugin\")"
        )))
    }
}

/// A relative path without `..`, drive letters or a root.
fn is_inside(file: &str) -> bool {
    let path = Path::new(file);
    !file.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

/// `*` alone, or lowercase dotted names where `*` may stand anywhere.
fn valid_glob(glob: &str) -> bool {
    !glob.is_empty()
        && glob.len() <= 128
        && glob
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '*'))
}

fn invalid(name: &str, message: &str) -> CmdError {
    CmdError::invalid_params(format!("plugin {name}: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = r#"{
        "name": "hello-plugin", "version": "0.1.0", "description": "d", "main": "main.ts",
        "permissions": ["tabs.list", "page.*"], "events": ["tab.updated"],
        "ui": { "palette": [{ "id": "count-tabs", "title": "Count tabs" }] }
    }"#;

    #[test]
    fn parses_a_full_manifest() {
        let m = Manifest::parse(HELLO).unwrap();
        assert_eq!(m.name, "hello-plugin");
        assert_eq!(m.permissions, ["tabs.list", "page.*"]);
        assert_eq!(m.events, ["tab.updated"]);
        assert_eq!(m.ui.palette[0].id, "count-tabs");
        assert!(m.userscripts.is_empty());
        assert!(m.warnings().is_empty());
    }

    #[test]
    fn minimal_manifest_and_palette_alias() {
        let m = Manifest::parse(
            r#"{"name":"x","version":"1","main":"a.js","ui":{"palette_commands":[{"id":"a","title":"A"}]}}"#,
        )
        .unwrap();
        assert!(m.permissions.is_empty() && m.events.is_empty());
        assert_eq!(m.ui.palette.len(), 1);
    }

    #[test]
    fn permission_matching() {
        let m = Manifest::parse(HELLO).unwrap();
        assert!(m.allows_command("tabs.list"));
        assert!(m.allows_command("page.snapshot"));
        assert!(!m.allows_command("tabs.close"));
        assert!(!m.allows_command("net.rule_add"));

        let everything =
            Manifest::parse(r#"{"name":"all","version":"1","main":"main.ts","permissions":["*"]}"#)
                .unwrap();
        assert!(everything.allows_command("net.rule_add"));
        // `*` covers script.run, plugins.install, … → warned about.
        assert_eq!(everything.warnings().len(), POWERFUL.len());
    }

    #[test]
    fn rejects_bad_manifests() {
        for (bad, why) in [
            (
                r#"{"name":"Hello","version":"1","main":"m.ts"}"#,
                "uppercase name",
            ),
            (
                r#"{"name":"-x","version":"1","main":"m.ts"}"#,
                "leading dash",
            ),
            (
                r#"{"name":"x","version":"","main":"m.ts"}"#,
                "empty version",
            ),
            (
                r#"{"name":"x","version":"1","main":"../evil.ts"}"#,
                "main outside",
            ),
            (
                r#"{"name":"x","version":"1","main":"C:/evil.ts"}"#,
                "absolute main",
            ),
            (r#"{"name":"x","version":"1","main":"m.py"}"#, "not ts/js"),
            (
                r#"{"name":"x","version":"1","main":"m.ts","permissions":["Page.*"]}"#,
                "bad glob",
            ),
            (
                r#"{"name":"x","version":"1","main":"m.ts","events":[""]}"#,
                "empty glob",
            ),
            (
                r#"{"name":"x","version":"1","main":"m.ts","extra":1}"#,
                "unknown field",
            ),
            (
                r#"{"name":"x","version":"1","main":"m.ts","userscripts":["../u.ts"]}"#,
                "us outside",
            ),
            (
                r#"{"name":"x","version":"1","main":"m.ts","ui":{"palette":[{"id":"","title":"t"}]}}"#,
                "empty id",
            ),
            ("not json", "not json"),
        ] {
            assert!(Manifest::parse(bad).is_err(), "{why} should be rejected");
        }
    }

    #[test]
    fn load_checks_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(MANIFEST_FILE), HELLO).unwrap();
        assert!(Manifest::load(dir.path()).is_err(), "main.ts is missing");
        std::fs::write(dir.path().join("main.ts"), "").unwrap();
        assert_eq!(Manifest::load(dir.path()).unwrap().name, "hello-plugin");
    }
}
