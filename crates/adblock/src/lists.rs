//! Filter lists on disk: download with `ETag`/`Last-Modified` checks, compile, cache.
//!
//! ```text
//! data/filters/
//!   easylist.txt  easyprivacy.txt  ublock.txt   downloaded lists (only the enabled ones)
//!   user.txt                                    your own rules (optional, always used)
//!   lists.json                                  ETag / Last-Modified / sizes per list
//!   engine.dat                                  compiled engine (adblock::Engine::serialize)
//! ```
//!
//! Everything here is blocking (HTTP, file IO, compiling); callers run it in
//! `tokio::task::spawn_blocking`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::RequestFilter;

/// A known filter list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListSource {
    /// Short id, also the file name (`easylist` → `easylist.txt`).
    pub id: &'static str,
    pub title: &'static str,
    pub url: &'static str,
}

/// The lists BrowserMCP knows how to download.
pub const CATALOG: &[ListSource] = &[
    ListSource {
        id: "easylist",
        title: "EasyList (ads)",
        url: "https://easylist.to/easylist/easylist.txt",
    },
    ListSource {
        id: "easyprivacy",
        title: "EasyPrivacy (trackers)",
        url: "https://easylist.to/easylist/easyprivacy.txt",
    },
    ListSource {
        id: "ublock",
        title: "uBlock filters",
        url: "https://ublockorigin.github.io/uAssets/filters/filters.txt",
    },
];

/// Lists used when `adblock.json` does not say otherwise.
pub const DEFAULT_LISTS: [&str; 2] = ["easylist", "easyprivacy"];

/// The user's own rules, compiled in when the file exists.
pub const USER_LIST: &str = "user";

/// Lists are a few MB; refuse anything absurd.
const MAX_LIST_BYTES: u64 = 64 * 1024 * 1024;

pub fn source(id: &str) -> Option<&'static ListSource> {
    CATALOG.iter().find(|s| s.id == id)
}

/// Download bookkeeping per list (`lists.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ListMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// Unix seconds of the last download that changed the file.
    #[serde(default)]
    pub fetched_at: u64,
    /// Unix seconds of the last update check.
    #[serde(default)]
    pub checked_at: u64,
    #[serde(default)]
    pub bytes: u64,
}

/// `lists.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListsFile {
    #[serde(default)]
    pub lists: BTreeMap<String, ListMeta>,
    /// List ids (in order) that `engine.dat` was compiled from.
    #[serde(default)]
    pub engine_lists: Vec<String>,
}

/// Result of checking one list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FetchStatus {
    /// Downloaded a new version.
    Updated,
    /// The server said it has not changed (HTTP 304).
    NotModified,
    /// Download failed; the old file (if any) is kept.
    Failed,
}

/// The folder layout of `data/filters`.
#[derive(Debug, Clone)]
pub struct FilterDir {
    pub root: PathBuf,
}

impl FilterDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        FilterDir { root: root.into() }
    }

    pub fn list_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.txt"))
    }

    pub fn engine_path(&self) -> PathBuf {
        self.root.join("engine.dat")
    }

    fn meta_path(&self) -> PathBuf {
        self.root.join("lists.json")
    }

    pub fn read_meta(&self) -> ListsFile {
        read_json(&self.meta_path()).unwrap_or_default()
    }

    pub fn write_meta(&self, meta: &ListsFile) -> Result<(), String> {
        write_json(&self.meta_path(), meta)
    }

    /// The list ids an engine for `enabled` is built from: the enabled lists that exist on
    /// disk, plus `user` when `user.txt` exists.
    pub fn engine_inputs(&self, enabled: &[String]) -> Vec<String> {
        let mut ids: Vec<String> = enabled
            .iter()
            .filter(|id| self.list_path(id).is_file())
            .cloned()
            .collect();
        if self.list_path(USER_LIST).is_file() {
            ids.push(USER_LIST.into());
        }
        ids
    }

    /// Compile the given lists and store the result as `engine.dat`. Returns the bytes.
    pub fn compile(&self, ids: &[String]) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        let mut texts = Vec::with_capacity(ids.len());
        for id in ids {
            let path = self.list_path(id);
            texts.push(
                std::fs::read_to_string(&path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?,
            );
        }
        let bytes = RequestFilter::compile(&texts);
        write_atomic(&self.engine_path(), &bytes)?;
        let mut meta = self.read_meta();
        meta.engine_lists = ids.to_vec();
        self.write_meta(&meta)?;
        tracing::info!(
            lists = ?ids,
            ms = started.elapsed().as_millis() as u64,
            bytes = bytes.len(),
            "adblock engine compiled"
        );
        Ok(bytes)
    }

    /// The cached engine, if it was built from exactly `ids` and still loads.
    pub fn load_cached(&self, ids: &[String]) -> Option<Vec<u8>> {
        if self.read_meta().engine_lists != ids {
            return None;
        }
        let bytes = std::fs::read(self.engine_path()).ok()?;
        // Reject files from another `adblock` version (or corrupt ones) right here.
        match RequestFilter::from_serialized(&bytes) {
            Ok(_) => Some(bytes),
            Err(err) => {
                tracing::warn!(%err, "ignoring engine.dat; recompiling");
                None
            }
        }
    }
}

