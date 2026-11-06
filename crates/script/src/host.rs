//! Run one script file in a fresh V8 isolate.

use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::Context;
use deno_core::error::CoreErrorKind;
use deno_core::serde_json::{Map, Value};
use deno_core::{JsRuntime, RuntimeOptions};

use crate::loader::TsModuleLoader;
use crate::ops::{Inputs, SinkHandle, bmcp};
use crate::sink::CommandSink;

/// What to run and how.
pub struct RunOptions {
    /// Path to a `.ts` or `.js` file.
    pub script: std::path::PathBuf,
    /// `--input key=value` pairs, exposed to the script as `inputs`.
    pub inputs: Vec<(String, String)>,
    /// Stop the script after this long (`None` = no limit).
    pub timeout: Option<Duration>,
}

/// How a run ended (setup problems such as "file not found" are `Err` instead).
#[derive(Debug)]
pub enum RunOutcome {
    /// The module and everything it awaited finished.
    Completed,
    /// Uncaught exception; the text is the formatted error with stack.
    Failed(String),
    /// The timeout fired and the isolate was terminated.
    TimedOut(Duration),
}

/// Rough timings for one run, for the spike measurements (`--timings`).
#[derive(Debug, Default, Clone, Copy)]
pub struct Timings {
    /// Creating the isolate + running the prelude.
    pub runtime_init: Duration,
    /// Load + transpile + evaluate the module, until its event loop is idle.
    pub run: Duration,
}

/// Run `opts.script` against `sink`. Must be awaited inside a tokio runtime
/// on the thread that will own the isolate (use a current-thread runtime).
pub async fn run_script(
    opts: RunOptions,
    sink: Rc<dyn CommandSink>,
) -> anyhow::Result<(RunOutcome, Timings)> {
    let mut timings = Timings::default();
    let main_module = module_url(&opts.script)?;

    // 1. A fresh isolate with our extension (ops + prelude) and TS loader.
    //    The sink and inputs go into the extension; ops read them from OpState.
    let inputs: Map<String, Value> = opts
        .inputs
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    let started = Instant::now();
    let mut runtime = JsRuntime::new(RuntimeOptions {
        module_loader: Some(Rc::new(TsModuleLoader::default())),
        extensions: vec![bmcp::init(SinkHandle(sink), Inputs(Value::Object(inputs)))],
        ..Default::default()
    });
    timings.runtime_init = started.elapsed();

    // 2. Watchdog for --timeout-secs. Two cases need two mechanisms:
    //    - JS is busy (`while (true) {}`): the tokio timer below can't fire,
    //      because this thread is stuck inside V8. A separate OS thread calls
    //      `terminate_execution()`, which is safe to call from any thread.
    //    - JS is idle, awaiting a slow command: V8 isn't running, so there is
    //      nothing to terminate; `tokio::time::timeout` drops the run instead.
    let timed_out = Arc::new(AtomicBool::new(false));
    let (stop_watchdog, watchdog_rx) = mpsc::channel::<()>();
    if let Some(limit) = opts.timeout {
        let isolate = runtime.v8_isolate().thread_safe_handle();
        let timed_out = timed_out.clone();
        std::thread::spawn(move || {
            // Wakes up early (Disconnected) when the script finishes first.
            if let Err(RecvTimeoutError::Timeout) = watchdog_rx.recv_timeout(limit) {
                timed_out.store(true, Ordering::SeqCst);
                isolate.terminate_execution();
            }
        });
    }

    // 3. Load, transpile, evaluate; then run the event loop until every
    //    pending promise/op is done (this is what makes top-level await work).
    let started = Instant::now();
    let work = async {
        let id = runtime.load_main_es_module(&main_module).await?;
        let evaluation = runtime.mod_evaluate(id);
        runtime.run_event_loop(Default::default()).await?;
        evaluation.await
    };
    let result = match opts.timeout {
        Some(limit) => tokio::time::timeout(limit, work).await.ok(),
        None => Some(work.await),
    };
    timings.run = started.elapsed();
    drop(stop_watchdog);

    let outcome = match result {
        // tokio timeout fired (script was awaiting something).
        None => RunOutcome::TimedOut(opts.timeout.unwrap_or_default()),
        // V8 was terminated by the watchdog (script was busy).
        Some(Err(_)) if timed_out.load(Ordering::SeqCst) => {
            RunOutcome::TimedOut(opts.timeout.unwrap_or_default())
        }
        Some(Err(err)) => RunOutcome::Failed(match err.0.as_ref() {
            // A JS exception: its Display is "Error: msg\n    at file.ts:L:C"
            // with positions already mapped back to the TS source.
            CoreErrorKind::Js(js_error) => format!("Uncaught {js_error}"),
            // Anything else: module not found, TS syntax error, ...
            _ => err.to_string(),
        }),
        Some(Ok(())) => RunOutcome::Completed,
    };
    Ok((outcome, timings))
}

/// `scripts/hello.ts` -> `file:///C:/.../scripts/hello.ts`.
fn module_url(path: &Path) -> anyhow::Result<deno_core::ModuleSpecifier> {
    let absolute = std::fs::canonicalize(path)
        .with_context(|| format!("script not found: {}", path.display()))?;
    deno_core::ModuleSpecifier::from_file_path(&absolute)
        .map_err(|()| anyhow::anyhow!("not a valid file path: {}", absolute.display()))
}
