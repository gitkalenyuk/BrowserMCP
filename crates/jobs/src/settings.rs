//! `script.settings`: defaults of script runs and mass runs (`data/config/scripts.json`) and
//! whether the script host was found (Settings → Recording & scripts).
//!
//! The defaults apply when `script.run` / `run.start` (and cron triggers) leave the values
//! out; explicit arguments still win, and `run.start`'s parallelism is still capped by CPU
//! count and free RAM.

use std::path::Path;

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Inner;
use crate::host;
use crate::runner::{DEFAULT_TIMEOUT_SECS, RAM_PER_SLOT};

/// File name of the script settings in `data/config/`.
pub const SCRIPTS_FILE: &str = "scripts.json";
/// Longest timeout the settings accept (one day).
const MAX_TIMEOUT_SECS: u64 = 86_400;
/// Most retries the settings accept.
const MAX_RETRIES: u32 = 10;

/// `data/config/scripts.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ScriptSettings {
    /// Seconds before `script.run` (and a cron trigger's script) is stopped (default 300).
    #[serde(default = "default_timeout")]
    pub script_timeout_secs: u64,
    /// Seconds per row of a mass run before the row is stopped (default 300).
    #[serde(default = "default_timeout")]
    pub run_timeout_secs: u64,
    /// Rows of a mass run at once (default 4; still capped by CPUs and free RAM).
    #[serde(default = "default_parallel")]
    pub run_parallel: usize,
    /// Extra attempts for a failed row (default 1).
    #[serde(default = "default_retries")]
    pub run_retries: u32,
}

fn default_timeout() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_parallel() -> usize {
    4
}

fn default_retries() -> u32 {
    1
}

impl Default for ScriptSettings {
    fn default() -> Self {
        ScriptSettings {
            script_timeout_secs: default_timeout(),
            run_timeout_secs: default_timeout(),
            run_parallel: default_parallel(),
            run_retries: default_retries(),
        }
    }
}

impl ScriptSettings {
    /// The stored settings; defaults when there is no file (or no path: tests).
    pub fn load(path: Option<&Path>) -> Result<Self, CmdError> {
        match path {
            Some(path) => bmcp_core::config_file::load(path),
            None => Ok(Self::default()),
        }
    }

    fn check(&self, max_parallel: usize) -> Result<(), CmdError> {
        for (name, secs) in [
            ("script_timeout_secs", self.script_timeout_secs),
            ("run_timeout_secs", self.run_timeout_secs),
        ] {
            if !(1..=MAX_TIMEOUT_SECS).contains(&secs) {
                return Err(CmdError::invalid_params(format!(
                    "{name} must be 1-{MAX_TIMEOUT_SECS} seconds, got {secs}"
                )));
            }
        }
        if !(1..=max_parallel).contains(&self.run_parallel) {
            return Err(CmdError::invalid_params(format!(
                "run_parallel must be 1-{max_parallel}, got {}",
                self.run_parallel
            )));
        }
        if self.run_retries > MAX_RETRIES {
            return Err(CmdError::invalid_params(format!(
                "run_retries must be 0-{MAX_RETRIES}, got {}",
                self.run_retries
            )));
        }
        Ok(())
    }
}

impl Inner {
    /// The settings in force. A broken file must not stop runs: it is logged and the
    /// defaults apply (`script.settings` reports the error).
    pub(crate) fn settings(&self) -> ScriptSettings {
        ScriptSettings::load(self.config.settings_path.as_deref()).unwrap_or_else(|err| {
            tracing::warn!(%err, "scripts.json ignored");
            ScriptSettings::default()
        })
    }
}

// ---------------------------------------------------------------------------------------------
// script.settings
// ---------------------------------------------------------------------------------------------

/// `script.settings`.
pub struct Settings;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsArgs {
    /// Back to the defaults first (the other fields then apply on top).
    pub reset: Option<bool>,
    /// Default timeout of script_run, seconds (1-86400).
    pub script_timeout_secs: Option<u64>,
    /// Default per-row timeout of run_start, seconds (1-86400).
    pub run_timeout_secs: Option<u64>,
    /// Default rows at once of run_start (1 to the hard limit).
    pub run_parallel: Option<usize>,
    /// Default extra attempts of a failed row (0-10).
    pub run_retries: Option<u32>,
}

impl SettingsArgs {
    fn changes(&self) -> bool {
        self.reset.unwrap_or(false)
            || self.script_timeout_secs.is_some()
            || self.run_timeout_secs.is_some()
            || self.run_parallel.is_some()
            || self.run_retries.is_some()
    }

