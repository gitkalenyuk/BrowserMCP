//! The `voice.*` commands.

use std::time::Instant;

use base64::Engine as _;
use bmcp_core::command::Empty;
use bmcp_core::{CmdError, Command, Ctx, Meta, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engines::{Audio, Options, Transcript};
use crate::local::LocalStatus;
use crate::service::VoiceService;
use crate::settings::{
    EngineId, EngineSettings, KeySource, VoiceMode, VoiceSettings, normalize_hotkey,
    normalize_language,
};
use crate::{catalog, wav};

// ------------------------------------------------------------------ views

/// A cloud engine's settings as clients see them: never the key itself.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EngineSettingsView {
    /// A key is stored in the voice settings.
    pub api_key_set: bool,
    /// Its last 4 characters, to tell keys apart.
    pub api_key_hint: Option<String>,
    /// OpenAI / Groq: the AI provider of the same name has a key that is used when this
    /// engine has none of its own.
    pub ai_provider_key: bool,
    /// `null` = the engine's default model.
    pub model: Option<String>,
    /// `null` = the engine's own API address.
    pub base_url: Option<String>,
}

/// Local Whisper's settings.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct LocalSettingsView {
    /// Chosen model id; `null` = the best one installed.
    pub model: Option<String>,
    pub threads: Option<u32>,
    pub idle_minutes: u32,
}

/// `voice.settings_get` / `voice.settings_set`.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct VoiceSettingsView {
    pub enabled: bool,
    pub engine: EngineId,
    pub mode: VoiceMode,
    pub hotkey: String,
    pub auto_submit_hold: bool,
    pub auto_submit_tap: bool,
    /// `""` = the UI language, `auto` = detect, else a language code.
    pub language: String,
    pub page_hint: bool,
    pub local: LocalSettingsView,
    pub openai: EngineSettingsView,
    pub groq: EngineSettingsView,
    pub elevenlabs: EngineSettingsView,
    pub deepgram: EngineSettingsView,
    pub custom: EngineSettingsView,
}

fn view(service: &VoiceService, s: &VoiceSettings) -> VoiceSettingsView {
    let engine = |id: EngineId, e: &EngineSettings| {
        let key = e
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty());
        EngineSettingsView {
            api_key_set: key.is_some(),
            api_key_hint: key.filter(|k| k.chars().count() >= 8).map(|k| {
                let tail: String = k
                    .chars()
                    .rev()
                    .take(4)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                format!("…{tail}")
            }),
            ai_provider_key: matches!(id, EngineId::Openai | EngineId::Groq)
                && crate::settings::ai_provider_key(service.config_dir(), id.as_str()).is_some(),
            model: e.model.clone(),
            base_url: e.base_url.clone(),
        }
    };
    VoiceSettingsView {
        enabled: s.enabled,
        engine: s.engine,
        mode: s.mode,
        hotkey: s.hotkey.clone(),
        auto_submit_hold: s.auto_submit_hold,
        auto_submit_tap: s.auto_submit_tap,
        language: s.language.clone(),
        page_hint: s.page_hint,
        local: LocalSettingsView {
            model: s.local.model.clone(),
            threads: s.local.threads,
            idle_minutes: s.local.idle_minutes,
        },
        openai: engine(EngineId::Openai, &s.openai),
        groq: engine(EngineId::Groq, &s.groq),
        elevenlabs: engine(EngineId::Elevenlabs, &s.elevenlabs),
        deepgram: engine(EngineId::Deepgram, &s.deepgram),
        custom: engine(EngineId::Custom, &s.custom),
    }
}

// ------------------------------------------------------------------ voice.settings_get

pub struct SettingsGet;

impl Command for SettingsGet {
    const NAME: &'static str = "voice.settings_get";
    type Args = Empty;
    type Output = VoiceSettingsView;

    fn meta() -> Meta {
        Meta::new(
            "Read the voice dictation settings: engine (local Whisper, OpenAI, Groq, \
             ElevenLabs, Deepgram, custom), hold/tap mode, hotkey, auto-submit, language, page \
             hint, local model. API keys are never returned (only whether one is set).",
            Toolset::Ui,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<VoiceSettingsView, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        let settings = service.settings()?;
        Ok(view(&service, &settings))
    }
}

// ------------------------------------------------------------------ voice.settings_set

/// Changes to one cloud engine: `""` clears a field.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnginePatch {
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
}

