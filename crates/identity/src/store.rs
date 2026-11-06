//! Small JSON files in `data/config/`: `identities.json` (saved identities and which profile
//! uses which) and `proxies.json` (proxy pools, see [`crate::proxy::pool`]).
//!
//! The files are read on every access and written atomically (temp file + rename), so a user
//! may edit them by hand while the browser is closed, and a crash never leaves half a file.
//! Note: proxy passwords are stored as plain text, like the rest of the portable data folder.

use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use bmcp_core::{CmdError, ErrorCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::identity::Identity;

/// Contents of `identities.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct IdentitiesFile {
    /// Saved identities by id.
    #[serde(default)]
    pub identities: BTreeMap<String, Identity>,
    /// Profile id → identity id: the identity a profile's tabs should get.
    #[serde(default)]
    pub profiles: BTreeMap<String, String>,
}

impl IdentitiesFile {
    pub fn get(&self, id: &str) -> Result<&Identity, CmdError> {
        self.identities.get(id).ok_or_else(|| {
            CmdError::new(ErrorCode::NotFound, format!("no saved identity {id:?}"))
                .with_hint("Call identity_list, or create one with identity_generate save=true.")
                .with_candidates(
                    self.identities
                        .keys()
                        .map(|k| serde_json::Value::String(k.clone()))
                        .collect(),
                )
        })
    }
}

/// One JSON file holding a `T`; a lock so concurrent commands do not lose writes.
pub struct JsonStore<T> {
    path: PathBuf,
    lock: Mutex<()>,
    _type: PhantomData<fn() -> T>,
}

impl<T: Serialize + DeserializeOwned + Default> JsonStore<T> {
    pub fn new(path: PathBuf) -> Self {
        JsonStore {
            path,
            lock: Mutex::new(()),
            _type: PhantomData,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current contents (default when the file does not exist).
    pub fn read(&self) -> Result<T, CmdError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        load(&self.path)
    }

    /// Read, change and write back under the lock. Nothing is written when `change` fails.
    pub fn update<R>(
        &self,
        change: impl FnOnce(&mut T) -> Result<R, CmdError>,
    ) -> Result<R, CmdError> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut value: T = load(&self.path)?;
        let result = change(&mut value)?;
        save(&self.path, &value)?;
        Ok(result)
    }
}

/// Read a JSON file; a missing file is the default value.
fn load<T: DeserializeOwned + Default>(path: &Path) -> Result<T, CmdError> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            CmdError::internal(format!("{} is not valid: {e}", path.display()))
                .with_hint("Fix or delete the file; it is rewritten on the next save.")
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(CmdError::internal(format!(
            "cannot read {}: {e}",
            path.display()
        ))),
    }
}

/// Write a JSON file atomically (pretty-printed for hand editing).
fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), CmdError> {
    let io =
        |e: std::io::Error| CmdError::internal(format!("cannot write {}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| CmdError::internal(format!("cannot serialize: {e}")))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(io)?;
    std::fs::rename(&tmp, path).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{Overrides, generate};

    #[test]
    fn identities_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("identities.json");
        let store = JsonStore::<IdentitiesFile>::new(path.clone());
        assert!(store.read().unwrap().identities.is_empty());
        let id = generate(None, 3, &Overrides::default());
        let name = id.id.clone();
        store
            .update(|f| {
                f.identities.insert(name.clone(), id.clone());
                f.profiles.insert("work".into(), name.clone());
                Ok(())
            })
            .unwrap();
        let again = JsonStore::<IdentitiesFile>::new(path).read().unwrap();
        assert_eq!(again.get(&name).unwrap(), &id);
        assert_eq!(again.profiles["work"], name);
        let err = again.get("nope").unwrap_err();
        assert_eq!(err.candidates.len(), 1);
    }

    #[test]
    fn failed_update_writes_nothing_and_broken_files_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identities.json");
        let store = JsonStore::<IdentitiesFile>::new(path.clone());
        let failed: Result<(), _> = store.update(|_| Err(CmdError::internal("no")));
        assert!(failed.is_err());
        assert!(!path.exists());
        std::fs::write(&path, "{oops").unwrap();
        let err = store.read().unwrap_err();
        assert!(err.message.contains("not valid"), "{err:?}");
    }
}
