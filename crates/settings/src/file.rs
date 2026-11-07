//! One settings file in `data/config/`, kept in memory.
//!
//! The browser reads these settings on hot paths (every download, every new tab), so the value
//! lives in memory and the file is only read at startup. Changes go through
//! [`ConfigFile::update`]: validate a copy, write it atomically, then publish it. A file that
//! does not parse is logged and replaced by the defaults in memory (it is overwritten on the
//! next save, like the other `data/config` files).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use bmcp_core::CmdError;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub struct ConfigFile<T> {
    path: PathBuf,
    value: RwLock<T>,
    /// Serializes writers, so two saves never interleave their read-modify-write.
    write: Mutex<()>,
}

impl<T: Serialize + DeserializeOwned + Default + Clone> ConfigFile<T> {
    /// Load `path` (missing or broken file: the defaults).
    pub fn load(path: PathBuf) -> Self {
        let value = read(&path);
        ConfigFile {
            path,
            value: RwLock::new(value),
            write: Mutex::new(()),
        }
    }

    /// The value in force.
    pub fn get(&self) -> T {
        self.value.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Change a copy with `change`, write it and make it the value in force. Nothing changes
    /// when `change` or the write fails.
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut T) -> Result<R, CmdError>,
    ) -> Result<(T, R), CmdError> {
        let _writer = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = self.get();
        let result = change(&mut next)?;
        write_atomic(&self.path, &next)?;
        *self.value.write().unwrap_or_else(|e| e.into_inner()) = next.clone();
        Ok((next, result))
    }
}

fn read<T: DeserializeOwned + Default>(path: &Path) -> T {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
            tracing::warn!(path = %path.display(), %err, "settings file unreadable; using the defaults");
            T::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => T::default(),
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "settings file not read; using the defaults");
            T::default()
        }
    }
}

/// Write pretty JSON via a temporary file + rename, so a crash never leaves half a file.
fn write_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), CmdError> {
    let io =
        |e: std::io::Error| CmdError::internal(format!("cannot write {}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| CmdError::internal(format!("cannot serialize {}: {e}", path.display())))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    struct Demo {
        #[serde(default)]
        n: u32,
    }

    #[test]
    fn missing_or_broken_file_is_the_default_and_updates_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config").join("demo.json");
        let file = ConfigFile::<Demo>::load(path.clone());
        assert_eq!(file.get(), Demo::default());

        file.update(|d| {
            d.n = 7;
            Ok(())
        })
        .unwrap();
        assert_eq!(ConfigFile::<Demo>::load(path.clone()).get().n, 7);

        // A failing change leaves memory and disk alone.
        let err = file.update(|d| {
            d.n = 9;
            Err::<(), _>(CmdError::invalid_params("no"))
        });
        assert!(err.is_err());
        assert_eq!(file.get().n, 7);
        assert_eq!(ConfigFile::<Demo>::load(path.clone()).get().n, 7);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(ConfigFile::<Demo>::load(path).get(), Demo::default());
    }
}
