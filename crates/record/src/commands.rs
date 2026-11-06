//! The `record.*` commands.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bmcp_core::{CmdError, Command, Ctx, ErrorCode, Meta, TabId, Toolset};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::codegen::{self, InputSpec};
use crate::human::{self, Mode};
use crate::model::{RecordingMeta, Source, Step};
use crate::session::Session;
use crate::store::{self, Active, Log, Recordings, check_id, data_dir, io_error, lock, now_ms};
use crate::video::{self, Format, VideoOptions, VideoRecorder, VideoReport};

/// URL and main-frame id of the tab.
async fn tab_page(ctx: &Ctx, tab: TabId) -> Result<(String, String), CmdError> {
    let tree = ctx
        .backend
        .cdp(tab, "Page.getFrameTree".into(), json!({}))
        .await?;
    let frame = &tree["frameTree"]["frame"];
    let url = frame["url"].as_str().unwrap_or_default().to_string();
    let id = frame["id"].as_str().unwrap_or_default().to_string();
    Ok((url, id))
}

// ------------------------------------------------------------------ video helpers

/// Check the options, find ffmpeg and start the screencast into `path`.
async fn start_video(
    ctx: &Ctx,
    tab: TabId,
    options: VideoOptions,
    path: PathBuf,
) -> Result<VideoRecorder, CmdError> {
    if !(1..=60).contains(&options.fps) {
        return Err(CmdError::invalid_params("fps must be between 1 and 60"));
    }
    if !(1..=100).contains(&options.quality) {
        return Err(CmdError::invalid_params(
            "quality must be between 1 and 100",
        ));
    }
    let encoder = match ctx.extensions.get::<EncoderOverride>() {
        // Tests: a fake encoder process.
        Some(over) => (over.0)(path.clone()),
        None => {
            let ffmpeg = video::find_ffmpeg().ok_or_else(video::ffmpeg_missing)?;
            let encoders = video::ffmpeg_encoders(&ffmpeg).await;
            // Fail now (not at the first frame) when the format cannot be encoded.
            video::ffmpeg_args(options, 2, 2, &encoders, &path).map_err(|e| {
                CmdError::new(ErrorCode::NotFound, e).with_hint("Try the other format.")
            })?;
            video::ffmpeg_encoder(ffmpeg, encoders, options, path.clone())
        }
    };
    Ok(VideoRecorder::start(ctx.backend.clone(), tab, options, path, encoder).await?)
}

/// Replaces ffmpeg (tests): makes the encoder for an output path.
pub struct EncoderOverride(pub Box<dyn Fn(PathBuf) -> video::MakeEncoder + Send + Sync>);

// ------------------------------------------------------------------ record.start

pub struct Start;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StartArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Name of the recording (also the default script name). Default "recording".
    pub name: Option<String>,
    /// Also record a video (needs ffmpeg). Default false.
    pub video: Option<bool>,
    /// Also log network requests to traffic.jsonl. Default false.
    pub traffic: Option<bool>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StartOutput {
    /// Recording id, for record_stop / record_get / record_codegen.
    pub id: String,
    pub tab: TabId,
    /// Folder with actions.jsonl and meta.json.
    pub dir: String,
    /// Video file being written, when video was asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
}

impl Command for Start {
    const NAME: &'static str = "record.start";
    type Args = StartArgs;
    type Output = StartOutput;

