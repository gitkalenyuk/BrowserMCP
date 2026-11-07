//! Voice dictation (like Claude Code's `/voice`): speech-to-text for the browser's prompts.
//!
//! The UI records the microphone (16 kHz mono PCM) and either sends a whole recording
//! (`voice.transcribe`) or streams it while the user speaks:
//!
//! ```text
//! voice.stream_start {language?, prompt?}      → {stream, interim, live}
//! voice.stream_push  {stream, pcm_base64} …    → {partial}      (every ~250 ms)
//! voice.stream_stop  {stream, cancel?}         → {text, language, duration_ms}
//! ```
//!
//! Engines ([`settings::EngineId`]):
//! * **Local Whisper** — whisper.cpp's `whisper-server` as a sidecar on a free localhost port
//!   (no libclang / C++ build needed), downloaded on demand into `data/voice` with its models
//!   ([`local`]). Interim text: the growing recording is transcribed again every ~1.5 s.
//! * **OpenAI**, **Groq**, **custom** — OpenAI-compatible `/audio/transcriptions`; OpenAI and
//!   Groq reuse the key of the AI provider of the same name when voice has none.
//! * **ElevenLabs Scribe** — `/v1/speech-to-text`.
//! * **Deepgram** — nova-3 over a WebSocket: true live text ([`deepgram`]).
//!
//! Paid batch engines get the audio once, at stop (no interim re-transcription: it would
//! multiply the cost). Settings: `data/config/voice.json` ([`settings`]); keys are plain
//! JSON like the AI provider keys.

pub mod catalog;
pub mod commands;
pub mod deepgram;
pub mod engines;
pub mod local;
pub mod multipart;
pub mod service;
pub mod settings;
#[cfg(test)]
mod tests;
pub mod wav;

use std::path::Path;
use std::sync::Arc;

use bmcp_core::Registry;

pub use service::VoiceService;
pub use settings::{EngineId, VoiceMode, VoiceSettings};

/// Register the `voice.*` commands.
pub fn register(registry: &mut Registry) {
    registry
        .add::<commands::SettingsGet>()
        .add::<commands::SettingsSet>()
        .add::<commands::Engines>()
        .add::<commands::Transcribe>()
        .add::<commands::StreamStart>()
        .add::<commands::StreamPush>()
        .add::<commands::StreamStop>()
        .add::<commands::LocalStatusCmd>()
        .add::<commands::LocalDownload>()
        .add::<commands::LocalCancel>()
        .add::<commands::LocalDelete>();
}

/// Put the service (data folder `data_root`) into the registry's extensions. The app calls
/// [`VoiceService::shutdown`] when the browser quits (stops whisper-server).
pub fn install(registry: &Registry, data_root: &Path) -> Arc<VoiceService> {
    registry.extensions().insert(VoiceService::new(data_root))
}
