//! `record.settings`: the video defaults (`data/config/recording.json`), where ffmpeg was
//! found, and the recordings / videos folders (Settings → Recording & scripts).
//!
//! The defaults apply to `record.video_start` arguments that are left out and to the video of
//! `record.start {video: true}`; explicit arguments still win.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use bmcp_core::{CmdError, Command, Ctx, DataDir, Meta, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::store::{self, data_dir};
use crate::video::{self, Format, VideoOptions};

/// File name of the recording settings in `data/config/`.
pub const RECORDING_FILE: &str = "recording.json";
/// How to install ffmpeg on Windows (also in `video::ffmpeg_missing`).
pub const FFMPEG_INSTALL: &str = "winget install Gyan.FFmpeg";

/// `data/config/recording.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordingSettings {
    /// Frames per second of videos, 1-60 (default 10). More = smoother and bigger files.
    #[serde(default = "default_fps")]
    pub video_fps: u32,
    /// webm (VP9, default) or mp4 (H.264, plays everywhere).
    #[serde(default)]
    pub video_format: Format,
    /// 1-100 (default 80): JPEG quality of the frames and encoder quality.
    #[serde(default = "default_quality")]
    pub video_quality: u8,
}

fn default_fps() -> u32 {
    VideoOptions::default().fps
}

fn default_quality() -> u8 {
    VideoOptions::default().quality
}

impl Default for RecordingSettings {
    fn default() -> Self {
        let v = VideoOptions::default();
        RecordingSettings {
            video_fps: v.fps,
            video_format: v.format,
            video_quality: v.quality,
        }
    }
}

impl RecordingSettings {
    pub fn path(data: &DataDir) -> PathBuf {
        data.config().join(RECORDING_FILE)
    }

    /// The stored settings (defaults when there is no file).
    pub fn load(data: &DataDir) -> Result<Self, CmdError> {
        bmcp_core::config_file::load(&Self::path(data))
    }

    /// The video options of a video started without explicit values.
    pub fn video_options(&self) -> VideoOptions {
        VideoOptions {
            fps: self.video_fps,
            format: self.video_format,
            quality: self.video_quality,
        }
    }

    fn check(&self) -> Result<(), CmdError> {
        if !(1..=60).contains(&self.video_fps) {
            return Err(CmdError::invalid_params(
                "video_fps must be between 1 and 60",
            ));
        }
        if !(1..=100).contains(&self.video_quality) {
            return Err(CmdError::invalid_params(
                "video_quality must be between 1 and 100",
            ));
        }
        Ok(())
    }
}

/// The settings for this call: the stored ones, or the defaults when the file cannot be read
/// (a broken file must not stop recordings; `record.settings` reports the error).
pub fn current(ctx: &Ctx) -> RecordingSettings {
    data_dir(ctx)
        .and_then(|data| RecordingSettings::load(&data))
        .unwrap_or_else(|err| {
            tracing::warn!(%err, "recording.json ignored");
            RecordingSettings::default()
        })
}

// ---------------------------------------------------------------------------------------------
// ffmpeg status
// ---------------------------------------------------------------------------------------------

/// Where ffmpeg was found (the order [`video::locate_ffmpeg`] looks in).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FfmpegSource {
    /// The `BMCP_FFMPEG` environment variable.
    Env,
    /// `ffmpeg.exe` next to browsermcp.exe.
    NextToExe,
    /// A folder on `PATH` (e.g. installed with winget).
    Path,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FfmpegStatus {
    pub found: bool,
    /// The program that will be run.
    pub path: Option<String>,
    pub source: Option<FfmpegSource>,
    /// First line of `ffmpeg -version`, e.g. "ffmpeg version 7.1-full_build-www.gyan.dev".
    pub version: Option<String>,
    /// The value of `BMCP_FFMPEG`, when set (even when it points nowhere).
    pub env: Option<String>,
    /// The folder to put ffmpeg.exe in (next to browsermcp.exe).
    pub exe_dir: Option<String>,
    /// The install command for Windows.
    pub install: String,
}

/// First line of `ffmpeg -version` (3 s at most: a hung program must not block Settings).
async fn ffmpeg_version(ffmpeg: &Path) -> Option<String> {
    let run = tokio::process::Command::new(ffmpeg)
        .arg("-version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(Duration::from_secs(3), run)
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

pub async fn ffmpeg_status() -> FfmpegStatus {
    let found = video::locate_ffmpeg();
    let version = match &found {
        Some((path, _)) => ffmpeg_version(path).await,
        None => None,
    };
    FfmpegStatus {
        found: found.is_some(),
        path: found.as_ref().map(|(p, _)| p.display().to_string()),
        source: found.map(|(_, s)| s),
        version,
        env: std::env::var_os("BMCP_FFMPEG").map(|v| v.to_string_lossy().into_owned()),
        exe_dir: std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.display().to_string())),
        install: FFMPEG_INSTALL.into(),
    }
}