/// Changes to Local Whisper.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalPatch {
    /// Model id; `""` = the best one installed.
    pub model: Option<String>,
    /// CPU threads; 0 = whisper-server's default.
    pub threads: Option<u32>,
    /// Stop the server after this many idle minutes (1–120).
    pub idle_minutes: Option<u32>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsSetArgs {
    pub enabled: Option<bool>,
    pub engine: Option<EngineId>,
    pub mode: Option<VoiceMode>,
    /// `Space`, `Ctrl+Space`, `Alt+V`, `F8`…
    pub hotkey: Option<String>,
    pub auto_submit_hold: Option<bool>,
    pub auto_submit_tap: Option<bool>,
    /// `""` = the UI language, `auto` = detect, or a code like `uk`.
    pub language: Option<String>,
    pub page_hint: Option<bool>,
    pub local: Option<LocalPatch>,
    pub openai: Option<EnginePatch>,
    pub groq: Option<EnginePatch>,
    pub elevenlabs: Option<EnginePatch>,
    pub deepgram: Option<EnginePatch>,
    pub custom: Option<EnginePatch>,
}

fn opt(value: String) -> Option<String> {
    let v = value.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// Validate `args` and apply them to `s`.
pub fn apply(s: &mut VoiceSettings, args: SettingsSetArgs) -> Result<(), CmdError> {
    if let Some(v) = args.enabled {
        s.enabled = v;
    }
    if let Some(v) = args.engine {
        s.engine = v;
    }
    if let Some(v) = args.mode {
        s.mode = v;
    }
    if let Some(v) = args.hotkey {
        s.hotkey = normalize_hotkey(&v)?;
    }
    if let Some(v) = args.auto_submit_hold {
        s.auto_submit_hold = v;
    }
    if let Some(v) = args.auto_submit_tap {
        s.auto_submit_tap = v;
    }
    if let Some(v) = args.language {
        s.language = normalize_language(&v)?;
    }
    if let Some(v) = args.page_hint {
        s.page_hint = v;
    }
    if let Some(local) = args.local {
        if let Some(model) = local.model {
            let model = opt(model);
            if let Some(m) = &model
                && catalog::model(m).is_none()
            {
                return Err(CmdError::invalid_params(format!(
                    "unknown Whisper model {m:?}"
                )));
            }
            s.local.model = model;
        }
        if let Some(t) = local.threads {
            s.local.threads = (t > 0).then_some(t.min(64));
        }
        if let Some(m) = local.idle_minutes {
            if !(1..=120).contains(&m) {
                return Err(CmdError::invalid_params("idle_minutes must be 1–120"));
            }
            s.local.idle_minutes = m;
        }
    }
    for (id, patch) in [
        (EngineId::Openai, args.openai),
        (EngineId::Groq, args.groq),
        (EngineId::Elevenlabs, args.elevenlabs),
        (EngineId::Deepgram, args.deepgram),
        (EngineId::Custom, args.custom),
    ] {
        let Some(patch) = patch else { continue };
        let e = s.engine_mut(id).expect("cloud engine");
        if let Some(k) = patch.api_key {
            e.api_key = opt(k);
        }
        if let Some(m) = patch.model {
            e.model = opt(m);
        }
        if let Some(u) = patch.base_url {
            let u = opt(u);
            if let Some(url) = &u
                && !(url.starts_with("http://") || url.starts_with("https://"))
            {
                return Err(CmdError::invalid_params(format!(
                    "base_url {url:?} must start with http:// or https://"
                )));
            }
            e.base_url = u.map(|u| u.trim_end_matches('/').to_string());
        }
    }
    Ok(())
}

pub struct SettingsSet;

impl Command for SettingsSet {
    const NAME: &'static str = "voice.settings_set";
    type Args = SettingsSetArgs;
    type Output = VoiceSettingsView;

    fn meta() -> Meta {
        Meta::new(
            "Change voice dictation settings (fields left out stay; \"\" clears a key, model \
             or address). Keys are stored as plain JSON in data/config/voice.json; OpenAI and \
             Groq fall back to the key of the AI provider of the same name. Returns the same as \
             voice.settings_get.",
            Toolset::Ui,
        )
        .secret()
    }

    async fn run(ctx: Ctx, args: SettingsSetArgs) -> Result<VoiceSettingsView, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        let _write = service.writes.lock().unwrap_or_else(|e| e.into_inner());
        let mut settings = service.settings()?;
        let model_before = settings.local.clone();
        apply(&mut settings, args)?;
        service.store(&settings)?;
        if settings.local != model_before || !settings.enabled {
            // The next transcription starts the server with the new model / threads.
            service.local.stop_server();
        }
        Ok(view(&service, &settings))
    }
}

