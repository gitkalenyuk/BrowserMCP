//! Portable data layout.
//!
//! Everything BrowserMCP stores lives in one `data/` folder next to the executable
//! (on macOS: next to the `.app` bundle), so the whole browser can be copied around.
//!
//! ```text
//! data/
//!   config/        settings.json, keymap.json
//!   profiles/<id>/ CEF cache for one profile (all under CEF's root_cache_path)
//!   db/            browsermcp.sqlite
//!   recordings/ videos/ har/ scripts/ plugins/ runs/ logs/
//!   runtime/       endpoint.json (pid + ports of the running instance)
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("cannot determine executable location: {0}")]
    CurrentExe(#[source] std::io::Error),
    #[error("cannot create directory {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid profile id {0:?}: use 1-64 chars of a-z, 0-9, '-' or '_'")]
    InvalidProfileId(String),
}

/// Name of a browser profile. Restricted charset so it is always a safe folder name
/// (no path traversal, no case-collisions between Windows and macOS).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileId(String);

impl ProfileId {
    pub const DEFAULT: &'static str = "default";

    pub fn new(id: impl Into<String>) -> Result<Self, PathError> {
        let id = id.into();
        let valid = !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if valid {
            Ok(Self(id))
        } else {
            Err(PathError::InvalidProfileId(id))
        }
    }

    pub fn default_profile() -> Self {
        Self(Self::DEFAULT.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Root of the portable data folder and typed accessors for its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    root: PathBuf,
}

impl DataDir {
    /// Sub-folders created by [`DataDir::ensure`].
    const SUBDIRS: &'static [&'static str] = &[
        "config",
        "profiles",
        "db",
        "recordings",
        "videos",
        "har",
        "scripts",
        "plugins",
        "runs",
        "logs",
        "runtime",
    ];

    /// Use an explicit root (e.g. from `--data-dir`), otherwise the portable default.
    pub fn resolve(explicit: Option<&Path>) -> Result<Self, PathError> {
        match explicit {
            Some(root) => Ok(Self::from_root(root)),
            None => {
                let exe = std::env::current_exe().map_err(PathError::CurrentExe)?;
                Ok(Self::from_root(portable_root_for_exe(&exe)))
            }
        }
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Create the folder layout if it does not exist yet.
    pub fn ensure(&self) -> Result<(), PathError> {
        for dir in std::iter::once(self.root.clone())
            .chain(Self::SUBDIRS.iter().map(|sub| self.root.join(sub)))
        {
            std::fs::create_dir_all(&dir).map_err(|source| PathError::Create {
                path: dir.clone(),
                source,
            })?;
        }
        Ok(())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    /// CEF `root_cache_path`: every profile cache must live below it.
    pub fn profiles(&self) -> PathBuf {
        self.root.join("profiles")
    }

    pub fn profile(&self, id: &ProfileId) -> PathBuf {
        self.profiles().join(id.as_str())
    }

    pub fn db(&self) -> PathBuf {
        self.root.join("db")
    }

    pub fn logs(&self) -> PathBuf {
        self.root.join("logs")
    }

    pub fn runtime(&self) -> PathBuf {
        self.root.join("runtime")
    }
}

/// `<exe dir>/data`, or `<folder containing X.app>/data` when running from a macOS bundle
/// (the bundle itself is read-only once signed).
pub fn portable_root_for_exe(exe: &Path) -> PathBuf {
    let exe_dir = exe.parent().unwrap_or(Path::new("."));
    let base = app_bundle_parent(exe_dir).unwrap_or(exe_dir);
    base.join("data")
}

/// For `.../Foo.app/Contents/MacOS` return the folder that contains `Foo.app`.
fn app_bundle_parent(exe_dir: &Path) -> Option<&Path> {
    let contents = exe_dir.parent()?;
    let bundle = contents.parent()?;
    let is_bundle = exe_dir.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app";
    if is_bundle { bundle.parent() } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_sits_next_to_plain_exe() {
        let exe = Path::new("/opt/browsermcp/browsermcp.exe");
        assert_eq!(
            portable_root_for_exe(exe),
            Path::new("/opt/browsermcp/data")
        );
    }

    #[test]
    fn data_dir_sits_next_to_macos_bundle() {
        let exe = Path::new("/Apps/BrowserMCP.app/Contents/MacOS/browsermcp");
        assert_eq!(portable_root_for_exe(exe), Path::new("/Apps/data"));
    }

    #[test]
    fn folder_named_macos_outside_bundle_is_not_a_bundle() {
        let exe = Path::new("/home/me/Contents/MacOS/browsermcp");
        assert_eq!(
            portable_root_for_exe(exe),
            Path::new("/home/me/Contents/MacOS/data")
        );
    }

    #[test]
    fn ensure_creates_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let data = DataDir::from_root(tmp.path().join("data"));
        data.ensure().unwrap();
        for sub in DataDir::SUBDIRS {
            assert!(data.root().join(sub).is_dir(), "{sub} missing");
        }
        // Idempotent.
        data.ensure().unwrap();
    }

    #[test]
    fn profile_ids_are_safe_folder_names() {
        for ok in ["default", "work", "profile-17", "a_b"] {
            assert!(ProfileId::new(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "..",
            "../x",
            "Work",
            "a/b",
            "a\\b",
            "a b",
            &"x".repeat(65),
        ] {
            assert!(ProfileId::new(bad).is_err(), "{bad:?}");
        }
        let data = DataDir::from_root("/d");
        let id = ProfileId::new("work").unwrap();
        assert_eq!(data.profile(&id), Path::new("/d/profiles/work"));
    }
}
