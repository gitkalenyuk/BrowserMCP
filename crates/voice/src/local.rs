//! Local Whisper: the whisper.cpp Windows build and its models, downloaded on demand into
//! `data/voice`, and the `whisper-server` sidecar that runs on a free localhost port while
//! dictation is used.
//!
//! * **Engine**: the newest whisper.cpp release on GitHub (`ggml-org/whisper.cpp`) that has
//!   the CPU build `whisper-bin-x64.zip` (the `v1.x` tags carry no files; the builds are on the
//!   `b<NNNN>` releases). Checked against the size and SHA-256 GitHub publishes for the asset,
//!   unpacked into `data/voice/bin` (`whisper-server.exe`, `whisper-cli.exe`, DLLs).
//! * **Models**: `data/voice/models/ggml-<id>.bin` from Hugging Face, checked against the size
//!   and SHA-256 in [`crate::catalog`].
//! * **Server**: started lazily by the first transcription, stopped after
//!   `local.idle_minutes` without use, when the model changes, before the engine is replaced,
//!   and when the browser quits (a Job Object kills it even if the browser crashes).
//!
//! Nothing is downloaded unless `voice.local_download` is called (the Download buttons in
//! Settings → Voice).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bmcp_core::{CmdError, ErrorCode};
use futures_util::StreamExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::catalog::{self, MODELS, ModelInfo, PREFERRED};
use crate::settings::LocalSettings;

/// The release asset with the CPU build for Windows x64.
pub const ENGINE_ASSET: &str = "whisper-bin-x64.zip";
/// The id `voice.local_download {what}` uses for the engine.
pub const ENGINE_ID: &str = "engine";
const GITHUB_API: &str = "https://api.github.com";
const REPO: &str = "ggml-org/whisper.cpp";

#[cfg(windows)]
pub const SERVER_EXE: &str = "whisper-server.exe";
#[cfg(not(windows))]
pub const SERVER_EXE: &str = "whisper-server";

/// Where downloads come from (tests point these at a mock server).
#[derive(Debug, Clone)]
pub struct Sources {
    /// GitHub REST API root.
    pub github_api: String,
    /// Hugging Face folder with the ggml model files.
    pub models_base: String,
}

impl Default for Sources {
    fn default() -> Self {
        Sources {
            github_api: GITHUB_API.into(),
            models_base: catalog::HF_BASE.into(),
        }
    }
}

/// Phase of the current (or last) download.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPhase {
    Running,
    Verifying,
    Installing,
    Done,
    Failed,
    Cancelled,
}

/// The current or last download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct DownloadState {
    /// `engine` or a model id.
    pub what: String,
    pub phase: DownloadPhase,
    pub received: u64,
    /// Bytes expected (`null` while unknown).
    pub total: Option<u64>,
    /// Why it failed.
    pub error: Option<String>,
}

/// What `install.json` next to the engine records.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EngineInstall {
    /// Release tag (`b5454`).
    pub version: String,
    pub asset: String,
    pub size: u64,
    pub sha256: Option<String>,
}

/// Local Whisper's engine as installed.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EngineStatus {
    pub installed: bool,
    /// Release tag of the installed build.
    pub version: Option<String>,
    /// Size of the download.
    pub size: Option<u64>,
    /// `data/voice/bin`.
    pub folder: String,
}

/// One model of the catalog and whether it is on disk.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ModelStatus {
    #[serde(flatten)]
    pub info: ModelInfo,
    pub installed: bool,
}

/// The running whisper-server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct ServerInfo {
    pub port: u16,
    pub model: String,
    pub pid: Option<u32>,
}

/// Everything Settings → Voice shows about Local Whisper (`voice.local_status`).
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LocalStatus {
    /// Can this computer run the downloadable build (Windows x64)?
    pub supported: bool,
    pub engine: EngineStatus,
    pub models: Vec<ModelStatus>,
    /// The model transcription uses now: the chosen one if installed, else the best installed.
    pub active_model: Option<String>,
    pub download: Option<DownloadState>,
    /// The sidecar while it runs.
    pub server: Option<ServerInfo>,
}

