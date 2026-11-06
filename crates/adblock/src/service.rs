//! [`AdblockService`]: the long-lived state behind the `adblock.*` / `simplify.*` commands.
//!
//! It owns the shared [`Gate`] (engine + settings + policies + stats), knows where the files
//! live, and keeps the interim CDP interception sessions per tab. One instance lives in the
//! registry's `Extensions`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bmcp_core::{CmdError, Ctx, DataDir, TabId};
use schemars::JsonSchema;
use serde::Serialize;

use crate::gate::Gate;
use crate::interim::Session;
use crate::lists::{self, FetchStatus, FilterDir, read_json, write_json};
use crate::policy::{AdblockSettings, SimplifyPolicies, SitePolicy, normalize_pattern};

/// Shared adblock/simplify state. See the module docs.
pub struct AdblockService {
    pub gate: Arc<Gate>,
    filters: FilterDir,
    config_dir: PathBuf,
    /// Held while loading the engine or updating lists, so they never run twice at once.
    loading: tokio::sync::Mutex<()>,
    /// Interim CDP `Fetch` interception, per tab (`adblock.apply`).
    pub(crate) sessions: Mutex<HashMap<TabId, Session>>,
}

/// Is a compiled engine loaded, and from which lists?
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EngineStatus {
    pub loaded: bool,
    /// List ids the engine was compiled from (plus `user` for data/filters/user.txt).
    pub lists: Vec<String>,
    /// Size of the serialized engine.
    pub engine_bytes: usize,
}

/// Outcome of checking one list in `adblock.update_lists`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListReport {
    pub id: String,
    pub url: String,
    pub status: FetchStatus,
    /// Size of the list file.
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of `adblock.update_lists`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct UpdateReport {
    pub lists: Vec<ListReport>,
    /// True when the engine was recompiled.
    pub compiled: bool,
    /// Time spent compiling (0 when not recompiled).
    pub compile_ms: u64,
    pub engine: EngineStatus,
}

