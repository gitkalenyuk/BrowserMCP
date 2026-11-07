//! [`VoiceService`]: settings, engine resolution, one-shot transcription and the live streams
//! the UI feeds while the user speaks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use bmcp_core::{CmdError, Ctx, DataDir, ErrorCode};

use crate::deepgram::Live;
use crate::engines::{self, Audio, Endpoint, Options, Transcript};
use crate::local::{LocalWhisper, Sources};
use crate::settings::{EngineId, KeySource, VoiceSettings, ai_provider_key};
use crate::wav;

/// Longest recording a stream takes (2 min auto-stop + a margin), in samples.
pub const MAX_STREAM_SAMPLES: usize = wav::SAMPLE_RATE as usize * 130;
/// Shorter than this is treated as nothing said (no engine call).
pub const MIN_SPEECH_SAMPLES: usize = wav::SAMPLE_RATE as usize * 3 / 10;
/// Local Whisper re-transcribes the growing audio at most this often for interim text.
pub const INTERIM_EVERY: Duration = Duration::from_millis(1500);
/// A stream nobody pushed to for this long is dropped.
const STREAM_IDLE: Duration = Duration::from_secs(60);
/// How often the background reaper looks for idle streams while any is open.
const REAP_EVERY: Duration = Duration::from_secs(15);
const MAX_STREAMS: usize = 8;

/// How an engine gets its key, and whether it is ready.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub engine: EngineId,
    /// `None` for local (its URL is known once the server runs).
    pub endpoint: Option<Endpoint>,
    pub key_source: Option<KeySource>,
}

/// Why an engine cannot be used (`None` = it can).
pub fn not_ready(engine: EngineId) -> CmdError {
    CmdError::new(
        ErrorCode::InvalidParams,
        format!("{} has no API key", engine.name()),
    )
    .with_hint(
        "Add the key in Settings → Voice (OpenAI and Groq also use the key from Settings → AI).",
    )
}

pub struct VoiceService {
    config_dir: PathBuf,
    pub local: Arc<LocalWhisper>,
    client: reqwest::Client,
    streams: Mutex<HashMap<String, Arc<Stream>>>,
    next_stream: AtomicU64,
    /// The idle-stream reaper runs (one at a time, only while streams are open).
    reaping: AtomicBool,
    /// Serializes `voice.settings_set` (read → change → write).
    pub writes: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl VoiceService {
    /// Settings in `data/config/voice.json`, Local Whisper in `data/voice`.
    pub fn new(data_root: &Path) -> Self {
        Self::with_sources(data_root, Sources::default())
    }

    pub fn with_sources(data_root: &Path, sources: Sources) -> Self {
        VoiceService {
            config_dir: data_root.join("config"),
            local: Arc::new(LocalWhisper::new(data_root.join("voice"), sources)),
            client: engines::http_client(),
            streams: Mutex::new(HashMap::new()),
            next_stream: AtomicU64::new(1),
            reaping: AtomicBool::new(false),
            writes: Mutex::new(()),
        }
    }

    /// The registry's service; made from the data folder on first use when the app did not
    /// [`crate::install`] one (tests, schema dump).
    pub fn from_ctx(ctx: &Ctx) -> Result<Arc<VoiceService>, CmdError> {
        if let Some(service) = ctx.extensions.get::<VoiceService>() {
            return Ok(service);
        }
        let data = ctx.extensions.get::<DataDir>().ok_or_else(|| {
            CmdError::internal("voice needs the data folder (bmcp_voice::install)")
        })?;
        Ok(ctx
            .extensions
            .get_or_insert_with(|| VoiceService::new(data.root())))
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn settings(&self) -> Result<VoiceSettings, CmdError> {
        VoiceSettings::load(&self.config_dir)
    }

    pub fn store(&self, settings: &VoiceSettings) -> Result<(), CmdError> {
        settings.save(&self.config_dir)
    }

    /// The key an engine uses and where it came from: the voice settings first, then (OpenAI,
    /// Groq) the AI provider of the same name. Also returns that provider's API root.
    pub fn key_for(
        &self,
        settings: &VoiceSettings,
        engine: EngineId,
    ) -> Option<(String, KeySource, Option<String>)> {
        let own = settings
            .engine(engine)?
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty());
        if let Some(key) = own {
            return Some((key.to_string(), KeySource::Voice, None));
        }
        if matches!(engine, EngineId::Openai | EngineId::Groq) {
            return ai_provider_key(&self.config_dir, engine.as_str())
                .map(|p| (p.api_key, KeySource::AiProvider, p.base_url));
        }
        None
    }

    /// Resolve `engine` with its settings; errors when it is not set up.
    pub fn resolve(
        &self,
        settings: &VoiceSettings,
        engine: EngineId,
    ) -> Result<Resolved, CmdError> {
        if engine == EngineId::Local {
            return Ok(Resolved {
                engine,
                endpoint: None,
                key_source: None,
            });
        }
        let cfg = settings.engine(engine).cloned().unwrap_or_default();
        let key = self.key_for(settings, engine);
        let model = cfg
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| {
                engine
                    .models()
                    .first()
                    .map_or("whisper-1".to_string(), |m| (*m).to_string())
            });
        let base_url = cfg
            .base_url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .or_else(|| key.as_ref().and_then(|k| k.2.clone()))
            .unwrap_or_else(|| engine.default_base_url().to_string());
        if engine == EngineId::Custom && base_url.is_empty() {
            return Err(CmdError::new(
                ErrorCode::InvalidParams,
                "the custom engine has no API address",
            )
            .with_hint(
                "Settings → Voice → Custom: enter the base URL, e.g. http://localhost:8000/v1.",
            ));
        }
        let key_source = key.as_ref().map(|k| k.1);
        let api_key = key.map(|k| k.0);
        let endpoint = match engine {
            EngineId::Openai | EngineId::Groq | EngineId::Custom => {
                if engine.needs_key() && api_key.is_none() {
                    return Err(not_ready(engine));
                }
                Endpoint::OpenAiCompatible {
                    engine,
                    base_url,
                    api_key,
                    model,
                }
            }
            EngineId::Elevenlabs => Endpoint::ElevenLabs {
                base_url,
                api_key: api_key.ok_or_else(|| not_ready(engine))?,
                model,
            },
            EngineId::Deepgram => Endpoint::Deepgram {
                base_url,
                api_key: api_key.ok_or_else(|| not_ready(engine))?,
                model,
            },
            EngineId::Local => unreachable!(),
        };
        Ok(Resolved {
            engine,
            endpoint: Some(endpoint),
            key_source,
        })
    }