struct Server {
    child: tokio::process::Child,
    #[cfg(windows)]
    _job: Option<win::Job>,
    info: ServerInfo,
    threads: Option<u32>,
}

/// Local Whisper's files and process (one per browser, in the registry's extensions).
pub struct LocalWhisper {
    dir: PathBuf,
    sources: Sources,
    client: reqwest::Client,
    download: Arc<Mutex<Option<DownloadState>>>,
    cancel: Mutex<Option<CancellationToken>>,
    /// Held while a server starts (so two first transcriptions start only one).
    starting: tokio::sync::Mutex<()>,
    server: Mutex<Option<Server>>,
    last_used: Mutex<Instant>,
    /// Requests running against the server: the idle timer waits for them.
    busy: std::sync::atomic::AtomicUsize,
    idle_watch: std::sync::atomic::AtomicBool,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl LocalWhisper {
    /// `dir` = `data/voice`.
    pub fn new(dir: impl Into<PathBuf>, sources: Sources) -> Self {
        LocalWhisper {
            dir: dir.into(),
            sources,
            client: crate::engines::http_client(),
            download: Arc::new(Mutex::new(None)),
            cancel: Mutex::new(None),
            starting: tokio::sync::Mutex::new(()),
            server: Mutex::new(None),
            last_used: Mutex::new(Instant::now()),
            busy: Default::default(),
            idle_watch: Default::default(),
        }
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.dir.join("bin")
    }

    pub fn models_dir(&self) -> PathBuf {
        self.dir.join("models")
    }

    pub fn model_path(&self, model: &ModelInfo) -> PathBuf {
        self.models_dir().join(model.file)
    }

    pub fn engine_installed(&self) -> bool {
        self.bin_dir().join(SERVER_EXE).is_file()
    }

    pub fn engine_install(&self) -> Option<EngineInstall> {
        let text = std::fs::read_to_string(self.bin_dir().join("install.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn model_installed(&self, model: &ModelInfo) -> bool {
        std::fs::metadata(self.model_path(model)).is_ok_and(|m| m.len() == model.size)
    }

    /// The model to use: the chosen one when installed, else the best installed one.
    pub fn active_model(&self, settings: &LocalSettings) -> Option<&'static ModelInfo> {
        if let Some(chosen) = settings.model.as_deref().and_then(catalog::model)
            && self.model_installed(chosen)
        {
            return Some(chosen);
        }
        PREFERRED
            .iter()
            .filter_map(|id| catalog::model(id))
            .find(|m| self.model_installed(m))
    }

    pub fn status(&self, settings: &LocalSettings) -> LocalStatus {
        let install = self.engine_install();
        LocalStatus {
            supported: cfg!(all(windows, target_arch = "x86_64")),
            engine: EngineStatus {
                installed: self.engine_installed(),
                version: install.as_ref().map(|i| i.version.clone()),
                size: install.as_ref().map(|i| i.size),
                folder: self.bin_dir().display().to_string(),
            },
            models: MODELS
                .iter()
                .map(|m| ModelStatus {
                    info: *m,
                    installed: self.model_installed(m),
                })
                .collect(),
            active_model: self.active_model(settings).map(|m| m.id.to_string()),
            download: lock(&self.download).clone(),
            server: self.server_info(),
        }
    }

    pub fn server_info(&self) -> Option<ServerInfo> {
        let mut server = lock(&self.server);
        let running = match server.as_mut() {
            Some(s) => matches!(s.child.try_wait(), Ok(None)),
            None => false,
        };
        if !running {
            *server = None;
        }
        server.as_ref().map(|s| s.info.clone())
    }

    // ---------------------------------------------------------------- downloads

    /// Start downloading `what` (`engine` or a model id) in the background. Errors when
    /// another download runs or `what` is unknown.
    pub fn start_download(self: &Arc<Self>, what: &str) -> Result<DownloadState, CmdError> {
        let model = if what == ENGINE_ID {
            None
        } else {
            Some(catalog::model(what).ok_or_else(|| {
                CmdError::invalid_params(format!(
                    "unknown download {what:?}: use \"engine\" or a model id ({})",
                    MODELS.iter().map(|m| m.id).collect::<Vec<_>>().join(", ")
                ))
            })?)
        };
        {
            let current = lock(&self.download);
            if let Some(d) = current.as_ref()
                && matches!(
                    d.phase,
                    DownloadPhase::Running | DownloadPhase::Verifying | DownloadPhase::Installing
                )
            {
                return Err(CmdError::new(
                    ErrorCode::InvalidParams,
                    format!("{} is still downloading", d.what),
                )
                .with_hint("Wait for it to finish, or cancel it with voice.local_cancel."));
            }
        }
        let state = DownloadState {
            what: what.to_string(),
            phase: DownloadPhase::Running,
            received: 0,
            total: model.map(|m| m.size),
            error: None,
        };
        *lock(&self.download) = Some(state.clone());
        let token = CancellationToken::new();
        *lock(&self.cancel) = Some(token.clone());
        let this = self.clone();
        tokio::spawn(async move {
            let result = match model {
                Some(model) => this.download_model(model, &token).await,
                None => this.download_engine(&token).await,
            };
            let mut state = lock(&this.download);
            if let Some(s) = state.as_mut() {
                match result {
                    Ok(()) => s.phase = DownloadPhase::Done,
                    Err(_) if token.is_cancelled() => s.phase = DownloadPhase::Cancelled,
                    Err(err) => {
                        tracing::warn!(what = s.what, %err, "voice download failed");
                        s.phase = DownloadPhase::Failed;
                        s.error = Some(err.message);
                    }
                }
            }
        });
        Ok(state)
    }

    /// Cancel the running download (no-op when none runs).
    pub fn cancel_download(&self) -> bool {
        match lock(&self.cancel).take() {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Wait until the current download is no longer running (tests).
    pub async fn wait_download(&self, timeout: Duration) -> Option<DownloadState> {
        let deadline = Instant::now() + timeout;
        loop {
            let state = lock(&self.download).clone();
            match &state {
                Some(s)
                    if matches!(
                        s.phase,
                        DownloadPhase::Running
                            | DownloadPhase::Verifying
                            | DownloadPhase::Installing
                    ) && Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                _ => return state,
            }
        }
    }

    fn set_phase(&self, phase: DownloadPhase) {
        if let Some(s) = lock(&self.download).as_mut() {
            s.phase = phase;
        }
    }

    async fn download_model(
        &self,
        model: &ModelInfo,
        cancel: &CancellationToken,
    ) -> Result<(), CmdError> {
        let dest = self.model_path(model);
        let url = format!(
            "{}/{}",
            self.sources.models_base.trim_end_matches('/'),
            model.file
        );
        let part = dest.with_extension("bin.part");
        let result = download_verified(
            &self.client,
            &url,
            &part,
            Some(model.size),
            Some(model.sha256),
            &self.download,
            cancel,
        )
        .await;
        if let Err(err) = result {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(err);
        }
        self.set_phase(DownloadPhase::Installing);
        // Replacing the model the server has loaded: stop it first (Windows locks open files).
        if self.server_info().is_some_and(|s| s.model == model.id) {
            self.stop_server();
        }
        tokio::fs::rename(&part, &dest)
            .await
            .map_err(|err| io_error("install the model", &dest, err))
    }

    /// The newest release with [`ENGINE_ASSET`]: (tag, url, size, sha256).
    async fn find_engine_release(&self) -> Result<(String, String, u64, Option<String>), CmdError> {
        let url = format!(
            "{}/repos/{REPO}/releases?per_page=30",
            self.sources.github_api.trim_end_matches('/')
        );
        let response = self
            .client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|err| CmdError::internal(format!("cannot reach GitHub: {err}")))?;
        let status = response.status();
        if !status.is_success() {
            return Err(CmdError::internal(format!(
                "GitHub answered HTTP {status} for the whisper.cpp releases"
            ))
            .with_hint("GitHub limits anonymous requests per hour; try again later."));
        }
        let releases: serde_json::Value = response
            .json()
            .await
            .map_err(|err| CmdError::internal(format!("GitHub releases: {err}")))?;
        pick_engine_asset(&releases).ok_or_else(|| {
            CmdError::new(
                ErrorCode::NotFound,
                format!("no whisper.cpp release on GitHub has {ENGINE_ASSET}"),
            )
        })
    }

    async fn download_engine(&self, cancel: &CancellationToken) -> Result<(), CmdError> {
        let (tag, url, size, sha256) = self.find_engine_release().await?;
        if let Some(s) = lock(&self.download).as_mut() {
            s.total = Some(size);
        }
        let tmp = self.dir.join("tmp");
        tokio::fs::create_dir_all(&tmp)
            .await
            .map_err(|err| io_error("create", &tmp, err))?;
        let part = tmp.join(format!("{ENGINE_ASSET}.part"));
        let downloaded = download_verified(
            &self.client,
            &url,
            &part,
            Some(size),
            sha256.as_deref(),
            &self.download,
            cancel,
        )
        .await;
        if let Err(err) = downloaded {
            let _ = tokio::fs::remove_dir_all(&tmp).await;
            return Err(err);
        }
        self.set_phase(DownloadPhase::Installing);
        // No server may start from bin/ while it is swapped: `ensure_server` waits for this
        // lock (a transcription that comes in now starts the new engine afterwards).
        let _starting = self.starting.lock().await;
        // The running server keeps its exe and DLLs open: wait until it is gone.
        self.stop_server_and_wait().await;
        let bin = self.bin_dir();
        let staging = self.dir.join("bin.new");
        let install = EngineInstall {
            version: tag,
            asset: ENGINE_ASSET.into(),
            size,
            sha256,
        };
        let result = tokio::task::spawn_blocking({
            let (part, staging, bin) = (part.clone(), staging.clone(), bin.clone());
            move || install_engine(&part, &staging, &bin, &install)
        })
        .await
        .map_err(|err| CmdError::internal(format!("installing whisper.cpp: {err}")))?;
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        let _ = tokio::fs::remove_dir_all(&staging).await;
        result
    }

    /// Delete `engine` or a model (stops the server when it uses it).
    pub fn delete(&self, what: &str) -> Result<(), CmdError> {
        if lock(&self.download)
            .as_ref()
            .is_some_and(|d| d.what == what && matches!(d.phase, DownloadPhase::Running))
        {
            self.cancel_download();
        }
        if what == ENGINE_ID {
            self.stop_server();
            let bin = self.bin_dir();
            return match std::fs::remove_dir_all(&bin) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_error("delete", &bin, e)),
            };
        }
        let model = catalog::model(what)
            .ok_or_else(|| CmdError::invalid_params(format!("unknown model {what:?}")))?;
        if self.server_info().is_some_and(|s| s.model == model.id) {
            self.stop_server();
        }
        let path = self.model_path(model);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_error("delete", &path, e)),
        }
    }

    // ---------------------------------------------------------------- the server

    /// Note that the server was just used (the idle timer starts again).
    pub fn touch(&self) {
        *lock(&self.last_used) = Instant::now();
    }

    /// Mark a request as running until the guard drops.
    pub fn busy(self: &Arc<Self>) -> BusyGuard {
        self.busy.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.touch();
        BusyGuard(self.clone())
    }

    /// The server's base URL, starting it when needed (with the model from `settings`).
    pub async fn ensure_server(
        self: &Arc<Self>,
        settings: &LocalSettings,
    ) -> Result<String, CmdError> {
        let model = self.active_model(settings).ok_or_else(|| {
            CmdError::new(ErrorCode::NotFound, "no Whisper model is downloaded")
                .with_hint("Settings → Voice → Local Whisper: download a model (Base for a quick start, Large v3 Turbo for Ukrainian).")
        })?;
        if !self.engine_installed() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                "the whisper.cpp engine is not downloaded",
            )
            .with_hint("Settings → Voice → Local Whisper: click Download next to the engine."));
        }
        let _starting = self.starting.lock().await;
        // An engine reinstall held the lock: bin/ may have changed while we waited.
        if !self.engine_installed() {
            return Err(CmdError::new(
                ErrorCode::NotFound,
                "the whisper.cpp engine is not downloaded",
            )
            .with_hint("Settings → Voice → Local Whisper: click Download next to the engine."));
        }
        if let Some(info) = self.server_info()
            && info.model == model.id
            && lock(&self.server)
                .as_ref()
                .is_some_and(|s| s.threads == settings.threads)
        {
            self.touch();
            return Ok(format!("http://127.0.0.1:{}", info.port));
        }
        self.stop_server();
        let port = free_port().map_err(|err| CmdError::internal(format!("no free port: {err}")))?;
        let log_path = self.dir.join("whisper-server.log");
        let log =
            std::fs::File::create(&log_path).map_err(|err| io_error("create", &log_path, err))?;
        let log_err = log
            .try_clone()
            .map_err(|err| io_error("open", &log_path, err))?;
        let mut cmd = tokio::process::Command::new(self.bin_dir().join(SERVER_EXE));
        cmd.arg("-m").arg(self.model_path(model)).args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ]);
        if let Some(threads) = settings.threads.filter(|t| *t > 0) {
            cmd.args(["-t", &threads.to_string()]);
        }
        cmd.current_dir(self.bin_dir())
            .stdin(std::process::Stdio::null())
            .stdout(log)
            .stderr(log_err)
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|err| {
            CmdError::internal(format!("cannot start {SERVER_EXE}: {err}"))
                .with_hint("Download the engine again in Settings → Voice.")
        })?;
        let pid = child.id();
        #[cfg(windows)]
        let job = pid.and_then(|pid| win::Job::with_process(pid).ok());
        tracing::info!(port, model = model.id, ?pid, "whisper-server starting");
        // Ready once it listens (it loads the model first: large ones take a few seconds).
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let tail = std::fs::read_to_string(&log_path)
                    .unwrap_or_default()
                    .lines()
                    .rev()
                    .take(6)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" | ");
                return Err(CmdError::internal(format!(
                    "whisper-server stopped at start ({status}): {tail}"
                )));
            }
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            if Instant::now() > deadline {
                let _ = child.start_kill();
                return Err(CmdError::new(
                    ErrorCode::Timeout,
                    "whisper-server did not start within 2 minutes",
                ));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        let info = ServerInfo {
            port,
            model: model.id.to_string(),
            pid,
        };
        *lock(&self.server) = Some(Server {
            child,
            #[cfg(windows)]
            _job: job,
            info,
            threads: settings.threads,
        });
        self.touch();
        self.watch_idle(settings.idle_minutes);
        Ok(format!("http://127.0.0.1:{port}"))
    }

    /// Stop the server after `minutes` without use (one watcher at a time).
    fn watch_idle(self: &Arc<Self>, minutes: u32) {
        use std::sync::atomic::Ordering;
        if self.idle_watch.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        let idle = Duration::from_secs(u64::from(minutes.max(1)) * 60);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let Some(this) = weak.upgrade() else { return };
                if this.server_info().is_none() {
                    this.idle_watch.store(false, Ordering::SeqCst);
                    return;
                }
                let idle_for = lock(&this.last_used).elapsed();
                if this.busy.load(Ordering::SeqCst) == 0 && idle_for >= idle {
                    tracing::info!(?idle_for, "whisper-server idle: stopping");
                    this.stop_server();
                    this.idle_watch.store(false, Ordering::SeqCst);
                    return;
                }
            }
        });
    }

    /// Stop the sidecar (no-op when it is not running).
    pub fn stop_server(&self) {
        if let Some(mut server) = lock(&self.server).take() {
            let _ = server.child.start_kill();
            tracing::info!(port = server.info.port, "whisper-server stopped");
        }
    }

    /// Stop the sidecar and wait (up to 5 s) until the process has exited, so its files can
    /// be replaced.
    async fn stop_server_and_wait(&self) {
        let server = lock(&self.server).take();
        if let Some(mut server) = server {
            let _ = server.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), server.child.wait()).await;
            tracing::info!(port = server.info.port, "whisper-server stopped");
        }
    }

    /// The browser quits.
    pub fn shutdown(&self) {
        self.cancel_download();
        self.stop_server();
    }
}