// ---------------------------------------------------------------------------------------------
// record.settings
// ---------------------------------------------------------------------------------------------

/// `record.settings`.
pub struct Settings;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsArgs {
    /// Back to the defaults first (the other fields then apply on top).
    pub reset: Option<bool>,
    /// Default frames per second of videos, 1-60.
    pub video_fps: Option<u32>,
    /// Default format: webm or mp4.
    pub video_format: Option<Format>,
    /// Default quality, 1-100.
    pub video_quality: Option<u8>,
}

impl SettingsArgs {
    fn changes(&self) -> bool {
        self.reset.unwrap_or(false)
            || self.video_fps.is_some()
            || self.video_format.is_some()
            || self.video_quality.is_some()
    }

    fn apply(&self, mut settings: RecordingSettings) -> RecordingSettings {
        if self.reset.unwrap_or(false) {
            settings = RecordingSettings::default();
        }
        if let Some(fps) = self.video_fps {
            settings.video_fps = fps;
        }
        if let Some(format) = self.video_format {
            settings.video_format = format;
        }
        if let Some(quality) = self.video_quality {
            settings.video_quality = quality;
        }
        settings
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SettingsOutput {
    /// The settings in force (saved in data/config/recording.json).
    pub settings: RecordingSettings,
    /// The built-in defaults.
    pub defaults: RecordingSettings,
    pub ffmpeg: FfmpegStatus,
    /// `data/recordings` (one folder per recording).
    pub recordings_dir: String,
    /// `data/videos` (record_video_start without a recording).
    pub videos_dir: String,
}

impl Command for Settings {
    const NAME: &'static str = "record.settings";
    type Args = SettingsArgs;
    type Output = SettingsOutput;

    fn meta() -> Meta {
        Meta::new(
            "Read or change the video defaults (fps, format, quality; saved in \
             data/config/recording.json; used when record_video_start / record_start \
             video=true leave them out). Also reports where ffmpeg was found (BMCP_FFMPEG, \
             next to browsermcp.exe, PATH) and the recordings / videos folders. No arguments = \
             just read.",
            Toolset::Record,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: SettingsArgs) -> Result<SettingsOutput, CmdError> {
        let data = data_dir(&ctx)?;
        let mut settings = RecordingSettings::load(&data)?;
        if args.changes() {
            settings = args.apply(settings);
            settings.check()?;
            bmcp_core::config_file::save(&RecordingSettings::path(&data), &settings)?;
        }
        Ok(SettingsOutput {
            settings,
            defaults: RecordingSettings::default(),
            ffmpeg: ffmpeg_status().await,
            recordings_dir: store::recordings_dir(&data).display().to_string(),
            videos_dir: store::videos_dir(&data).display().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_patch_reset_and_validation() {
        let stored = RecordingSettings {
            video_fps: 30,
            video_format: Format::Mp4,
            video_quality: 50,
        };
        let only_fps = SettingsArgs {
            video_fps: Some(15),
            ..Default::default()
        };
        assert_eq!(
            only_fps.apply(stored),
            RecordingSettings {
                video_fps: 15,
                ..stored
            }
        );
        let reset = SettingsArgs {
            reset: Some(true),
            video_quality: Some(90),
            ..Default::default()
        };
        let after = reset.apply(stored);
        assert_eq!(after.video_fps, 10);
        assert_eq!(after.video_format, Format::Webm);
        assert_eq!(after.video_quality, 90);
        assert!(!SettingsArgs::default().changes());
        let bad = RecordingSettings {
            video_fps: 0,
            ..Default::default()
        };
        assert!(bad.check().is_err());
        let bad = RecordingSettings {
            video_quality: 101,
            ..Default::default()
        };
        assert!(bad.check().is_err());
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::from_root(dir.path());
        std::fs::create_dir_all(data.config()).unwrap();
        std::fs::write(RecordingSettings::path(&data), r#"{"video_format":"mp4"}"#).unwrap();
        let s = RecordingSettings::load(&data).unwrap();
        assert_eq!(s.video_format, Format::Mp4);
        assert_eq!(s.video_fps, 10);
        assert_eq!(s.video_options().quality, 80);
    }
}
