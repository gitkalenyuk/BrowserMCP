//! Video of a tab (spike S6): CDP screencast → `ffmpeg` sidecar.
//!
//! ```text
//! Page.startScreencast {format: jpeg, quality, everyNthFrame: 1}
//!   ← Page.screencastFrame {data: base64 JPEG, sessionId}   (only while the page paints)
//!   → Page.screencastFrameAck {sessionId}                    (else Chromium stops sending)
//! latest frame ──every 1/fps s──▶ ffmpeg stdin  (-f image2pipe -c:v mjpeg -framerate fps -i -)
//! ```
//!
//! Chromium sends frames only when something changes on screen, so a still page sends none.
//! We keep the newest frame and write it on a fixed clock: a still page repeats its last
//! frame and the file has a constant frame rate without timestamps.
//!
//! The encoder starts with the first frame, because its output size comes from that frame
//! (later frames of another size, e.g. after a window resize, are scaled and padded to it).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use bmcp_core::{BackendError, BrowserBackend, CmdError, ErrorCode, TabId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{broadcast::error::RecvError, oneshot};

/// Container / codec family of the output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// VP9 in WebM (`libvpx-vp9`).
    #[default]
    Webm,
    /// H.264 in MP4 (`libx264`, else `h264_mf` on Windows).
    Mp4,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Format::Webm => "webm",
            Format::Mp4 => "mp4",
        }
    }
}

/// Settings of one video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoOptions {
    /// Output frames per second (1–60).
    pub fps: u32,
    pub format: Format,
    /// JPEG quality of the screencast and target quality of the encoder, 1–100.
    pub quality: u8,
}

impl Default for VideoOptions {
    fn default() -> Self {
        VideoOptions {
            fps: 10,
            format: Format::Webm,
            quality: 80,
        }
    }
}

/// The encoder process to start: `program args…`, frames go to its stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoder {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Extra environment variables for the process.
    pub env: Vec<(String, String)>,
    /// Codec name for reports (`libvpx-vp9`).
    pub codec: String,
}

/// Builds the encoder once the frame size (width, height) is known.
pub type MakeEncoder = Box<dyn FnOnce(u32, u32) -> Result<Encoder, String> + Send>;

/// What a finished video looks like.
#[derive(Debug, Clone, Default, PartialEq, Serialize, JsonSchema)]
pub struct VideoReport {
    pub path: String,
    pub codec: String,
    /// Output size (even numbers, as encoders need).
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Screencast frames from Chromium (each one acknowledged).
    pub frames_received: u64,
    /// Frames written to the encoder (constant rate: repeats included).
    pub frames_written: u64,
    /// Length of the video (`frames_written / fps`).
    pub seconds: f64,
    /// Real time between the first frame and stop.
    pub wall_seconds: f64,
    /// File size in bytes (0 when it was not written).
    pub bytes: u64,
    /// Why the video failed or is incomplete (encoder error text, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A running video. Call [`VideoRecorder::stop`] to finish the file.
pub struct VideoRecorder {
    pub path: PathBuf,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<VideoReport>,
}

impl VideoRecorder {
    /// Subscribe to the tab's CDP events, start the screencast and the frame task.
    pub async fn start(
        backend: Arc<dyn BrowserBackend>,
        tab: TabId,
        options: VideoOptions,
        path: PathBuf,
        make_encoder: MakeEncoder,
    ) -> Result<VideoRecorder, BackendError> {
        let events = backend.cdp_events(tab).await?;
        backend
            .cdp(
                tab,
                "Page.startScreencast".into(),
                json!({ "format": "jpeg", "quality": options.quality, "everyNthFrame": 1 }),
            )
            .await?;
        let (stop, stopped) = oneshot::channel();
        let job = FrameJob {
            backend,
            tab,
            options,
            path: path.clone(),
            make_encoder: Some(make_encoder),
        };
        let task = tokio::spawn(job.run(events, stopped));
        Ok(VideoRecorder {
            path,
            stop: Some(stop),
            task,
        })
    }

