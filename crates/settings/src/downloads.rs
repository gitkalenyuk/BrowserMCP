//! Download settings (`data/config/downloads.json`, `downloads.config`).
//!
//! * the folder downloads go to (default `data/downloads`, next to the exe like everything
//!   else; tabs of a mass run always use `data/runs/<run>/downloads`);
//! * "ask where to save each file": the browser shows its Save as dialog instead of saving
//!   straight into the folder (never for mass-run tabs, nobody would answer).

use std::path::{Path, PathBuf};

use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{BrowserSettings, SETTINGS_CHANGED};

/// Contents of `downloads.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DownloadsConfig {
    /// Absolute folder for downloads; `null` = `data/downloads`.
    #[serde(default)]
    pub dir: Option<String>,
    /// Show the Save as dialog for every download.
    #[serde(default)]
    pub ask: bool,
}

/// The default download folder.
pub fn default_dir(data_root: &Path) -> PathBuf {
    data_root.join("downloads")
}

/// Where a download goes (pure): a mass run's own folder, else the chosen folder, else
/// `data/downloads`.
pub fn download_dir(config: &DownloadsConfig, data_root: &Path, run: Option<&str>) -> PathBuf {
    if run.is_some() {
        return bmcp_profiles::downloads::download_dir(data_root, run);
    }
    match &config.dir {
        Some(dir) => PathBuf::from(dir),
        None => default_dir(data_root),
    }
}

/// Check a folder typed in Settings: absolute, on a drive (no `\\server\share`: even looking
/// at a network path makes an SMB connection), created when missing.
pub fn check_dir(text: &str) -> Result<PathBuf, CmdError> {
    let path = PathBuf::from(text.trim());
    if !path.is_absolute() {
        return Err(CmdError::invalid_params(format!(
            "the download folder must be a full path like C:\\Users\\you\\Downloads, got {text:?}"
        )));
    }
    let unc = text.trim().starts_with(r"\\") || text.trim().starts_with("//");
    if unc {
        return Err(CmdError::invalid_params(
            "network folders (\\\\server\\share) cannot be the download folder",
        ));
    }
    std::fs::create_dir_all(&path).map_err(|e| {
        CmdError::invalid_params(format!(
            "cannot use {} as the download folder: {e}",
            path.display()
        ))
    })?;
    if !path.is_dir() {
        return Err(CmdError::invalid_params(format!(
            "{} is not a folder",
            path.display()
        )));
    }
    Ok(path)
}

pub struct Config;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigArgs {
    /// New download folder (absolute; created if missing); "" = back to data/downloads.
    pub dir: Option<String>,
    /// true: show the Save as dialog for every download.
    pub ask: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ConfigOutput {
    /// The folder downloads go to now.
    pub dir: String,
    /// The default folder (data/downloads).
    pub default_dir: String,
    /// true when `dir` was chosen in Settings.
    pub custom: bool,
    /// The Save as dialog is shown for every download.
    pub ask: bool,
}

impl Command for Config {
    const NAME: &'static str = "downloads.config";
    type Args = ConfigArgs;
    type Output = ConfigOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read or change where downloads go (data/config/downloads.json): dir (absolute \
             folder, \"\" = data/downloads), ask (show the Save as dialog for each file). No \
             arguments = read. Tabs of mass runs always save into data/runs/<run>/downloads.",
            Toolset::Data,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: ConfigArgs) -> Result<ConfigOutput, CmdError> {
        let settings = BrowserSettings::from_ctx(&ctx)?;
        let config = if args.dir.is_some() || args.ask.is_some() {
            let (config, ()) = settings.files().downloads.update(|c| {
                if let Some(dir) = &args.dir {
                    c.dir = if dir.trim().is_empty() {
                        None
                    } else {
                        Some(check_dir(dir)?.display().to_string())
                    };
                }
                if let Some(ask) = args.ask {
                    c.ask = ask;
                }
                Ok(())
            })?;
            ctx.events
                .emit(SETTINGS_CHANGED, serde_json::json!({ "area": "downloads" }));
            config
        } else {
            settings.downloads()
        };
        let dir = download_dir(&config, settings.data_root(), None);
        // The browser creates it with the first download; create it now so "Open folder"
        // works before that (best effort: a failure shows up when downloading).
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::debug!(dir = %dir.display(), %err, "download folder not created");
        }
        Ok(ConfigOutput {
            dir: dir.display().to_string(),
            default_dir: default_dir(settings.data_root()).display().to_string(),
            custom: config.dir.is_some(),
            ask: config.ask,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_keep_their_folder_and_the_choice_wins_otherwise() {
        let root = Path::new("/data");
        let mut c = DownloadsConfig::default();
        assert_eq!(download_dir(&c, root, None), Path::new("/data/downloads"));
        c.dir = Some("/elsewhere".into());
        assert_eq!(download_dir(&c, root, None), Path::new("/elsewhere"));
        assert_eq!(
            download_dir(&c, root, Some("r1")),
            Path::new("/data/runs/r1/downloads")
        );
    }

    #[test]
    fn folder_must_be_absolute_and_local() {
        let tmp = tempfile::tempdir().unwrap();
        let wanted = tmp.path().join("new").join("dl");
        assert_eq!(check_dir(&wanted.display().to_string()).unwrap(), wanted);
        assert!(wanted.is_dir(), "created");
        assert!(check_dir("relative\\dir").is_err());
        assert!(check_dir(r"\\server\share\dl").is_err());
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(check_dir(&file.display().to_string()).is_err());
    }
}