    /// Can `engine` be used right now? `Err(reason)` otherwise.
    pub fn ready(&self, settings: &VoiceSettings, engine: EngineId) -> Result<(), CmdError> {
        if engine == EngineId::Local {
            if !self.local.engine_installed() {
                return Err(CmdError::new(
                    ErrorCode::NotFound,
                    "the whisper.cpp engine is not downloaded",
                ));
            }
            if self.local.active_model(&settings.local).is_none() {
                return Err(CmdError::new(
                    ErrorCode::NotFound,
                    "no Whisper model is downloaded",
                ));
            }
            return Ok(());
        }
        self.resolve(settings, engine).map(|_| ())
    }

    /// Transcribe one recording with `engine`.
    pub async fn transcribe(
        &self,
        settings: &VoiceSettings,
        engine: EngineId,
        audio: Audio<'_>,
        opts: &Options,
    ) -> Result<Transcript, CmdError> {
        let resolved = self.resolve(settings, engine)?;
        match resolved.endpoint {
            Some(endpoint) => engines::transcribe(&self.client, &endpoint, audio, opts).await,
            None => {
                let base = audio.mime.split(';').next().unwrap_or("").trim();
                if !matches!(base, "audio/wav" | "audio/x-wav" | "audio/wave" | "") {
                    return Err(CmdError::invalid_params(format!(
                        "Local Whisper takes WAV audio (16 kHz mono), not {base}"
                    )));
                }
                let _busy = self.local.busy();
                let base_url = self.local.ensure_server(&settings.local).await?;
                let endpoint = Endpoint::Whisper { base_url };
                let result = engines::transcribe(&self.client, &endpoint, audio, opts).await;
                if result
                    .as_ref()
                    .is_err_and(|e| e.message.starts_with("cannot reach"))
                {
                    // A crashed server is started again by the next request.
                    self.local.stop_server();
                }
                result
            }
        }
    }

    // ---------------------------------------------------------------- streams