/// Download one list unless the server says it has not changed. Returns the new metadata.
pub fn fetch_list(
    dir: &FilterDir,
    source: &ListSource,
    old: &ListMeta,
    force: bool,
) -> Result<(FetchStatus, ListMeta), String> {
    let path = dir.list_path(source.id);
    let have_file = path.is_file();
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .user_agent(concat!(
            "BrowserMCP/",
            env!("CARGO_PKG_VERSION"),
            " (filter list update)"
        ))
        .build();
    let agent: ureq::Agent = config.into();
    let mut request = agent.get(source.url);
    // Conditional request: the server answers 304 without a body when nothing changed.
    if have_file && !force {
        if let Some(etag) = &old.etag {
            request = request.header("If-None-Match", etag);
        }
        if let Some(modified) = &old.last_modified {
            request = request.header("If-Modified-Since", modified);
        }
    }
    let mut response = request.call().map_err(|e| format!("{}: {e}", source.url))?;
    let now = unix_now();
    let status = response.status().as_u16();
    if status == 304 && have_file {
        let meta = ListMeta {
            checked_at: now,
            ..old.clone()
        };
        return Ok((FetchStatus::NotModified, meta));
    }
    if status != 200 {
        return Err(format!("{}: HTTP {status}", source.url));
    }
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let etag = header("etag");
    let last_modified = header("last-modified");
    let text = response
        .body_mut()
        .with_config()
        .limit(MAX_LIST_BYTES)
        .read_to_string()
        .map_err(|e| format!("{}: {e}", source.url))?;
    // A captive portal or error page is HTML, not a filter list.
    if text.trim_start().starts_with('<') || text.lines().count() < 10 {
        return Err(format!("{}: the response is not a filter list", source.url));
    }
    write_atomic(&path, text.as_bytes())?;
    let meta = ListMeta {
        etag,
        last_modified,
        fetched_at: now,
        checked_at: now,
        bytes: text.len() as u64,
    };
    Ok((FetchStatus::Updated, meta))
}

/// Read a JSON file; `None` if missing or invalid (invalid files are logged).
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(value) => Some(value),
        Err(err) => {
            tracing::warn!(path = %path.display(), %err, "ignoring invalid JSON file");
            None
        }
    }
}

/// Write pretty JSON atomically.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes())
}

/// Write to `<path>.tmp`, then rename over `path`, so readers never see half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace {}: {e}", path.display()))
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_caches_and_reloads() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FilterDir::new(tmp.path());
        std::fs::write(dir.list_path("easylist"), "||ads.example^\n").unwrap();
        std::fs::write(dir.list_path(USER_LIST), "||mine.test^\n").unwrap();
        let ids = dir.engine_inputs(&["easylist".into(), "easyprivacy".into()]);
        assert_eq!(ids, ["easylist", "user"]);
        let bytes = dir.compile(&ids).unwrap();
        assert_eq!(dir.load_cached(&ids), Some(bytes));
        // Built from other lists → not reused.
        assert_eq!(dir.load_cached(&["easylist".into()]), None);
        // Corrupt cache → not reused.
        std::fs::write(dir.engine_path(), b"junk").unwrap();
        assert_eq!(dir.load_cached(&ids), None);
    }

    #[test]
    fn catalog_has_the_default_lists() {
        for id in DEFAULT_LISTS {
            assert!(source(id).is_some(), "{id}");
        }
        assert!(source("ublock").is_some());
    }
}