    fn meta() -> Meta {
        Meta::new(
            "Start recording a tab: every page/nav/input/tabs command and what a person does \
             in the page (clicks, typing, keys, selects, navigations) go to an action log. \
             Optional video and network log. Returns the recording id.",
            Toolset::Record,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: StartArgs) -> Result<StartOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let data = data_dir(&ctx)?;
        let name = args.name.unwrap_or_else(|| "recording".into());
        let (id, dir) = store::new_recording_dir(&data, &name)?;

        // Subscribe before installing anything, so the recorder's first events are not lost.
        let bus = ctx.events.subscribe();
        let cdp = ctx.backend.cdp_events(tab).await?;
        // Page events (frameNavigated, screencast) need the Page domain; unlike
        // Runtime.enable it is not visible to the page.
        ctx.backend
            .cdp(tab, "Page.enable".into(), json!({}))
            .await?;
        let (url, frame) = tab_page(&ctx, tab).await?;

        let mut meta = RecordingMeta {
            id: id.clone(),
            name,
            tab,
            started_ms: now_ms(),
            start_url: url.clone(),
            human_capture: "pending".into(),
            ..RecordingMeta::default()
        };
        let script_id = match human::install(&ctx.backend, tab, Mode::Binding).await {
            Ok(id) => id,
            Err(err) => {
                // API steps are still recorded; say why human ones are missing.
                meta.human_capture = format!("failed: {err}");
                String::new()
            }
        };

        let traffic = if args.traffic.unwrap_or(false) {
            ctx.backend
                .cdp(tab, "Network.enable".into(), json!({}))
                .await?;
            let path = dir.join("traffic.jsonl");
            let file = std::fs::File::create(&path).map_err(|e| io_error(&path, e))?;
            meta.traffic = Some(path.display().to_string());
            Some(std::io::BufWriter::new(file))
        } else {
            None
        };
        let video = if args.video.unwrap_or(false) {
            let path = dir.join("video.webm");
            let recorder = start_video(&ctx, tab, VideoOptions::default(), path.clone()).await?;
            meta.video = Some(path.display().to_string());
            Some(recorder)
        } else {
            None
        };

        let mut log = Log::new(dir.join("actions.jsonl"));
        log.push(Step {
            ts_ms: meta.started_ms,
            source: Source::System,
            action: "start".into(),
            url: Some(url.clone()),
            ..Step::default()
        });
        log.save()?;
        meta.steps = 1;
        store::write_meta(&dir, &meta)?;

        let log = Arc::new(Mutex::new(log));
        let meta_shared = Arc::new(Mutex::new(meta.clone()));
        let mut session = Session::new(
            ctx.clone(),
            tab,
            dir.clone(),
            log.clone(),
            meta_shared.clone(),
            frame,
            url,
            script_id,
        );
        session.video = video;
        session.traffic = traffic;
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(session.run(bus, cdp, stopped));
        Recordings::from_ctx(&ctx).insert(Arc::new(Active {
            id: id.clone(),
            dir: dir.clone(),
            meta: meta_shared,
            log,
            stop: Mutex::new(Some(stop)),
            task: Mutex::new(Some(task)),
        }));
        ctx.events.emit(
            "record.started",
            json!({ "id": id, "tab": tab, "name": meta.name, "start_url": meta.start_url }),
        );
        Ok(StartOutput {
            id,
            tab,
            dir: dir.display().to_string(),
            video: meta.video,
        })
    }
}

// ------------------------------------------------------------------ record.stop

pub struct Stop;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StopArgs {
    /// Recording id from record_start. Default: the only running recording.
    pub id: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StopOutput {
    pub id: String,
    /// Steps in actions.jsonl.
    pub steps: usize,
    /// How human actions were captured (binding, poll, failed: …).
    pub human_capture: String,
    pub actions: String,
    pub meta: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic: Option<String>,
}

impl Command for Stop {
    const NAME: &'static str = "record.stop";
    type Args = StopArgs;
    type Output = StopOutput;