    /// Stop the screencast, let the encoder finish the file and report.
    pub async fn stop(mut self) -> VideoReport {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        match self.task.await {
            Ok(report) => report,
            Err(err) => VideoReport {
                path: self.path.display().to_string(),
                error: Some(format!("video task failed: {err}")),
                ..VideoReport::default()
            },
        }
    }
}

/// The running encoder process.
struct Running {
    child: Child,
    stdin: ChildStdin,
    /// Collects the encoder's stderr (its error messages).
    stderr: tokio::task::JoinHandle<String>,
}

struct FrameJob {
    backend: Arc<dyn BrowserBackend>,
    tab: TabId,
    options: VideoOptions,
    path: PathBuf,
    make_encoder: Option<MakeEncoder>,
}

impl FrameJob {
    async fn run(
        mut self,
        mut events: bmcp_core::CdpEvents,
        mut stopped: oneshot::Receiver<()>,
    ) -> VideoReport {
        let fps = self.options.fps.clamp(1, 60);
        let mut report = VideoReport {
            path: self.path.display().to_string(),
            fps,
            ..VideoReport::default()
        };
        let mut latest: Option<Vec<u8>> = None;
        let mut running: Option<Running> = None;
        // Screencast frames not acknowledged yet (acked on the next clock tick).
        let mut unacked: Vec<i64> = Vec::new();
        let mut first_frame: Option<Instant> = None;
        let mut clock = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(fps)));
        // Late ticks are caught up (written back to back), keeping the rate constant.
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

        // A page that does not repaint sends no screencast frame, so the video would only
        // start at the next change. A screenshot gives the first frame right away.
        let shot = self
            .backend
            .cdp(
                self.tab,
                "Page.captureScreenshot".into(),
                json!({ "format": "jpeg", "quality": self.options.quality }),
            )
            .await;
        if let Some(frame) = shot.ok().and_then(|r| {
            let data = r["data"].as_str()?.to_string();
            base64::engine::general_purpose::STANDARD.decode(data).ok()
        }) && let Err(err) = self
            .accept(
                frame,
                &mut running,
                &mut latest,
                &mut first_frame,
                &mut clock,
                &mut report,
            )
            .await
        {
            report.error = Some(err);
        }

