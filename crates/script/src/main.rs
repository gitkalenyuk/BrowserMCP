//! `bmcp-script-host`: command-line entry point of the script sidecar.
//!
//! ```text
//! bmcp-script-host run <file.ts> [--input key=value]... [--inputs-file <file.json>]
//!                      [--mock | --ws <url>] [--timeout-secs N] [--timings]
//!                      [--inspect pipe|<port>]
//! ```
//!
//! `--inputs-file` reads a JSON object (`{"key": "value", …}`); programs that start the host
//! use it instead of `--input` so long values or NUL bytes never hit the command-line limit.
//!
//! Exit codes: 0 ok, 1 uncaught script error, 2 usage/setup error, 124 timeout
//! (same number GNU `timeout` uses).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use bmcp_script::{
    CommandSink, DebugChannel, MockSink, RunOptions, RunOutcome, WsSink, run_script,
    run_script_debug,
};
use deno_core::serde_json::Value;

const USAGE: &str = "\
usage: bmcp-script-host run <file.ts> [options]

options:
  --input key=value   expose `inputs.key = \"value\"` to the script (repeatable)
  --inputs-file F     read inputs from a JSON object file {\"key\": \"value\", ...}
                      (applied before --input; non-string values become JSON text)
  --mock              answer browser.* calls with canned data (default)
  --ws <url>          send browser.* calls as JSON-RPC to a WebSocket
                      of the running browser (e.g. ws://127.0.0.1:47800/v1/ws)
  --timeout-secs N    terminate the script after N seconds (exit code 124)
  --timings           print startup/run timings to stderr
  --inspect pipe|PORT step debugger (off by default): load the script, then wait for
                      commands on stdin (events on stdout as `##bmcp:debug {json}`)
                      or on a TCP socket at 127.0.0.1:PORT; see src/debugger.rs";

/// Where browser calls go.
enum SinkChoice {
    Mock,
    Ws(String),
}

/// Parsed command line.
struct Cli {
    script: PathBuf,
    inputs: Vec<(String, String)>,
    sink: SinkChoice,
    timeout: Option<Duration>,
    timings: bool,
    /// `--inspect`: `Some("pipe")` or `Some("<port>")`.
    inspect: Option<String>,
}

/// Hand-written parser: few flags, so no extra dependency.
fn parse_args(args: Vec<String>) -> anyhow::Result<Cli> {
    let mut args = args.into_iter();
    match args.next().as_deref() {
        Some("run") => {}
        Some("-h" | "--help") | None => bail!("{USAGE}"),
        Some(other) => bail!("unknown command `{other}`\n\n{USAGE}"),
    }
    let mut cli = Cli {
        script: PathBuf::new(),
        inputs: Vec::new(),
        sink: SinkChoice::Mock,
        timeout: None,
        timings: false,
        inspect: None,
    };
    let mut script = None;
    let mut file_inputs = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--input" => {
                let pair = args.next().context("--input needs key=value")?;
                let (key, value) = pair
                    .split_once('=')
                    .with_context(|| format!("--input `{pair}` is not key=value"))?;
                cli.inputs.push((key.to_string(), value.to_string()));
            }
            "--inputs-file" => {
                let path = args.next().context("--inputs-file needs a path")?;
                file_inputs.extend(read_inputs_file(Path::new(&path))?);
            }
            "--mock" => cli.sink = SinkChoice::Mock,
            "--ws" => cli.sink = SinkChoice::Ws(args.next().context("--ws needs a URL")?),
            "--timeout-secs" => {
                let secs: f64 = args
                    .next()
                    .context("--timeout-secs needs a number")?
                    .parse()
                    .context("--timeout-secs must be a number")?;
                if !(secs.is_finite() && secs > 0.0) {
                    bail!("--timeout-secs must be > 0");
                }
                cli.timeout = Some(Duration::from_secs_f64(secs));
            }
            "--timings" => cli.timings = true,
            "--inspect" => {
                let target = args.next().context("--inspect needs `pipe` or a port")?;
                if target != "pipe" && target.parse::<u16>().is_err() {
                    bail!("--inspect must be `pipe` or a port number, got `{target}`");
                }
                cli.inspect = Some(target);
            }
            flag if flag.starts_with("--") => bail!("unknown option `{flag}`\n\n{USAGE}"),
            path if script.is_none() => script = Some(PathBuf::from(path)),
            extra => bail!("unexpected argument `{extra}`"),
        }
    }
    cli.script = script.with_context(|| format!("missing <file.ts>\n\n{USAGE}"))?;
    // File inputs first, so a later `--input` with the same key wins.
    file_inputs.append(&mut cli.inputs);
    cli.inputs = file_inputs;
    Ok(cli)
}

/// `--inputs-file`: a JSON object; string values as-is, other values as JSON text.
fn read_inputs_file(path: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read --inputs-file {}", path.display()))?;
    let value: Value = deno_core::serde_json::from_str(&text)
        .with_context(|| format!("--inputs-file {} is not JSON", path.display()))?;
    let Value::Object(map) = value else {
        bail!(
            "--inputs-file {} must hold a JSON object {{\"key\": \"value\"}}",
            path.display()
        );
    };
    Ok(map
        .into_iter()
        .map(|(key, value)| match value {
            Value::String(s) => (key, s),
            other => (key, other.to_string()),
        })
        .collect())
}

fn main() -> ExitCode {
    let cli = match parse_args(std::env::args().skip(1).collect()) {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("{err:#}");
            return ExitCode::from(2);
        }
    };

    // One thread, one isolate: V8 isolates are not thread-safe, and a
    // current-thread tokio runtime keeps everything on this thread.
    let tokio = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("error: cannot start tokio: {err}");
            return ExitCode::from(2);
        }
    };

    let started = Instant::now();
    let result = tokio.block_on(async {
        let sink: Rc<dyn CommandSink> = match &cli.sink {
            SinkChoice::Mock => Rc::new(MockSink),
            SinkChoice::Ws(url) => Rc::new(WsSink::connect(url).await?),
        };
        let opts = RunOptions {
            script: cli.script.clone(),
            inputs: cli.inputs.clone(),
            timeout: cli.timeout,
        };
        match cli.inspect.as_deref() {
            None => run_script(opts, sink).await,
            Some("pipe") => run_script_debug(opts, sink, DebugChannel::pipe()).await,
            Some(port) => {
                let channel = DebugChannel::tcp(port.parse()?)?;
                run_script_debug(opts, sink, channel).await
            }
        }
    });

    match result {
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
        Ok((outcome, timings)) => {
            if cli.timings {
                eprintln!(
                    "[timings] runtime_init={:.1}ms run={:.1}ms total={:.1}ms",
                    timings.runtime_init.as_secs_f64() * 1000.0,
                    timings.run.as_secs_f64() * 1000.0,
                    started.elapsed().as_secs_f64() * 1000.0,
                );
            }
            match outcome {
                RunOutcome::Completed => ExitCode::SUCCESS,
                RunOutcome::Failed(message) => {
                    eprintln!("error: {message}");
                    ExitCode::from(1)
                }
                RunOutcome::TimedOut(limit) => {
                    eprintln!("error: script timed out after {limit:?} and was terminated");
                    ExitCode::from(124)
                }
            }
        }
    }
}