/// Ends a [`LocalWhisper::busy`] request.
pub struct BusyGuard(Arc<LocalWhisper>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0
            .busy
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        self.0.touch();
    }
}

/// From a GitHub `releases` listing: the newest non-draft release with [`ENGINE_ASSET`] →
/// (tag, download URL, size, sha256 from the asset's `digest`).
pub fn pick_engine_asset(
    releases: &serde_json::Value,
) -> Option<(String, String, u64, Option<String>)> {
    let mut candidates: Vec<&serde_json::Value> = releases
        .as_array()?
        .iter()
        .filter(|r| r["draft"] != true)
        .collect();
    // Newest first, whatever order GitHub used.
    candidates.sort_by(|a, b| {
        b["published_at"]
            .as_str()
            .unwrap_or("")
            .cmp(a["published_at"].as_str().unwrap_or(""))
    });
    candidates.into_iter().find_map(|release| {
        let asset = release["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"] == ENGINE_ASSET)?;
        let sha = asset["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_ascii_lowercase);
        Some((
            release["tag_name"].as_str()?.to_string(),
            asset["browser_download_url"].as_str()?.to_string(),
            asset["size"].as_u64()?,
            sha,
        ))
    })
}

/// Download `url` to `dest`, updating `progress.received`, then check the size and SHA-256.
pub async fn download_verified(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    size: Option<u64>,
    sha256: Option<&str>,
    progress: &Mutex<Option<DownloadState>>,
    cancel: &CancellationToken,
) -> Result<(), CmdError> {
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|err| io_error("create", dir, err))?;
    }
    let response = tokio::select! {
        _ = cancel.cancelled() => {
            return Err(CmdError::new(ErrorCode::InvalidParams, "download cancelled"));
        }
        response = client.get(url).send() => response,
    }
    .map_err(|err| CmdError::internal(format!("cannot download {url}: {err}")))?;
    if !response.status().is_success() {
        return Err(CmdError::internal(format!(
            "download of {url} failed: HTTP {}",
            response.status()
        )));
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|err| io_error("create", dest, err))?;
    let mut hasher = sha2::Sha256::new();
    let mut received: u64 = 0;
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => {
                return Err(CmdError::new(ErrorCode::InvalidParams, "download cancelled"));
            }
            chunk = tokio::time::timeout(Duration::from_secs(60), stream.next()) => chunk,
        };
        let chunk = match chunk {
            Err(_) => {
                return Err(CmdError::new(
                    ErrorCode::Timeout,
                    "the download stalled for 60 s",
                ));
            }
            Ok(None) => break,
            Ok(Some(Err(err))) => {
                return Err(CmdError::internal(format!("download interrupted: {err}")));
            }
            Ok(Some(Ok(chunk))) => chunk,
        };
        received += chunk.len() as u64;
        if let Some(size) = size
            && received > size
        {
            return Err(CmdError::internal(format!(
                "the download is larger than the published {size} bytes"
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|err| io_error("write", dest, err))?;
        if let Some(s) = lock(progress).as_mut() {
            s.received = received;
        }
    }
    file.flush()
        .await
        .map_err(|err| io_error("write", dest, err))?;
    drop(file);
    if let Some(s) = lock(progress).as_mut() {
        s.phase = DownloadPhase::Verifying;
    }
    if let Some(size) = size
        && received != size
    {
        return Err(CmdError::internal(format!(
            "size mismatch: got {received} bytes, the source publishes {size}"
        )));
    }
    if let Some(expected) = sha256 {
        let actual = hex(&hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(CmdError::internal(format!(
                "SHA-256 mismatch: got {actual}, the source publishes {expected}"
            )));
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpack the zip into `staging`, then move the folder holding [`SERVER_EXE`] to `bin`.
fn install_engine(
    zip_path: &Path,
    staging: &Path,
    bin: &Path,
    install: &EngineInstall,
) -> Result<(), CmdError> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(|e| io_error("create", staging, e))?;
    let file = std::fs::File::open(zip_path).map_err(|e| io_error("open", zip_path, e))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| CmdError::internal(format!("the whisper.cpp download is not a zip: {e}")))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| CmdError::internal(format!("broken zip entry: {e}")))?;
        // Never write outside the staging folder (`../` in entry names).
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let out = staging.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| io_error("create", &out, e))?;
            continue;
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io_error("create", dir, e))?;
        }
        let mut target = std::fs::File::create(&out).map_err(|e| io_error("create", &out, e))?;
        std::io::copy(&mut entry, &mut target).map_err(|e| io_error("write", &out, e))?;
    }
    let root = find_dir_with(staging, SERVER_EXE, 3).ok_or_else(|| {
        CmdError::internal(format!("{SERVER_EXE} is not in the whisper.cpp download"))
    })?;
    let json = serde_json::to_string_pretty(install).unwrap_or_default();
    std::fs::write(root.join("install.json"), json)
        .map_err(|e| io_error("write", &root.join("install.json"), e))?;
    swap_dir(&root, bin)
}

