//! The script library: one `<name>.ts` file per script in `data/scripts/`.
//!
//! Names are restricted so they are always a safe file name: 1–64 chars of `A-Z a-z 0-9 _ -`,
//! starting with a letter or digit, optionally followed by a version suffix `.v<N>`
//! (`login`, `login.v2`). Healed versions are saved as `<base>.v<N>.ts` next to the original.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use bmcp_core::{CmdError, ErrorCode};
use schemars::JsonSchema;
use serde::Serialize;

/// One script in the library.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ScriptInfo {
    /// Name without `.ts`, e.g. `login` or `login.v2`.
    pub name: String,
    /// Size of the file in bytes.
    pub size: u64,
    /// Last change, milliseconds since the Unix epoch.
    pub modified_ms: u64,
}

/// Access to `data/scripts/`.
#[derive(Debug, Clone)]
pub struct ScriptLib {
    dir: PathBuf,
}

impl ScriptLib {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        ScriptLib { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of a (valid) script name. Does not check that the file exists.
    pub fn path(&self, name: &str) -> Result<PathBuf, CmdError> {
        validate_name(name)?;
        Ok(self.dir.join(format!("{name}.ts")))
    }

    /// All scripts, sorted by name.
    pub fn list(&self) -> Result<Vec<ScriptInfo>, CmdError> {
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(io_error(&self.dir, err)),
        };
        let mut scripts = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".ts"))
            else {
                continue;
            };
            // Skip folders and files we would never have written (other names).
            if !path.is_file() || validate_name(name).is_err() {
                continue;
            }
            scripts.push(info(name, &path)?);
        }
        scripts.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(scripts)
    }

    /// The source code of a script.
    pub fn get(&self, name: &str) -> Result<String, CmdError> {
        let path = self.path(name)?;
        std::fs::read_to_string(&path).map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => not_found(name),
            _ => io_error(&path, err),
        })
    }

    /// Create or overwrite a script.
    pub fn save(&self, name: &str, code: &str) -> Result<ScriptInfo, CmdError> {
        let path = self.path(name)?;
        std::fs::create_dir_all(&self.dir).map_err(|e| io_error(&self.dir, e))?;
        std::fs::write(&path, code).map_err(|e| io_error(&path, e))?;
        info(name, &path)
    }

    /// Delete a script. Returns false when it did not exist.
    pub fn delete(&self, name: &str) -> Result<bool, CmdError> {
        let path = self.path(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(io_error(&path, err)),
        }
    }

    /// Fails with `NOT_FOUND` when the script does not exist.
    pub fn require(&self, name: &str) -> Result<PathBuf, CmdError> {
        let path = self.path(name)?;
        if path.is_file() {
            Ok(path)
        } else {
            Err(not_found(name))
        }
    }

    /// The name for the next healed version: `login` / `login.v2` → `login.v3` when
    /// `login.v2` is the highest existing version.
    pub fn next_version(&self, name: &str) -> Result<String, CmdError> {
        validate_name(name)?;
        let base = base_name(name);
        let mut highest = 1;
        for script in self.list()? {
            if base_name(&script.name) == base
                && let Some(v) = version_of(&script.name)
            {
                highest = highest.max(v);
            }
        }
        Ok(format!("{base}.v{}", highest + 1))
    }
}

/// `login.v3` → `login`; `login` → `login`.
pub fn base_name(name: &str) -> &str {
    match name.rsplit_once(".v") {
        Some((base, digits))
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
        {
            base
        }
        _ => name,
    }
}

/// `login.v3` → `Some(3)`; `login` → `None`.
fn version_of(name: &str) -> Option<u32> {
    let (_, digits) = name.rsplit_once(".v")?;
    digits.parse().ok()
}

/// See the [module docs](self) for the rules.
pub fn validate_name(name: &str) -> Result<(), CmdError> {
    let base = base_name(name);
    let valid_base = !base.is_empty()
        && base.len() <= 64
        && base.starts_with(|c: char| c.is_ascii_alphanumeric())
        && base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    // Windows refuses these as file names (in any case, with any extension).
    const RESERVED: &[&str] = &[
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    let reserved = RESERVED.contains(&base.to_ascii_lowercase().as_str());
    if valid_base && !reserved {
        Ok(())
    } else {
        Err(CmdError::invalid_params(format!(
            "invalid script name {name:?}: use 1-64 chars of A-Z, a-z, 0-9, '_' or '-' \
             (optionally with a version suffix like \".v2\"), without \".ts\""
        )))
    }
}

fn info(name: &str, path: &Path) -> Result<ScriptInfo, CmdError> {
    let meta = std::fs::metadata(path).map_err(|e| io_error(path, e))?;
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);
    Ok(ScriptInfo {
        name: name.to_string(),
        size: meta.len(),
        modified_ms,
    })
}

fn not_found(name: &str) -> CmdError {
    CmdError::new(ErrorCode::NotFound, format!("script '{name}' not found"))
        .with_hint("Call script_list to see the saved scripts, or script_save to create one.")
}

fn io_error(path: &Path, err: std::io::Error) -> CmdError {
    CmdError::internal(format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_safe_file_names() {
        for ok in ["login", "Login_2", "a-b", "x.v2", "login.v10", "9lives"] {
            assert!(validate_name(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "../x", "a/b", "a\\b", "a b", "x.ts", ".hidden", "-x", "con", "NUL", "x.v",
            "x.vv2", "x.v2.v3",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        assert!(validate_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn save_get_list_delete_and_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = ScriptLib::new(tmp.path().join("scripts"));
        assert!(lib.list().unwrap().is_empty());
        lib.save("login", "console.log(1)").unwrap();
        lib.save("other", "x").unwrap();
        std::fs::write(tmp.path().join("scripts/notes.txt"), "ignored").unwrap();
        let names: Vec<_> = lib.list().unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["login", "other"]);
        assert_eq!(lib.get("login").unwrap(), "console.log(1)");
        assert_eq!(lib.get("missing").unwrap_err().code, ErrorCode::NotFound);

        assert_eq!(lib.next_version("login").unwrap(), "login.v2");
        lib.save("login.v2", "v2").unwrap();
        assert_eq!(lib.next_version("login").unwrap(), "login.v3");
        assert_eq!(lib.next_version("login.v2").unwrap(), "login.v3");

        assert!(lib.delete("other").unwrap());
        assert!(!lib.delete("other").unwrap());
        assert_eq!(lib.require("other").unwrap_err().code, ErrorCode::NotFound);
    }
}