        while report.error.is_none() {
            tokio::select! {
                _ = &mut stopped => break,
                event = events.recv() => match event {
                    Ok(event) if event.method == "Page.screencastFrame" => {
                        // Chromium sends the next frame only after the ack. Acking on our
                        // clock tick (not at once) keeps it near our fps instead of the
                        // display's 60–100 Hz, which saves its JPEG work (measured in S6).
                        if let Some(session) = event.params["sessionId"].as_i64() {
                            if running.is_some() {
                                unacked.push(session);
                            } else {
                                ack(&self.backend, self.tab, session).await;
                            }
                        }
                        report.frames_received += 1;
                        let data = event.params["data"].as_str().unwrap_or_default();
                        let Ok(frame) = base64::engine::general_purpose::STANDARD.decode(data) else {
                            continue;
                        };
                        let accepted = self
                            .accept(frame, &mut running, &mut latest, &mut first_frame, &mut clock, &mut report)
                            .await;
                        if let Err(err) = accepted {
                            report.error = Some(err);
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(RecvError::Lagged(_)) => {}
                    // The tab closed.
                    Err(RecvError::Closed) => break,
                },
                _ = clock.tick(), if running.is_some() => {
                    for session in unacked.drain(..) {
                        ack(&self.backend, self.tab, session).await;
                    }
                    if let Err(err) = write_frame(&mut running, &latest, &mut report).await {
                        report.error = Some(err);
                        break;
                    }
                }
            }
        }

        for session in unacked.drain(..) {
            ack(&self.backend, self.tab, session).await;
        }
        let _ = self
            .backend
            .cdp(self.tab, "Page.stopScreencast".into(), json!({}))
            .await;
        report.wall_seconds = first_frame.map_or(0.0, |t| t.elapsed().as_secs_f64());
        // A busy machine can fall behind the clock; pad with the last frame so the video is as
        // long as the recording was (at most 10 s of padding).
        if report.error.is_none() {
            let due = (report.wall_seconds * f64::from(fps)).floor() as u64 + 1;
            let missing = due
                .saturating_sub(report.frames_written)
                .min(u64::from(fps) * 10);
            for _ in 0..missing {
                if let Err(err) = write_frame(&mut running, &latest, &mut report).await {
                    report.error = Some(err);
                    break;
                }
            }
        }
        report.seconds = report.frames_written as f64 / f64::from(fps);
        match running {
            Some(running) => finish_encoder(running, &mut report).await,
            None if report.error.is_none() => {
                report.error = Some("no screencast frame arrived (is the tab visible?)".into());
            }
            None => {}
        }
        report.bytes = std::fs::metadata(&self.path).map_or(0, |m| m.len());
        report
    }

    /// A new frame: the first one starts the encoder and the clock, later ones replace the
    /// frame the clock repeats.
    async fn accept(
        &mut self,
        frame: Vec<u8>,
        running: &mut Option<Running>,
        latest: &mut Option<Vec<u8>>,
        first_frame: &mut Option<Instant>,
        clock: &mut tokio::time::Interval,
        report: &mut VideoReport,
    ) -> Result<(), String> {
        if running.is_some() {
            *latest = Some(frame);
            return Ok(());
        }
        *running = Some(self.spawn_encoder(&frame, report)?);
        *first_frame = Some(Instant::now());
        clock.reset();
        *latest = Some(frame);
        write_frame(running, latest, report).await
    }

    fn spawn_encoder(&mut self, frame: &[u8], report: &mut VideoReport) -> Result<Running, String> {
        let (width, height) = jpeg_size(frame).ok_or("the first screencast frame is not a JPEG")?;
        // yuv420p encoders need even sizes.
        let (width, height) = ((width & !1).max(2), (height & !1).max(2));
        let make = self.make_encoder.take().ok_or("encoder already started")?;
        let encoder = make(width, height)?;
        report.codec = encoder.codec.clone();
        report.width = width;
        report.height = height;
        let mut child = tokio::process::Command::new(&encoder.program)
            .args(&encoder.args)
            .envs(encoder.env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", encoder.program.display()))?;
        let stdin = child.stdin.take().ok_or("encoder has no stdin")?;
        let mut stderr = child.stderr.take().ok_or("encoder has no stderr")?;
        let stderr = tokio::spawn(async move {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text).await;
            text
        });
        Ok(Running {
            child,
            stdin,
            stderr,
        })
    }
}

/// Acknowledge a screencast frame (Chromium then sends the next one).
async fn ack(backend: &Arc<dyn BrowserBackend>, tab: TabId, session: i64) {
    let _ = backend
        .cdp(
            tab,
            "Page.screencastFrameAck".into(),
            json!({ "sessionId": session }),
        )
        .await;
}

/// Write the newest frame once more.
async fn write_frame(
    running: &mut Option<Running>,
    latest: &Option<Vec<u8>>,
    report: &mut VideoReport,
) -> Result<(), String> {
    let (Some(running), Some(frame)) = (running.as_mut(), latest.as_ref()) else {
        return Ok(());
    };
    running
        .stdin
        .write_all(frame)
        .await
        .map_err(|e| format!("encoder stopped taking frames: {e}"))?;
    report.frames_written += 1;
    Ok(())
}

/// Close the encoder's stdin (end of input) and wait for it to finish the file.
async fn finish_encoder(running: Running, report: &mut VideoReport) {
    let Running {
        mut child,
        stdin,
        stderr,
    } = running;
    drop(stdin);
    match tokio::time::timeout(Duration::from_secs(60), child.wait()).await {
        Ok(Ok(status)) if status.success() => {}
        Ok(Ok(status)) => {
            let text = stderr.await.unwrap_or_default();
            report.error = Some(format!("encoder exited with {status}: {}", text.trim()));
        }
        Ok(Err(err)) => report.error = Some(format!("encoder: {err}")),
        Err(_) => {
            let _ = child.kill().await;
            report.error = Some("encoder did not finish within 60 s".into());
        }
    }
}

/// Width and height from a JPEG's start-of-frame marker.
pub fn jpeg_size(data: &[u8]) -> Option<(u32, u32)> {
    if data.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // Fill bytes and markers without a length.
        if marker == 0xFF || marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            i += if marker == 0xFF { 1 } else { 2 };
            continue;
        }
        let len = usize::from(u16::from_be_bytes([data[i + 2], data[i + 3]]));
        let is_sof = (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker);
        if is_sof {
            let h = u16::from_be_bytes([*data.get(i + 5)?, *data.get(i + 6)?]);
            let w = u16::from_be_bytes([*data.get(i + 7)?, *data.get(i + 8)?]);
            return Some((u32::from(w), u32::from(h)));
        }
        i += 2 + len;
    }
    None
}