/// Put the complete folder `new` in place of `bin` by renames only, so `bin` is always either
/// the old engine or the new one, never a half-deleted mix: `bin` → `bin.old`, `new` → `bin`,
/// then delete `bin.old`. When the second step fails, the old engine is moved back.
fn swap_dir(new: &Path, bin: &Path) -> Result<(), CmdError> {
    let mut old = bin.as_os_str().to_owned();
    old.push(".old");
    let old = PathBuf::from(old);
    // Leftovers of an earlier swap that could not delete them.
    let _ = std::fs::remove_dir_all(&old);
    let had_old = match rename_retry(bin, &old) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(io_error("replace", bin, e)),
    };
    if let Err(e) = rename_retry(new, bin) {
        if had_old && let Err(back) = rename_retry(&old, bin) {
            tracing::warn!(%back, "could not restore the previous whisper.cpp engine");
        }
        return Err(io_error("install", bin, e));
    }
    if had_old && let Err(e) = std::fs::remove_dir_all(&old) {
        // Harmless: the next install removes it first.
        tracing::warn!(%e, path = %old.display(), "could not delete the previous engine");
    }
    Ok(())
}

/// `rename`, retried for a moment: right after a process exits (or while an antivirus scans
/// the new files) Windows may still refuse to move its folder.
fn rename_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(e),
            Err(_) if tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e),
        }
    }
}