impl AdblockService {
    /// State stored under `data_root` (`filters/`, `config/adblock.json`,
    /// `config/simplify.json`). Small config files are read right away; the engine is loaded
    /// on first use.
    pub fn new(data_root: &Path) -> Self {
        let config_dir = data_root.join("config");
        let settings: AdblockSettings =
            read_json(&config_dir.join("adblock.json")).unwrap_or_default();
        let policies: SimplifyPolicies =
            read_json(&config_dir.join("simplify.json")).unwrap_or_default();
        AdblockService {
            gate: Arc::new(Gate::new(settings, policies)),
            filters: FilterDir::new(data_root.join("filters")),
            config_dir,
            loading: tokio::sync::Mutex::new(()),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// The shared instance. Created on first use under the `DataDir` found in the registry's
    /// extensions (the app puts it there), else the portable default `data/` next to the exe.
    pub fn from_ctx(ctx: &Ctx) -> Arc<AdblockService> {
        if let Some(service) = ctx.extensions.get::<AdblockService>() {
            return service;
        }
        // Look the data folder up before `get_or_insert_with`, which holds the map's lock.
        let data = ctx
            .extensions
            .get::<DataDir>()
            .map(|d| (*d).clone())
            .or_else(|| DataDir::resolve(None).ok())
            .unwrap_or_else(|| DataDir::from_root("data"));
        ctx.extensions
            .get_or_insert_with(|| AdblockService::new(data.root()))
    }

    pub fn filters(&self) -> &FilterDir {
        &self.filters
    }

    pub fn engine_status(&self) -> EngineStatus {
        let state = self.gate.state();
        EngineStatus {
            loaded: state.engine.is_some(),
            lists: if state.engine.is_some() {
                self.filters.read_meta().engine_lists
            } else {
                Vec::new()
            },
            engine_bytes: state.engine.as_ref().map_or(0, |e| e.bytes.len()),
        }
    }

    /// Make sure an engine is loaded: from `engine.dat`, else by compiling the list files,
    /// else (when `download`) by downloading the enabled lists first. Without lists on disk
    /// and without `download`, nothing is loaded (only site policies apply).
    pub async fn ensure_engine(&self, download: bool) -> Result<EngineStatus, CmdError> {
        let _guard = self.loading.lock().await;
        if self.gate.has_engine() {
            return Ok(self.engine_status());
        }
        let enabled = self.gate.state().settings.lists.clone();
        let filters = self.filters.clone();
        let bytes = blocking(move || {
            let ids = filters.engine_inputs(&enabled);
            if ids.is_empty() {
                return Ok(None);
            }
            match filters.load_cached(&ids) {
                Some(bytes) => Ok(Some(bytes)),
                None => filters.compile(&ids).map(Some),
            }
        })
        .await?;
        match bytes {
            Some(bytes) => self.gate.set_engine(bytes),
            None if download => {
                self.update_locked(false).await?;
            }
            None => {}
        }
        Ok(self.engine_status())
    }

    /// `adblock.update_lists`: check every enabled list for a new version (conditional GET),
    /// then recompile when anything changed. `lists` replaces the enabled set first.
    pub async fn update_lists(
        &self,
        force: bool,
        lists: Option<Vec<String>>,
    ) -> Result<UpdateReport, CmdError> {
        let _guard = self.loading.lock().await;
        if let Some(ids) = lists {
            for id in &ids {
                if lists::source(id).is_none() {
                    let known: Vec<_> = lists::CATALOG.iter().map(|s| s.id).collect();
                    return Err(CmdError::invalid_params(format!(
                        "unknown filter list '{id}'; known lists: {}",
                        known.join(", ")
                    )));
                }
            }
            self.gate.update(|s| s.settings.lists = ids);
            self.save_settings()?;
        }
        self.update_locked(force).await
    }

    /// The body of [`Self::update_lists`]; the caller holds `loading`.
    async fn update_locked(&self, force: bool) -> Result<UpdateReport, CmdError> {
        let enabled = self.gate.state().settings.lists.clone();
        let had_engine = self.gate.has_engine();
        let filters = self.filters.clone();
        let (reports, compiled) = blocking(move || {
            let mut meta = filters.read_meta();
            let mut reports = Vec::new();
            let mut changed = false;
            for id in &enabled {
                let Some(source) = lists::source(id) else {
                    continue;
                };
                let old = meta.lists.get(id).cloned().unwrap_or_default();
                let report = match lists::fetch_list(&filters, source, &old, force) {
                    Ok((status, new_meta)) => {
                        changed |= status == FetchStatus::Updated;
                        let bytes = new_meta.bytes;
                        meta.lists.insert(id.clone(), new_meta);
                        ListReport {
                            id: id.clone(),
                            url: source.url.into(),
                            status,
                            bytes,
                            error: None,
                        }
                    }
                    Err(err) => {
                        tracing::warn!(list = %id, %err, "filter list update failed");
                        ListReport {
                            id: id.clone(),
                            url: source.url.into(),
                            status: FetchStatus::Failed,
                            bytes: old.bytes,
                            error: Some(err),
                        }
                    }
                };
                reports.push(report);
            }
            filters.write_meta(&meta)?;
            let ids = filters.engine_inputs(&enabled);
            // Recompile when a list changed, when the engine was built from other lists, or
            // when nothing is loaded yet.
            let stale = meta.engine_lists != ids || !filters.engine_path().is_file();
            let compiled = if !ids.is_empty() && (changed || stale || !had_engine) {
                let started = Instant::now();
                let bytes = filters.compile(&ids)?;
                Some((bytes, started.elapsed().as_millis() as u64))
            } else {
                None
            };
            Ok((reports, compiled))
        })
        .await?;
        let compile_ms = compiled.as_ref().map_or(0, |(_, ms)| *ms);
        let did_compile = compiled.is_some();
        if let Some((bytes, _)) = compiled {
            self.gate.set_engine(bytes);
        }
        Ok(UpdateReport {
            lists: reports,
            compiled: did_compile,
            compile_ms,
            engine: self.engine_status(),
        })
    }

    /// Compile an engine from rule texts directly (tests, debugging). Nothing is written.
    pub fn load_rules(&self, lists: &[&str]) {
        self.gate
            .set_engine(crate::engine::RequestFilter::compile(lists));
    }

    /// `adblock.toggle`: switch filtering on/off globally (`host: None`) or for one site
    /// (`on: false` adds it to the allowlist).
    pub fn toggle(&self, host: Option<&str>, on: bool) -> Result<AdblockSettings, CmdError> {
        let pattern = host
            .map(normalize_pattern)
            .transpose()
            .map_err(CmdError::invalid_params)?;
        self.gate.update(|s| match &pattern {
            None => s.settings.enabled = on,
            Some(p) if on => {
                s.settings.allowlist.remove(p);
            }
            Some(p) => {
                s.settings.allowlist.insert(p.clone());
            }
        });
        self.save_settings()?;
        Ok(self.gate.state().settings.clone())
    }

    /// `simplify.site`: merge (or with `clear`, replace) the stored policy of a host pattern.
    /// Returns the normalized pattern and its stored policy.
    pub fn set_policy(
        &self,
        host: &str,
        update: SitePolicy,
        clear: bool,
    ) -> Result<(String, SitePolicy), CmdError> {
        let pattern = normalize_pattern(host).map_err(CmdError::invalid_params)?;
        let mut stored = SitePolicy::default();
        self.gate.update(|s| {
            if clear {
                s.policies.sites.remove(&pattern);
            }
            stored = s.policies.merge(&pattern, update);
        });
        self.save_policies()?;
        Ok((pattern, stored))
    }

    fn save_settings(&self) -> Result<(), CmdError> {
        let state = self.gate.state();
        write_json(&self.config_dir.join("adblock.json"), &state.settings)
            .map_err(CmdError::internal)
    }

    fn save_policies(&self) -> Result<(), CmdError> {
        let state = self.gate.state();
        write_json(&self.config_dir.join("simplify.json"), &state.policies)
            .map_err(CmdError::internal)
    }
}

/// Run blocking work (HTTP, disk, compiling) off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, CmdError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| CmdError::internal(format!("adblock worker failed: {e}")))?
        .map_err(CmdError::internal)
}