// ---------------------------------------------------------------- ffmpeg

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// `BMCP_FFMPEG`, then `ffmpeg(.exe)` next to the browser executable, then `PATH`.
pub fn find_ffmpeg() -> Option<PathBuf> {
    locate_ffmpeg().map(|(path, _)| path)
}

/// [`find_ffmpeg`] plus where it was found (shown in Settings → Recording & scripts).
pub fn locate_ffmpeg() -> Option<(PathBuf, crate::settings::FfmpegSource)> {
    use crate::settings::FfmpegSource;
    if let Some(path) = std::env::var_os("BMCP_FFMPEG").map(PathBuf::from)
        && path.is_file()
    {
        return Some((path, FfmpegSource::Env));
    }
    if let Ok(exe) = std::env::current_exe() {
        let next_to_exe = exe.with_file_name(exe_name());
        if next_to_exe.is_file() {
            return Some((next_to_exe, FfmpegSource::NextToExe));
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe_name()))
        .find(|p| p.is_file())
        .map(|p| (resolve_shim(&p).unwrap_or(p), FfmpegSource::Path))
}

/// Scoop installs `ffmpeg.exe` as a small launcher ("shim") with an `ffmpeg.shim` file next to
/// it (`path = "C:\…\ffmpeg.exe"`). Run the real program, so it is our direct child (killing
/// it works, CPU shows up under the right process).
fn resolve_shim(exe: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(exe.with_extension("shim")).ok()?;
    let line = text.lines().find(|l| l.trim_start().starts_with("path"))?;
    let target = line.split_once('=')?.1.trim().trim_matches('"');
    let target = PathBuf::from(target);
    target.is_file().then_some(target)
}

/// The error when there is no ffmpeg.
pub fn ffmpeg_missing() -> CmdError {
    CmdError::new(
        ErrorCode::NotFound,
        "video needs ffmpeg, which was not found (BMCP_FFMPEG, next to browsermcp.exe, or PATH)",
    )
    .with_hint(
        "Install it with `winget install Gyan.FFmpeg` (or put ffmpeg.exe next to \
         browsermcp.exe), then restart the browser so it sees the new PATH.",
    )
}

/// The encoder names ffmpeg was built with (`ffmpeg -hide_banner -encoders`).
pub async fn ffmpeg_encoders(ffmpeg: &Path) -> String {
    let output = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-encoders"])
        .stdin(Stdio::null())
        .output()
        .await;
    output
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// Does the `-encoders` listing contain this encoder name (as a whole word)?
fn has_encoder(listing: &str, name: &str) -> bool {
    listing
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some(name))
}

/// ffmpeg arguments for `width`×`height` frames at `options`, given the encoder listing.
pub fn ffmpeg_args(
    options: VideoOptions,
    width: u32,
    height: u32,
    encoders: &str,
    out: &Path,
) -> Result<(String, Vec<String>), String> {
    let fps = options.fps.clamp(1, 60).to_string();
    let quality = f64::from(options.quality.clamp(1, 100));
    let (codec, codec_args): (&str, Vec<String>) = match options.format {
        Format::Webm => {
            if !has_encoder(encoders, "libvpx-vp9") {
                return Err("this ffmpeg has no libvpx-vp9 encoder; use format mp4".into());
            }
            let crf = (15.0 + (100.0 - quality) * 0.4).round() as u32;
            let args = [
                "-c:v",
                "libvpx-vp9",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
                "-row-mt",
                "1",
                "-b:v",
                "0",
                "-crf",
            ];
            let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            args.push(crf.to_string());
            ("libvpx-vp9", args)
        }
        Format::Mp4 if has_encoder(encoders, "libx264") => {
            let crf = (18.0 + (100.0 - quality) * 0.25).round() as u32;
            let mut args: Vec<String> = ["-c:v", "libx264", "-preset", "veryfast", "-crf"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            args.push(crf.to_string());
            ("libx264", args)
        }
        Format::Mp4 if has_encoder(encoders, "h264_mf") => {
            let kbps = (1000.0 + quality * 40.0).round() as u32;
            let args = vec![
                "-c:v".to_string(),
                "h264_mf".to_string(),
                "-b:v".to_string(),
                format!("{kbps}k"),
            ];
            ("h264_mf", args)
        }
        Format::Mp4 => return Err("this ffmpeg has neither libx264 nor h264_mf".into()),
    };
    let filter = format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease,\
         pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,format=yuv420p"
    );
    let mut args: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-f",
        "image2pipe",
        "-framerate",
        &fps,
        "-c:v",
        "mjpeg",
        "-i",
        "-",
        "-vf",
        &filter,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(codec_args);
    if options.format == Format::Mp4 {
        args.extend(["-movflags".to_string(), "+faststart".to_string()]);
    }
    args.push(out.display().to_string());
    Ok((codec.to_string(), args))
}