    fn meta() -> Meta {
        Meta::new(
            "Stop a recording and write its files. Returns the paths of actions.jsonl, \
             meta.json and the optional video / traffic log.",
            Toolset::Record,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: StopArgs) -> Result<StopOutput, CmdError> {
        let recordings = Recordings::from_ctx(&ctx);
        let active = recordings.pick(args.id.as_deref())?;
        recordings.remove(&active.id);
        if let Some(stop) = lock(&active.stop).take() {
            let _ = stop.send(());
        }
        let task = lock(&active.task).take();
        let meta = match task {
            Some(task) => task
                .await
                .map_err(|e| CmdError::internal(format!("recording task failed: {e}")))?,
            None => lock(&active.meta).clone(),
        };
        ctx.events.emit(
            "record.stopped",
            json!({ "id": meta.id, "steps": meta.steps, "video": meta.video }),
        );
        Ok(StopOutput {
            id: meta.id.clone(),
            steps: meta.steps,
            human_capture: meta.human_capture.clone(),
            actions: active.dir.join("actions.jsonl").display().to_string(),
            meta: active.dir.join("meta.json").display().to_string(),
            video: meta.video,
            traffic: meta.traffic,
        })
    }
}

// ------------------------------------------------------------------ record.list

pub struct List;

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListEntry {
    pub id: String,
    pub name: String,
    pub active: bool,
    pub steps: usize,
    pub started_ms: u64,
    pub start_url: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListOutput {
    /// Newest first.
    pub recordings: Vec<ListEntry>,
}

impl Command for List {
    const NAME: &'static str = "record.list";
    type Args = bmcp_core::command::Empty;
    type Output = ListOutput;

    fn meta() -> Meta {
        Meta::new(
            "List recordings (running and saved), newest first.",
            Toolset::Record,
        )
    }

    async fn run(ctx: Ctx, _args: bmcp_core::command::Empty) -> Result<ListOutput, CmdError> {
        let data = data_dir(&ctx)?;
        let recordings = Recordings::from_ctx(&ctx);
        let running = recordings.ids();
        let recordings = store::list_on_disk(&data)
            .into_iter()
            .map(|m| {
                let active = running.contains(&m.id);
                let steps = match Recordings::from_ctx(&ctx).get(&m.id) {
                    Some(a) => lock(&a.log).written().count(),
                    None => m.steps,
                };
                ListEntry {
                    active,
                    steps,
                    id: m.id,
                    name: m.name,
                    started_ms: m.started_ms,
                    start_url: m.start_url,
                }
            })
            .collect();
        Ok(ListOutput { recordings })
    }
}

// ------------------------------------------------------------------ record.get

pub struct Get;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GetFormat {
    /// The raw actions.jsonl lines.
    Jsonl,
    /// One readable line per step (default).
    #[default]
    Summary,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetArgs {
    /// Recording id.
    pub id: String,
    /// summary (default) or jsonl.
    pub format: Option<GetFormat>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetOutput {
    pub id: String,
    pub active: bool,
    pub meta: RecordingMeta,
    pub steps: usize,
    pub text: String,
}

/// Meta and steps of a recording, from memory while it runs, else from disk.
fn load(ctx: &Ctx, id: &str) -> Result<(RecordingMeta, Vec<Step>, bool), CmdError> {
    check_id(id)?;
    if let Some(active) = Recordings::from_ctx(ctx).get(id) {
        let steps = lock(&active.log).written().cloned().collect();
        return Ok((lock(&active.meta).clone(), steps, true));
    }
    let dir = store::recordings_dir(&data_dir(ctx)?).join(id);
    if !dir.is_dir() {
        return Err(
            CmdError::new(ErrorCode::NotFound, format!("no recording {id:?}"))
                .with_hint("record_list shows the recording ids."),
        );
    }
    Ok((store::read_meta(&dir)?, store::read_steps(&dir)?, false))
}

impl Command for Get {
    const NAME: &'static str = "record.get";
    type Args = GetArgs;
    type Output = GetOutput;

    fn meta() -> Meta {
        Meta::new(
            "A recording's steps: a readable summary (default) or the raw JSONL action log.",
            Toolset::Record,
        )
    }

    async fn run(ctx: Ctx, args: GetArgs) -> Result<GetOutput, CmdError> {
        let (meta, steps, active) = load(&ctx, &args.id)?;
        let text = match args.format.unwrap_or_default() {
            GetFormat::Summary => codegen::summary(&steps),
            GetFormat::Jsonl => steps
                .iter()
                .map(|s| serde_json::to_string(s).unwrap_or_default() + "\n")
                .collect(),
        };
        Ok(GetOutput {
            id: meta.id.clone(),
            active,
            steps: steps.len(),
            meta,
            text,
        })
    }
}

// ------------------------------------------------------------------ record.codegen

pub struct Codegen;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CodegenArgs {
    /// Recording id.
    pub id: String,
    /// Turn typed/selected values into `inputs.<name>` with the recorded value as default.
    /// Default false (secrets always become required inputs).
    pub parametrize: Option<bool>,
    /// Script file name without .ts (default: the recording name).
    pub name: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct CodegenOutput {
    /// Where the script was saved (data/scripts/<name>.ts).
    pub path: String,
    /// The TypeScript.
    pub code: String,
    /// Inputs the script reads (`--input name=value`).
    pub inputs: Vec<InputSpec>,
}

impl Command for Codegen {
    const NAME: &'static str = "record.codegen";
    type Args = CodegenArgs;
    type Output = CodegenOutput;

    fn meta() -> Meta {
        Meta::new(
            "Turn a recording into a TypeScript script for the script host (elements found by \
             role and name, not refs). With parametrize, typed values become inputs. Saves it \
             to data/scripts/<name>.ts and returns the code.",
            Toolset::Record,
        )
    }

    async fn run(ctx: Ctx, args: CodegenArgs) -> Result<CodegenOutput, CmdError> {
        let (meta, steps, _) = load(&ctx, &args.id)?;
        let options = codegen::Options {
            parametrize: args.parametrize.unwrap_or(false),
        };
        let generated = codegen::generate(&meta, &steps, options);
        let dir = store::scripts_dir(&data_dir(&ctx)?);
        std::fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
        let file = store::slug(args.name.as_deref().unwrap_or(&meta.name));
        let path = dir.join(format!("{file}.ts"));
        std::fs::write(&path, &generated.code).map_err(|e| io_error(&path, e))?;
        Ok(CodegenOutput {
            path: path.display().to_string(),
            code: generated.code,
            inputs: generated.inputs,
        })
    }
}

// ------------------------------------------------------------------ record.video_start / stop

/// Running videos started by `record.video_start` (an `Extensions` service).
#[derive(Default)]
pub struct Videos(Mutex<Vec<(String, VideoRecorder)>>);

pub struct VideoStart;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoStartArgs {
    /// Tab id from tabs_list. Default: the most recently opened tab.
    pub tab: Option<TabId>,
    /// Frames per second, 1–60 (default 10).
    pub fps: Option<u32>,
    /// webm (VP9, default) or mp4 (H.264).
    pub format: Option<Format>,
    /// 1–100 (default 80): JPEG quality of the frames and encoder quality.
    pub quality: Option<u8>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct VideoStartOutput {
    /// Video id for record_video_stop.
    pub id: String,
    pub tab: TabId,
    /// The file being written.
    pub path: String,
}

impl Command for VideoStart {
    const NAME: &'static str = "record.video_start";
    type Args = VideoStartArgs;
    type Output = VideoStartOutput;

    fn meta() -> Meta {
        Meta::new(
            "Start recording a tab's video (CDP screencast → ffmpeg, constant frame rate). \
             Needs ffmpeg (winget install Gyan.FFmpeg). Stop with record_video_stop.",
            Toolset::Record,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: VideoStartArgs) -> Result<VideoStartOutput, CmdError> {
        let tab = ctx.resolve_tab(args.tab).await?;
        let defaults = VideoOptions::default();
        let options = VideoOptions {
            fps: args.fps.unwrap_or(defaults.fps),
            format: args.format.unwrap_or(defaults.format),
            quality: args.quality.unwrap_or(defaults.quality),
        };
        let dir = store::videos_dir(&data_dir(&ctx)?);
        std::fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
        let id = format!("tab{}-{}", tab.0, store::stamp(now_ms()));
        let path = dir.join(format!("{id}.{}", options.format.extension()));
        let recorder = start_video(&ctx, tab, options, path.clone()).await?;
        ctx.extensions
            .get_or_insert_with(Videos::default)
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id.clone(), recorder));
        Ok(VideoStartOutput {
            id,
            tab,
            path: path.display().to_string(),
        })
    }
}

pub struct VideoStop;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoStopArgs {
    /// Video id from record_video_start. Default: the most recently started video.
    pub id: Option<String>,
}

impl Command for VideoStop {
    const NAME: &'static str = "record.video_stop";
    type Args = VideoStopArgs;
    type Output = VideoReport;

    fn meta() -> Meta {
        Meta::new(
            "Stop a video and finish the file. Returns its path, size, frame counts and length.",
            Toolset::Record,
        )
        .act()
    }

    async fn run(ctx: Ctx, args: VideoStopArgs) -> Result<VideoReport, CmdError> {
        let videos = ctx.extensions.get_or_insert_with(Videos::default);
        let recorder = {
            let mut list = videos.0.lock().unwrap_or_else(|e| e.into_inner());
            let index = match &args.id {
                Some(id) => list.iter().position(|(i, _)| i == id),
                None => list.len().checked_sub(1),
            };
            index.map(|i| list.remove(i).1)
        };
        let recorder = recorder.ok_or_else(|| {
            CmdError::new(ErrorCode::NotFound, "no such running video")
                .with_hint("Start one with record_video_start.")
        })?;
        let report = recorder.stop().await;
        if report.bytes == 0
            && let Some(err) = &report.error
        {
            return Err(CmdError::internal(format!("video failed: {err}")));
        }
        Ok(report)
    }
}
