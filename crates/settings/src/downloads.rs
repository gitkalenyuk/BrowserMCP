//! Download settings (`data/config/downloads.json`, `downloads.config`).
//!
//! * the folder downloads go to (default `data/downloads`, next to the exe like everything
//!   else; tabs of a mass run always use `data/runs/<run>/downloads`);
//! * "ask where to save each file": the browser shows its Save as dialog instead of saving
//!   straight into the folder (never for mass-run tabs, nobody would answer).
//!
//! The folder can never be a system folder ([`protected_reason`]): downloads are saved there
//! without asking, and the folder is also one `system.open_path` / `reveal_path` may show, so
//! `C:\` or the Startup folder (`%APPDATA%\…\Startup`, which runs what lands in it) would
//! turn a download into something much worse.

use std::path::{Component, Path, PathBuf};

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

/// Where a download goes: a mass run's own folder, else the chosen folder, else
/// `data/downloads`. A chosen folder that is a system folder (a hand-edited
/// `downloads.json`; `downloads.config` refuses them) is ignored.
pub fn download_dir(config: &DownloadsConfig, data_root: &Path, run: Option<&str>) -> PathBuf {
    if run.is_some() {
        return bmcp_profiles::downloads::download_dir(data_root, run);
    }
    match &config.dir {
        Some(dir) if protected_reason(Path::new(dir), &ProtectedDirs::from_env()).is_none() => {
            PathBuf::from(dir)
        }
        Some(dir) => {
            tracing::warn!(
                dir,
                "download folder is a system folder; using data/downloads"
            );
            default_dir(data_root)
        }
        None => default_dir(data_root),
    }
}

/// System folders a download folder must not be (or be inside).
#[derive(Debug, Clone, Default)]
pub struct ProtectedDirs {
    /// Neither these folders nor anything inside them.
    pub inside: Vec<PathBuf>,
    /// These folders themselves (their subfolders, e.g. `%USERPROFILE%\Downloads`, are fine).
    pub exact: Vec<PathBuf>,
}

impl ProtectedDirs {
    /// The Windows folders from the environment: Windows, Program Files, ProgramData (the
    /// all-users Startup folder), Roaming AppData (the user's Startup folder),
    /// `%LOCALAPPDATA%\Microsoft` (`WindowsApps` is on PATH); the profile folder and Local
    /// AppData themselves (Temp under it stays allowed).
    pub fn from_env() -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let inside = [
            "SystemRoot",
            "windir",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "ProgramData",
            "APPDATA",
        ]
        .into_iter()
        .filter_map(var)
        .map(PathBuf::from)
        .chain(var("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Microsoft")))
        .collect();
        let exact = ["USERPROFILE", "LOCALAPPDATA"]
            .into_iter()
            .filter_map(var)
            .map(PathBuf::from)
            .collect();
        ProtectedDirs { inside, exact }
    }
}

/// Why `path` cannot be the download folder, or `None` when it can. Decided from the text
/// (case-insensitive, as Windows paths are), so call it on a resolved path too.
pub fn protected_reason(path: &Path, dirs: &ProtectedDirs) -> Option<String> {
    // `C:\` (and `C:\x\..`, which the caller refuses anyway) has no last component.
    if path.file_name().is_none() {
        return Some(format!("{} is the root of a drive", path.display()));
    }
    let key = path_key(path);
    let hit = |root: &PathBuf, exact: bool| {
        let root = path_key(root);
        !root.is_empty() && (key == root || !exact && key.starts_with(&format!(r"{root}\")))
    };
    dirs.inside
        .iter()
        .find(|root| hit(root, false))
        .or_else(|| dirs.exact.iter().find(|root| hit(root, true)))
        .map(|root| format!("{} is a system folder ({})", path.display(), root.display()))
}

/// `path` for comparisons: no `\\?\` prefix, backslashes, no trailing separator, lowercase.
fn path_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', r"\");
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.trim_end_matches('\\').to_lowercase()
}

/// Check a folder typed in Settings: absolute, on a drive (no `\\server\share`: even looking
/// at a network path makes an SMB connection), not a system folder, created when missing.
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
    // `..` would let a path look like it is somewhere else to the text check below.
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(CmdError::invalid_params(format!(
            "the download folder must not contain `.` or `..`, got {text:?}"
        )));
    }
    let protected = ProtectedDirs::from_env();
    refuse_protected(&path, &protected)?;
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
    // A junction or link can point into a system folder: check where it really is too.
    if let Ok(real) = path.canonicalize() {
        refuse_protected(&real, &protected)?;
    }
    Ok(path)
}

fn refuse_protected(path: &Path, dirs: &ProtectedDirs) -> Result<(), CmdError> {
    match protected_reason(path, dirs) {
        Some(why) => Err(CmdError::invalid_params(format!(
            "{why}: pick a folder of your own, e.g. C:\\Users\\you\\Downloads"
        ))),
        None => Ok(()),
    }
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
            // Not just `dir.is_some()`: a refused (system) folder falls back to the default.
            custom: dir != default_dir(settings.data_root()),
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
    fn system_folders_are_refused() {
        let dirs = ProtectedDirs {
            inside: vec![
                PathBuf::from(r"C:\Windows"),
                PathBuf::from(r"C:\Users\me\AppData\Roaming"),
            ],
            exact: vec![PathBuf::from(r"C:\Users\me")],
        };
        let refused = |p: &str| protected_reason(Path::new(p), &dirs).is_some();
        // Drive roots, the folders and everything inside them, in any case and either slash.
        assert!(refused(r"C:\"));
        assert!(refused(r"c:\windows"));
        assert!(refused(r"C:\Windows\System32\"));
        assert!(refused(
            r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"
        ));
        assert!(refused("C:/users/ME/appdata/roaming/x"));
        assert!(refused(r"\\?\C:\Windows\Temp"));
        // The profile folder itself, but not its subfolders; look-alike names are fine.
        assert!(refused(r"C:\Users\me\"));
        assert!(!refused(r"C:\Users\me\Downloads"));
        assert!(!refused(r"C:\Windows2\dl"));
        assert!(!refused(r"D:\Downloads"));
    }

    #[test]
    fn a_hand_edited_system_folder_is_ignored() {
        let Some(windir) = std::env::var_os("SystemRoot") else {
            return;
        };
        let c = DownloadsConfig {
            dir: Some(PathBuf::from(windir).display().to_string()),
            ask: false,
        };
        assert_eq!(
            download_dir(&c, Path::new("/data"), None),
            Path::new("/data/downloads")
        );
        assert!(check_dir(c.dir.as_deref().unwrap()).is_err());
    }

    #[test]
    fn folder_must_be_absolute_and_local() {
        let tmp = tempfile::tempdir().unwrap();
        let wanted = tmp.path().join("new").join("dl");
        assert_eq!(check_dir(&wanted.display().to_string()).unwrap(), wanted);
        assert!(wanted.is_dir(), "created");
        assert!(check_dir("relative\\dir").is_err());
        assert!(check_dir(r"\\server\share\dl").is_err());
        let sneaky = format!(r"{}\..\dl", tmp.path().display());
        assert!(check_dir(&sneaky).is_err(), "`..` refused");
        let file = tmp.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(check_dir(&file.display().to_string()).is_err());
    }
}