    fn apply(&self, mut s: ScriptSettings) -> ScriptSettings {
        if self.reset.unwrap_or(false) {
            s = ScriptSettings::default();
        }
        s.script_timeout_secs = self.script_timeout_secs.unwrap_or(s.script_timeout_secs);
        s.run_timeout_secs = self.run_timeout_secs.unwrap_or(s.run_timeout_secs);
        s.run_parallel = self.run_parallel.unwrap_or(s.run_parallel);
        s.run_retries = self.run_retries.unwrap_or(s.run_retries);
        s
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct HostStatus {
    /// bmcp-script-host(.exe) was found: scripts and mass runs can start.
    pub found: bool,
    pub path: Option<String>,
    /// Why it was not found.
    pub error: Option<String>,
    /// How to fix it.
    pub hint: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SettingsOutput {
    /// The settings in force (saved in data/config/scripts.json).
    pub settings: ScriptSettings,
    /// The built-in defaults.
    pub defaults: ScriptSettings,
    pub host: HostStatus,
    /// `data/scripts` (saved scripts).
    pub scripts_dir: String,
    /// `data/runs` (exports of mass runs).
    pub runs_dir: String,
    /// Hard upper limit of rows at once.
    pub max_parallel: usize,
    /// What this machine can run at once right now (CPUs / 2, free RAM / 400 MB).
    pub recommended_parallel: usize,
}

fn host_status(explicit: Option<&Path>) -> HostStatus {
    match host::locate_host(explicit) {
        Ok(path) => HostStatus {
            found: true,
            path: Some(path.display().to_string()),
            error: None,
            hint: None,
        },
        Err(err) => HostStatus {
            found: false,
            path: None,
            error: Some(err.message),
            hint: err.hint,
        },
    }
}

impl Command for Settings {
    const NAME: &'static str = "script.settings";
    type Args = SettingsArgs;
    type Output = SettingsOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read or change the defaults of script_run (timeout) and run_start (timeout per \
             row, rows at once, retries), saved in data/config/scripts.json and used when a \
             call leaves them out. Also reports whether bmcp-script-host was found, the \
             folders, and how many rows this machine can run at once. No arguments = just \
             read.",
            Toolset::Script,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SettingsArgs) -> Result<SettingsOutput, CmdError> {
        let inner = crate::commands::service(&ctx)?;
        let config = &inner.config;
        let path = config.settings_path.as_deref();
        let mut settings = ScriptSettings::load(path)?;
        if args.changes() {
            let path = path.ok_or_else(|| {
                CmdError::internal("the jobs service has no settings file (settings_path)")
            })?;
            settings = args.apply(settings);
            settings.check(config.max_parallel)?;
            bmcp_core::config_file::save(path, &settings)?;
        }
        Ok(SettingsOutput {
            settings,
            defaults: ScriptSettings::default(),
            host: host_status(config.host_exe.as_deref()),
            scripts_dir: config.scripts_dir.display().to_string(),
            runs_dir: config.runs_dir.display().to_string(),
            max_parallel: config.max_parallel,
            recommended_parallel: bmcp_governor::recommend_parallel(RAM_PER_SLOT)
                .parallel
                .clamp(1, config.max_parallel),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_patch_and_reset() {
        let stored = ScriptSettings {
            script_timeout_secs: 60,
            run_timeout_secs: 120,
            run_parallel: 8,
            run_retries: 3,
        };
        let patch = SettingsArgs {
            run_parallel: Some(2),
            ..Default::default()
        };
        assert_eq!(
            patch.apply(stored),
            ScriptSettings {
                run_parallel: 2,
                ..stored
            }
        );
        let reset = SettingsArgs {
            reset: Some(true),
            run_retries: Some(0),
            ..Default::default()
        };
        assert_eq!(
            reset.apply(stored),
            ScriptSettings {
                run_retries: 0,
                ..ScriptSettings::default()
            }
        );
        assert!(!SettingsArgs::default().changes());
    }

    #[test]
    fn check_rejects_out_of_range_values() {
        let ok = ScriptSettings::default();
        assert!(ok.check(32).is_ok());
        for bad in [
            ScriptSettings {
                script_timeout_secs: 0,
                ..ok
            },
            ScriptSettings {
                run_timeout_secs: MAX_TIMEOUT_SECS + 1,
                ..ok
            },
            ScriptSettings {
                run_parallel: 0,
                ..ok
            },
            ScriptSettings {
                run_parallel: 33,
                ..ok
            },
            ScriptSettings {
                run_retries: 11,
                ..ok
            },
        ] {
            assert!(bad.check(32).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn missing_path_or_partial_file_gives_defaults() {
        assert_eq!(
            ScriptSettings::load(None).unwrap(),
            ScriptSettings::default()
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SCRIPTS_FILE);
        std::fs::write(&path, r#"{"run_parallel": 2}"#).unwrap();
        let s = ScriptSettings::load(Some(&path)).unwrap();
        assert_eq!(s.run_parallel, 2);
        assert_eq!(s.run_timeout_secs, DEFAULT_TIMEOUT_SECS);
    }
}