/// The folder under `dir` (itself included, `depth` levels down) that contains `file`.
fn find_dir_with(dir: &Path, file: &str, depth: usize) -> Option<PathBuf> {
    if dir.join(file).is_file() {
        return Some(dir.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();
    subdirs
        .iter()
        .find_map(|d| find_dir_with(d, file, depth - 1))
}

fn free_port() -> std::io::Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn io_error(what: &str, path: &Path, err: std::io::Error) -> CmdError {
    CmdError::internal(format!("cannot {what} {}: {err}", path.display()))
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    /// A kill-on-close job: when the last handle closes (we drop it, or our process ends,
    /// even by a crash) Windows ends the sidecar.
    pub struct Job(HANDLE);

    // A plain kernel object handle; any thread may use or close it.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn with_process(pid: u32) -> std::io::Result<Job> {
            // SAFETY: plain Win32 calls with pointers to live locals; each handle is closed
            // exactly once (`Job` owns the job handle, the process handle is closed here).
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let job = Job(handle);
                let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const limits).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let assigned = AssignProcessToJobObject(job.0, process);
                let err = std::io::Error::last_os_error();
                CloseHandle(process);
                if assigned == 0 {
                    return Err(err);
                }
                Ok(job)
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: we own the handle and close it once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod swap_tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn zip_file(path: &Path, name: &str, bytes: &[u8]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        z.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut z, bytes).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn the_new_engine_replaces_the_old_one_completely() {
        let tmp = tempfile::tempdir().unwrap();
        let (bin, new) = (tmp.path().join("bin"), tmp.path().join("bin.new"));
        write(&bin, SERVER_EXE, "old");
        write(&bin, "old-only.dll", "old");
        write(&new, SERVER_EXE, "new");
        swap_dir(&new, &bin).unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join(SERVER_EXE)).unwrap(),
            "new"
        );
        assert!(!bin.join("old-only.dll").exists());
        assert!(!new.exists());
        assert!(!tmp.path().join("bin.old").exists());
        // First install: there is no old engine.
        let (bin2, new2) = (tmp.path().join("b2"), tmp.path().join("b2.new"));
        write(&new2, SERVER_EXE, "first");
        swap_dir(&new2, &bin2).unwrap();
        assert_eq!(
            std::fs::read_to_string(bin2.join(SERVER_EXE)).unwrap(),
            "first"
        );
    }

    #[test]
    fn a_failed_swap_keeps_the_old_engine_whole() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        write(&bin, SERVER_EXE, "old");
        write(&bin, "ggml.dll", "old");
        // The new folder cannot be moved in (here: it does not exist).
        let err = swap_dir(&tmp.path().join("missing"), &bin).unwrap_err();
        assert!(err.message.contains("install"), "{err:?}");
        assert_eq!(
            std::fs::read_to_string(bin.join(SERVER_EXE)).unwrap(),
            "old"
        );
        assert!(bin.join("ggml.dll").is_file());
        assert!(!tmp.path().join("bin.old").exists());
    }

    #[test]
    fn install_records_the_version_and_a_broken_download_keeps_the_engine() {
        let tmp = tempfile::tempdir().unwrap();
        let (bin, staging) = (tmp.path().join("bin"), tmp.path().join("bin.new"));
        write(&bin, SERVER_EXE, "old");
        let zip_path = tmp.path().join("e.zip");
        zip_file(&zip_path, &format!("Release/{SERVER_EXE}"), b"new");
        let install = EngineInstall {
            version: "b1".into(),
            asset: ENGINE_ASSET.into(),
            size: 3,
            sha256: None,
        };
        install_engine(&zip_path, &staging, &bin, &install).unwrap();
        assert_eq!(
            std::fs::read_to_string(bin.join(SERVER_EXE)).unwrap(),
            "new"
        );
        assert!(bin.join("install.json").is_file());
        // A download without the server keeps the installed engine.
        let empty = tmp.path().join("empty.zip");
        zip_file(&empty, "readme.txt", b"");
        assert!(install_engine(&empty, &staging, &bin, &install).is_err());
        assert_eq!(
            std::fs::read_to_string(bin.join(SERVER_EXE)).unwrap(),
            "new"
        );
    }

    /// A transcription that arrives while the engine is being replaced waits for the swap
    /// and then sees the new bin/ (it never starts the old exe mid-delete).
    #[tokio::test]
    async fn ensure_server_waits_while_the_engine_is_replaced() {
        let tmp = tempfile::tempdir().unwrap();
        let lw = Arc::new(LocalWhisper::new(tmp.path(), Sources::default()));
        let model = MODELS.iter().min_by_key(|m| m.size).unwrap();
        std::fs::create_dir_all(lw.models_dir()).unwrap();
        std::fs::File::create(lw.model_path(model))
            .unwrap()
            .set_len(model.size)
            .unwrap();
        let settings = LocalSettings {
            model: Some(model.id.to_string()),
            ..LocalSettings::default()
        };
        write(&lw.bin_dir(), SERVER_EXE, "old");
        // The install holds the lock and removes the engine (a failed reinstall).
        let guard = lw.starting.lock().await;
        let ensure = tokio::spawn({
            let lw = lw.clone();
            async move { lw.ensure_server(&settings).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !ensure.is_finished(),
            "ensure_server must wait for the install"
        );
        std::fs::remove_dir_all(lw.bin_dir()).unwrap();
        drop(guard);
        let err = ensure.await.unwrap().unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "{err:?}");
        assert!(lw.server_info().is_none());
    }
}
