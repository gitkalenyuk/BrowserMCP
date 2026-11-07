//! Small JSON settings files in `data/config/` (`connections.json`, `recording.json`,
//! `scripts.json`, …).
//!
//! Read on every access, so a user may edit a file by hand while the browser runs; written
//! atomically (temp file + rename), so a crash never leaves half a file. A missing file means
//! "all defaults"; a broken file is an error (never silently replaced by defaults, which would
//! throw away what the user wrote).

use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::CmdError;

/// The contents of `path`, or `T::default()` when the file does not exist.
pub fn load<T: DeserializeOwned + Default>(path: &Path) -> Result<T, CmdError> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(T::default()),
        Ok(text) => serde_json::from_str(&text).map_err(|err| {
            CmdError::internal(format!("{} is not valid: {err}", path.display()))
                .with_hint("Fix or delete the file; deleting it restores the defaults.")
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(err) => Err(CmdError::internal(format!(
            "cannot read {}: {err}",
            path.display()
        ))),
    }
}

/// Write `value` to `path` as pretty JSON (creating the folder), atomically.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), CmdError> {
    let fail = |what: &str, err: &dyn std::fmt::Display| {
        CmdError::internal(format!("cannot {what} {}: {err}", path.display()))
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail("create the folder of", &e))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| fail("serialize", &e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| fail("write", &e))?;
    std::fs::rename(&tmp, path).map_err(|e| fail("replace", &e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Sample {
        #[serde(default)]
        n: u32,
    }

    #[test]
    fn missing_or_empty_file_is_default_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("sample.json");
        assert_eq!(load::<Sample>(&path).unwrap(), Sample::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "  ").unwrap();
        assert_eq!(load::<Sample>(&path).unwrap(), Sample::default());
        save(&path, &Sample { n: 7 }).unwrap();
        assert_eq!(load::<Sample>(&path).unwrap(), Sample { n: 7 });
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn broken_file_is_an_error_not_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err = load::<Sample>(&path).unwrap_err();
        assert!(err.message.contains("sample.json"), "{}", err.message);
    }
}