    /// Open a stream for `engine` (`Deepgram` connects its live socket now).
    pub async fn stream_start(
        self: &Arc<Self>,
        settings: &VoiceSettings,
        engine: EngineId,
        opts: Options,
    ) -> Result<Arc<Stream>, CmdError> {
        self.reap();
        if lock(&self.streams).len() >= MAX_STREAMS {
            return Err(CmdError::invalid_params("too many voice streams are open")
                .with_hint("Stop the other streams with voice.stream_stop first."));
        }
        let resolved = self.resolve(settings, engine)?;
        if engine == EngineId::Local {
            self.ready(settings, engine)?;
            // Start the server while the user speaks, so the first interim text is quick.
            let local = self.local.clone();
            let local_settings = settings.local.clone();
            tokio::spawn(async move {
                if let Err(err) = local.ensure_server(&local_settings).await {
                    tracing::warn!(%err, "whisper-server did not start");
                }
            });
        }
        let live = match &resolved.endpoint {
            Some(Endpoint::Deepgram {
                base_url,
                api_key,
                model,
            }) => match Live::connect(base_url, api_key, model, opts.language.as_deref()).await {
                Ok(live) => Some(live),
                Err(err) if err.message.contains("refused") => return Err(err),
                Err(err) => {
                    // No live socket (network, proxy): the audio is sent in one piece at stop.
                    tracing::warn!(%err, "Deepgram live unavailable; using batch");
                    None
                }
            },
            _ => None,
        };
        let id = format!("v{}", self.next_stream.fetch_add(1, Ordering::Relaxed));
        let stream = Arc::new(Stream {
            id: id.clone(),
            engine,
            opts,
            settings: settings.clone(),
            pcm: Mutex::new(Vec::new()),
            partial: Mutex::new(String::new()),
            live: Mutex::new(live),
            last_seen: Mutex::new(Instant::now()),
            interim_running: AtomicBool::new(false),
            interim_at: Mutex::new(None),
            interim_len: Mutex::new(0),
            finished: AtomicBool::new(false),
        });
        lock(&self.streams).insert(id, stream.clone());
        self.watch_streams();
        Ok(stream)
    }

    pub fn stream(&self, id: &str) -> Result<Arc<Stream>, CmdError> {
        lock(&self.streams).get(id).cloned().ok_or_else(|| {
            CmdError::new(ErrorCode::NotFound, format!("no voice stream {id:?}"))
                .with_hint("It was stopped or expired (60 s without audio); start a new one.")
        })
    }

    /// Add 16-bit PCM to a stream; returns the text heard so far.
    pub fn stream_push(self: &Arc<Self>, id: &str, pcm: &[u8]) -> Result<(String, u64), CmdError> {
        let stream = self.stream(id)?;
        *lock(&stream.last_seen) = Instant::now();
        let samples = wav::samples_from_le_bytes(pcm);
        let total = {
            let mut buf = lock(&stream.pcm);
            let room = MAX_STREAM_SAMPLES.saturating_sub(buf.len());
            buf.extend_from_slice(&samples[..samples.len().min(room)]);
            buf.len()
        };
        if let Some(live) = lock(&stream.live).as_ref() {
            live.push(pcm.to_vec());
            *lock(&stream.partial) = live.text().text();
        } else if stream.engine == EngineId::Local {
            self.maybe_interim(&stream, total);
        }
        let duration_ms = total as u64 * 1000 / u64::from(wav::SAMPLE_RATE);
        Ok((lock(&stream.partial).clone(), duration_ms))
    }

    /// Local Whisper: re-transcribe the whole recording in the background when enough new
    /// audio arrived since the last pass (the result is the next interim text).
    fn maybe_interim(self: &Arc<Self>, stream: &Arc<Stream>, total: usize) {
        let due = lock(&stream.interim_at).is_none_or(|at| at.elapsed() >= INTERIM_EVERY);
        let grown =
            total.saturating_sub(*lock(&stream.interim_len)) >= wav::SAMPLE_RATE as usize / 2;
        if !due || !grown || total < MIN_SPEECH_SAMPLES * 2 {
            return;
        }
        if stream.interim_running.swap(true, Ordering::SeqCst) {
            return;
        }
        *lock(&stream.interim_len) = total;
        let (this, stream) = (self.clone(), stream.clone());
        tokio::spawn(async move {
            let audio = wav::encode(&lock(&stream.pcm).clone(), wav::SAMPLE_RATE, 1);
            let result = this
                .transcribe(
                    &stream.settings,
                    EngineId::Local,
                    Audio {
                        bytes: &audio,
                        mime: "audio/wav",
                    },
                    &stream.opts,
                )
                .await;
            match result {
                Ok(t) if !stream.finished.load(Ordering::SeqCst) => {
                    *lock(&stream.partial) = t.text;
                }
                Ok(_) => {}
                Err(err) => tracing::debug!(%err, "interim transcription failed"),
            }
            *lock(&stream.interim_at) = Some(Instant::now());
            stream.interim_running.store(false, Ordering::SeqCst);
        });
    }