/// A [`MakeEncoder`] that runs `ffmpeg` with [`ffmpeg_args`].
pub fn ffmpeg_encoder(
    ffmpeg: PathBuf,
    encoders: String,
    options: VideoOptions,
    out: PathBuf,
) -> MakeEncoder {
    Box::new(move |width, height| {
        let (codec, args) = ffmpeg_args(options, width, height, &encoders, &out)?;
        Ok(Encoder {
            program: ffmpeg,
            args,
            env: Vec::new(),
            codec,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bmcp_core::testing::FakeBackend;

    /// A tiny "JPEG": start-of-image + start-of-frame (80×64) + end-of-image. Enough for
    /// [`jpeg_size`] and the fake encoder, which never decodes it.
    fn fake_jpeg() -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        v.extend([0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x40, 0x00, 0x50, 0x03]);
        v.extend([0; 9]);
        v.extend([0xFF, 0xD9]);
        v
    }

    #[test]
    fn jpeg_size_reads_the_frame_header() {
        assert_eq!(jpeg_size(&fake_jpeg()), Some((80, 64)));
        assert_eq!(jpeg_size(b"not a jpeg"), None);
        assert_eq!(jpeg_size(&[0xFF, 0xD8]), None);
    }

    const LISTING: &str = " V....D libx264              libx264 H.264\n \
                           V....D h264_mf              H264 via MediaFoundation\n \
                           V....D libvpx-vp9           libvpx VP9\n";

    #[test]
    fn scoop_shims_are_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real-ffmpeg.exe");
        std::fs::write(&real, b"").unwrap();
        let shim = tmp.path().join("ffmpeg.exe");
        std::fs::write(&shim, b"").unwrap();
        assert_eq!(resolve_shim(&shim), None);
        std::fs::write(
            tmp.path().join("ffmpeg.shim"),
            format!("path = \"{}\"\n", real.display()),
        )
        .unwrap();
        assert_eq!(resolve_shim(&shim), Some(real));
    }

    #[test]
    fn ffmpeg_args_pick_the_encoder() {
        let out = Path::new("out.webm");
        let (codec, args) = ffmpeg_args(VideoOptions::default(), 1280, 720, LISTING, out).unwrap();
        assert_eq!(codec, "libvpx-vp9");
        let line = args.join(" ");
        assert!(
            line.starts_with(
                "-hide_banner -loglevel error -y -f image2pipe -framerate 10 -c:v mjpeg -i -"
            ),
            "{line}"
        );
        assert!(line.contains("scale=1280:720"), "{line}");
        assert!(line.contains("-crf 23"), "{line}");
        assert!(line.ends_with("out.webm"));

        let mp4 = VideoOptions {
            format: Format::Mp4,
            ..VideoOptions::default()
        };
        let (codec, _) = ffmpeg_args(mp4, 2, 2, LISTING, out).unwrap();
        assert_eq!(codec, "libx264");
        let only_mf = " V....D h264_mf              H264 via MediaFoundation\n";
        let (codec, args) = ffmpeg_args(mp4, 2, 2, only_mf, out).unwrap();
        assert_eq!(codec, "h264_mf");
        assert!(args.join(" ").contains("+faststart"));
        assert!(ffmpeg_args(mp4, 2, 2, "", out).is_err());
        assert!(ffmpeg_args(VideoOptions::default(), 2, 2, "", out).is_err());
        // "libx264rgb" must not count as "libx264".
        assert!(!has_encoder(" V....D libx264rgb   x", "libx264"));
    }

    /// Not a real test: the fake encoder process. [`fake_encoder`] re-runs this test binary
    /// with only this test selected and `FAKE_ENCODER_OUT` set; it copies stdin to that file.
    #[test]
    fn fake_encoder_process() {
        let Some(out) = std::env::var_os("FAKE_ENCODER_OUT") else {
            return;
        };
        let mut input = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut input).unwrap();
        std::fs::write(out, input).unwrap();
    }

    fn fake_encoder(out: PathBuf) -> MakeEncoder {
        Box::new(move |width, height| {
            assert_eq!((width, height), (80, 64));
            Ok(Encoder {
                program: std::env::current_exe().unwrap(),
                args: vec![
                    "video::tests::fake_encoder_process".into(),
                    "--exact".into(),
                    "--test-threads=1".into(),
                ],
                env: vec![("FAKE_ENCODER_OUT".into(), out.display().to_string())],
                codec: "fake".into(),
            })
        })
    }

    #[tokio::test]
    async fn frames_are_acked_and_written_at_a_constant_rate() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("frames.bin");
        let fake = FakeBackend::with_tabs(&["https://a.test"]);
        fake.on_cdp(|_, method, _| {
            method
                .starts_with("Page.")
                .then(|| Ok(serde_json::json!({})))
        });
        let options = VideoOptions {
            fps: 20,
            ..VideoOptions::default()
        };
        let path = tmp.path().join("video.webm");
        let recorder = VideoRecorder::start(
            fake.clone(),
            TabId(1),
            options,
            path,
            fake_encoder(out.clone()),
        )
        .await
        .unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(fake_jpeg());
        for session in 1..=3 {
            fake.emit_cdp_event(
                TabId(1),
                "Page.screencastFrame",
                json!({ "data": data, "sessionId": session, "metadata": {} }),
            );
        }
        // Then the page is still for a while: the last frame must be repeated.
        tokio::time::sleep(Duration::from_millis(800)).await;
        let report = recorder.stop().await;
        assert_eq!(report.error, None, "{report:?}");
        assert_eq!(report.frames_received, 3);
        assert_eq!((report.width, report.height), (80, 64));
        // The clock starts when the encoder is up (starting a process can take a while on a
        // busy Windows machine), then runs at 20 fps whether frames come or not.
        let expected = (report.wall_seconds * 20.0).floor() as u64;
        assert!(
            report.frames_written + 1 >= expected,
            "constant rate: {report:?}"
        );
        if report.wall_seconds > 0.3 {
            assert!(
                report.frames_written > 3,
                "still page repeats frames: {report:?}"
            );
        }

        let calls = fake.cdp_calls();
        let acks: Vec<_> = calls
            .iter()
            .filter(|(m, _)| m == "Page.screencastFrameAck")
            .map(|(_, p)| p["sessionId"].as_i64().unwrap())
            .collect();
        assert_eq!(acks, [1, 2, 3]);
        assert_eq!(calls[0].0, "Page.startScreencast");
        assert_eq!(calls[0].1["format"], "jpeg");
        assert_eq!(calls.last().unwrap().0, "Page.stopScreencast");

        let written = std::fs::read(&out).unwrap();
        assert_eq!(
            written.len(),
            fake_jpeg().len() * report.frames_written as usize
        );
    }

    #[tokio::test]
    async fn no_frames_is_an_error_not_a_hang() {
        let fake = FakeBackend::with_tabs(&["https://a.test"]);
        fake.on_cdp(|_, _, _| Some(Ok(serde_json::json!({}))));
        let tmp = tempfile::tempdir().unwrap();
        let recorder = VideoRecorder::start(
            fake.clone(),
            TabId(1),
            VideoOptions::default(),
            tmp.path().join("v.webm"),
            Box::new(|_, _| Err("must not be called".into())),
        )
        .await
        .unwrap();
        let report = recorder.stop().await;
        assert!(report.error.unwrap().contains("no screencast frame"));
        assert_eq!(report.frames_written, 0);
    }
}