// ------------------------------------------------------------------ voice.engines

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EngineInfo {
    pub id: EngineId,
    pub name: &'static str,
    /// Ready to transcribe (key set / model downloaded).
    pub configured: bool,
    /// Why not, when it is not.
    pub reason: Option<String>,
    pub needs_key: bool,
    pub key_source: Option<KeySource>,
    /// Text appears while you speak: Deepgram streams it; Local Whisper re-transcribes the
    /// growing recording. Batch engines insert the text when you stop.
    pub interim: bool,
    /// Models to pick from (empty: any name, or the local catalog).
    pub models: Vec<&'static str>,
    /// The model it uses now.
    pub model: Option<String>,
    /// Takes the page title / site as a vocabulary hint.
    pub page_hint: bool,
    /// Runs on this computer (no audio leaves it).
    pub offline: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EnginesOutput {
    /// The engine dictation uses.
    pub active: EngineId,
    pub engines: Vec<EngineInfo>,
}

pub struct Engines;

impl Command for Engines {
    const NAME: &'static str = "voice.engines";
    type Args = Empty;
    type Output = EnginesOutput;

    fn meta() -> Meta {
        Meta::new(
            "List the speech-to-text engines for voice dictation and whether each is ready \
             (key set, local model downloaded), streams text while you speak, and runs offline.",
            Toolset::Ui,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<EnginesOutput, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        let settings = service.settings()?;
        let engines = EngineId::ALL
            .into_iter()
            .map(|id| {
                let ready = service.ready(&settings, id);
                let model = if id == EngineId::Local {
                    service
                        .local
                        .active_model(&settings.local)
                        .map(|m| m.id.to_string())
                } else {
                    settings
                        .engine(id)
                        .and_then(|e| e.model.clone())
                        .or_else(|| id.models().first().map(|m| (*m).to_string()))
                };
                EngineInfo {
                    id,
                    name: id.name(),
                    configured: ready.is_ok(),
                    reason: ready.err().map(|e| e.message),
                    needs_key: id.needs_key(),
                    key_source: service.key_for(&settings, id).map(|k| k.1),
                    interim: matches!(id, EngineId::Local | EngineId::Deepgram),
                    models: id.models().to_vec(),
                    model,
                    page_hint: matches!(
                        id,
                        EngineId::Local | EngineId::Openai | EngineId::Groq | EngineId::Custom
                    ),
                    offline: id == EngineId::Local,
                }
            })
            .collect();
        Ok(EnginesOutput {
            active: settings.engine,
            engines,
        })
    }
}

// ------------------------------------------------------------------ voice.transcribe

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscribeArgs {
    /// The recording, base64. WAV 16 kHz mono works with every engine; cloud engines also
    /// take webm/ogg/mp3/m4a.
    pub audio_base64: String,
    /// MIME type of the audio (default `audio/wav`).
    pub mime: Option<String>,
    /// Language code (`uk`, `en`) or `auto`; default: the voice settings.
    pub language: Option<String>,
    /// Vocabulary hint (e.g. the page title and site) for engines that take one.
    pub prompt: Option<String>,
    /// Engine to use instead of the one in the settings.
    pub engine: Option<EngineId>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TranscribeOutput {
    pub text: String,
    /// Language the engine detected or used, when it says.
    pub language: Option<String>,
    /// Length of the audio.
    pub duration_ms: u64,
    pub engine: EngineId,
    /// How long the engine took.
    pub elapsed_ms: u64,
}

/// Request options from the arguments and the settings.
fn options(
    settings: &VoiceSettings,
    language: Option<String>,
    prompt: Option<String>,
) -> Result<Options, CmdError> {
    let language = match language {
        Some(l) => normalize_language(&l)?,
        None => settings.language.clone(),
    };
    Ok(Options {
        language: (!language.is_empty() && language != "auto").then_some(language),
        prompt: prompt
            .map(|p| p.trim().chars().take(400).collect::<String>())
            .filter(|p| !p.is_empty()),
    })
}

fn output(t: Transcript, engine: EngineId, fallback_ms: u64, started: Instant) -> TranscribeOutput {
    TranscribeOutput {
        text: t.text,
        language: t.language,
        duration_ms: t.duration_ms.unwrap_or(fallback_ms),
        engine,
        elapsed_ms: started.elapsed().as_millis() as u64,
    }
}

fn decode(b64: &str, what: &str) -> Result<Vec<u8>, CmdError> {
    base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|err| CmdError::invalid_params(format!("{what} is not base64: {err}")))
}

pub struct Transcribe;

impl Command for Transcribe {
    const NAME: &'static str = "voice.transcribe";
    type Args = TranscribeArgs;
    type Output = TranscribeOutput;

    fn meta() -> Meta {
        Meta::new(
            "Turn a recording into text with the voice engine (local Whisper by default; cloud \
             engines send the audio to that provider). Returns the text, the language and the \
             audio length.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: TranscribeArgs) -> Result<TranscribeOutput, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        let settings = service.settings()?;
        let engine = args.engine.unwrap_or(settings.engine);
        let bytes = decode(&args.audio_base64, "audio_base64")?;
        if bytes.is_empty() {
            return Err(CmdError::invalid_params("audio_base64 is empty"));
        }
        let mime = args.mime.unwrap_or_else(|| "audio/wav".into());
        let opts = options(&settings, args.language, args.prompt)?;
        let fallback_ms = wav::parse(&bytes).map_or(0, |i| i.duration_ms());
        let started = Instant::now();
        let t = service
            .transcribe(
                &settings,
                engine,
                Audio {
                    bytes: &bytes,
                    mime: &mime,
                },
                &opts,
            )
            .await?;
        Ok(output(t, engine, fallback_ms, started))
    }
}

// ------------------------------------------------------------------ voice.stream_*

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreamStartArgs {
    /// Language code or `auto`; default: the voice settings.
    pub language: Option<String>,
    /// Vocabulary hint (page title and site).
    pub prompt: Option<String>,
    /// Engine to use instead of the one in the settings.
    pub engine: Option<EngineId>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StreamStartOutput {
    /// Id for voice.stream_push / voice.stream_stop.
    pub stream: String,
    pub engine: EngineId,
    /// `voice.stream_push` returns text while you speak (else only at stop).
    pub interim: bool,
    /// A live socket streams the audio (Deepgram).
    pub live: bool,
}

pub struct StreamStart;

impl Command for StreamStart {
    const NAME: &'static str = "voice.stream_start";
    type Args = StreamStartArgs;
    type Output = StreamStartOutput;

    fn meta() -> Meta {
        Meta::new(
            "Start a dictation stream: push 16 kHz mono 16-bit PCM with voice.stream_push \
             (returns the text so far) and finish with voice.stream_stop (the final text). \
             Deepgram streams live; Local Whisper re-transcribes the growing audio every ~1.5 s; \
             other engines transcribe at stop.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: StreamStartArgs) -> Result<StreamStartOutput, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        let settings = service.settings()?;
        if !settings.enabled {
            return Err(CmdError::invalid_params("voice dictation is turned off")
                .with_hint("Turn it on in Settings → Voice."));
        }
        let engine = args.engine.unwrap_or(settings.engine);
        let opts = options(&settings, args.language, args.prompt)?;
        let stream = service.stream_start(&settings, engine, opts).await?;
        Ok(StreamStartOutput {
            stream: stream.id.clone(),
            engine,
            interim: stream.interim(),
            live: stream.live(),
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreamPushArgs {
    pub stream: String,
    /// 16-bit little-endian mono PCM at 16 kHz, base64.
    pub pcm_base64: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct StreamPushOutput {
    /// Text heard so far (not final; empty for batch engines).
    pub partial: String,
    /// Audio received so far.
    pub duration_ms: u64,
}

pub struct StreamPush;

impl Command for StreamPush {
    const NAME: &'static str = "voice.stream_push";
    type Args = StreamPushArgs;
    type Output = StreamPushOutput;

    fn meta() -> Meta {
        Meta::new(
            "Add audio to a dictation stream (16 kHz mono 16-bit PCM, base64). Returns the \
             interim text so far.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: StreamPushArgs) -> Result<StreamPushOutput, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        let pcm = decode(&args.pcm_base64, "pcm_base64")?;
        let (partial, duration_ms) = service.stream_push(&args.stream, &pcm)?;
        Ok(StreamPushOutput {
            partial,
            duration_ms,
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StreamStopArgs {
    pub stream: String,
    /// Discard the recording (Esc): no engine call, empty text.
    #[serde(default)]
    pub cancel: bool,
}

pub struct StreamStop;

impl Command for StreamStop {
    const NAME: &'static str = "voice.stream_stop";
    type Args = StreamStopArgs;
    type Output = TranscribeOutput;

    fn meta() -> Meta {
        Meta::new(
            "Finish a dictation stream and return the final text (cancel: true discards it).",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: StreamStopArgs) -> Result<TranscribeOutput, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        let started = Instant::now();
        let (t, engine) = service.stream_stop(&args.stream, args.cancel).await?;
        Ok(output(t, engine, 0, started))
    }
}

// ------------------------------------------------------------------ voice.local_*

pub struct LocalStatusCmd;

impl Command for LocalStatusCmd {
    const NAME: &'static str = "voice.local_status";
    type Args = Empty;
    type Output = LocalStatus;

    fn meta() -> Meta {
        Meta::new(
            "Local Whisper: whether the whisper.cpp engine and each model (with sizes) are \
             downloaded, the download in progress, and the running whisper-server.",
            Toolset::Ui,
        )
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<LocalStatus, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        let settings = service.settings()?;
        Ok(service.local.status(&settings.local))
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocalWhatArgs {
    /// `engine` (whisper.cpp from GitHub) or a model id (`base`, `small`,
    /// `large-v3-turbo-q5_0`, …).
    pub what: String,
}

pub struct LocalDownload;

impl Command for LocalDownload {
    const NAME: &'static str = "voice.local_download";
    type Args = LocalWhatArgs;
    type Output = LocalStatus;

    fn meta() -> Meta {
        Meta::new(
            "Download the whisper.cpp engine (official GitHub release) or a Whisper model \
             (official Hugging Face repository) into data/voice, checked by size and SHA-256. \
             Runs in the background: poll voice.local_status for progress.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: LocalWhatArgs) -> Result<LocalStatus, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        service.local.start_download(args.what.trim())?;
        let settings = service.settings()?;
        Ok(service.local.status(&settings.local))
    }
}

pub struct LocalCancel;

impl Command for LocalCancel {
    const NAME: &'static str = "voice.local_cancel";
    type Args = Empty;
    type Output = LocalStatus;

    fn meta() -> Meta {
        Meta::new("Cancel the running Local Whisper download.", Toolset::Ui).act()
    }

    async fn run(ctx: Ctx, _args: Empty) -> Result<LocalStatus, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        service.local.cancel_download();
        let settings = service.settings()?;
        Ok(service.local.status(&settings.local))
    }
}

pub struct LocalDelete;

impl Command for LocalDelete {
    const NAME: &'static str = "voice.local_delete";
    type Args = LocalWhatArgs;
    type Output = LocalStatus;

    fn meta() -> Meta {
        Meta::new(
            "Delete the downloaded whisper.cpp engine (`engine`) or a model (its id) from \
             data/voice.",
            Toolset::Ui,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: LocalWhatArgs) -> Result<LocalStatus, CmdError> {
        let service = VoiceService::from_ctx(&ctx)?;
        service.local.delete(args.what.trim())?;
        let settings = service.settings()?;
        Ok(service.local.status(&settings.local))
    }
}