    /// Close a stream: the final transcript (`cancel` = discard everything, no engine call).
    pub async fn stream_stop(
        &self,
        id: &str,
        cancel: bool,
    ) -> Result<(Transcript, EngineId), CmdError> {
        let stream = lock(&self.streams)
            .remove(id)
            .ok_or_else(|| CmdError::new(ErrorCode::NotFound, format!("no voice stream {id:?}")))?;
        stream.finished.store(true, Ordering::SeqCst);
        let live = lock(&stream.live).take();
        let pcm = std::mem::take(&mut *lock(&stream.pcm));
        let duration_ms = pcm.len() as u64 * 1000 / u64::from(wav::SAMPLE_RATE);
        let empty = Transcript {
            text: String::new(),
            language: None,
            duration_ms: Some(duration_ms),
        };
        if cancel {
            if let Some(live) = live {
                live.abort();
            }
            return Ok((empty, stream.engine));
        }
        if let Some(live) = live {
            let heard = live.finish(Duration::from_secs(5)).await;
            if heard.error.is_none() || !heard.text().is_empty() {
                return Ok((
                    Transcript {
                        text: engines::clean_text(&heard.text()),
                        language: stream.opts.language.clone(),
                        duration_ms: Some(duration_ms),
                    },
                    stream.engine,
                ));
            }
            tracing::warn!(error = ?heard.error, "Deepgram live failed; sending the audio in one piece");
        }
        if pcm.len() < MIN_SPEECH_SAMPLES {
            return Ok((empty, stream.engine));
        }
        let audio = wav::encode(&pcm, wav::SAMPLE_RATE, 1);
        let mut t = self
            .transcribe(
                &stream.settings,
                stream.engine,
                Audio {
                    bytes: &audio,
                    mime: "audio/wav",
                },
                &stream.opts,
            )
            .await?;
        t.duration_ms = Some(duration_ms);
        Ok((t, stream.engine))
    }

    /// Drop streams nobody fed for [`STREAM_IDLE`] (a page that went away mid-recording),
    /// closing their Deepgram sockets.
    fn reap(&self) {
        let idle: Vec<Arc<Stream>> = {
            let mut streams = lock(&self.streams);
            let ids: Vec<String> = streams
                .iter()
                .filter(|(_, s)| lock(&s.last_seen).elapsed() >= STREAM_IDLE)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| streams.remove(id)).collect()
        };
        for stream in idle {
            tracing::debug!(stream = stream.id, "voice stream idle: dropped");
            stream.finished.store(true, Ordering::SeqCst);
            if let Some(live) = lock(&stream.live).take() {
                live.abort();
            }
        }
    }

    /// Reap idle streams every [`REAP_EVERY`] while any is open, so an abandoned stream
    /// does not hold its slot (and a live socket) until the next `stream_start`.
    fn watch_streams(self: &Arc<Self>) {
        if self.reaping.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REAP_EVERY).await;
                let Some(this) = weak.upgrade() else { return };
                this.reap();
                if lock(&this.streams).is_empty() {
                    this.reaping.store(false, Ordering::SeqCst);
                    // A stream opened between the check and the store starts its own watcher;
                    // one opened just before it would be missed, so look once more.
                    if lock(&this.streams).is_empty() || this.reaping.swap(true, Ordering::SeqCst) {
                        return;
                    }
                }
            }
        });
    }

    /// Age a stream as if nobody had pushed to it for `by` (tests of the reaper).
    #[cfg(test)]
    pub(crate) fn age_stream(&self, id: &str, by: Duration) {
        if let Some(s) = lock(&self.streams).get(id) {
            let mut seen = lock(&s.last_seen);
            *seen = seen.checked_sub(by).unwrap_or(*seen);
        }
    }

    /// Run the reaper now (tests).
    #[cfg(test)]
    pub(crate) fn reap_now(&self) {
        self.reap();
    }

    pub fn open_streams(&self) -> usize {
        lock(&self.streams).len()
    }

    /// The browser quits: stop the sidecar and any download.
    pub fn shutdown(&self) {
        lock(&self.streams).clear();
        self.local.shutdown();
    }
}

/// One recording in progress.
pub struct Stream {
    pub id: String,
    pub engine: EngineId,
    pub opts: Options,
    settings: VoiceSettings,
    pcm: Mutex<Vec<i16>>,
    partial: Mutex<String>,
    live: Mutex<Option<Live>>,
    last_seen: Mutex<Instant>,
    interim_running: AtomicBool,
    interim_at: Mutex<Option<Instant>>,
    interim_len: Mutex<usize>,
    finished: AtomicBool,
}

impl Stream {
    /// Text appears while speaking (Deepgram live, Local Whisper re-transcription).
    pub fn interim(&self) -> bool {
        self.engine == EngineId::Local || lock(&self.live).is_some()
    }

    pub fn live(&self) -> bool {
        lock(&self.live).is_some()
    }
}
