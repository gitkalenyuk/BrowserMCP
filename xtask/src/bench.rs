//! `cargo xtask bench --agent claude-code|codex --tasks all|<id,id> [options]`
//!
//! The MVP exit benchmark (plan Phase 3): a CLI agent solves 10 tasks on local fixture sites
//! through BrowserMCP's MCP server, without screenshots. For every task we record pass/fail
//! (a machine check, see [`tasks`]), tool calls and tokens, and write
//! `tests/bench/results/<date>.json` + `<date>.md`.
//!
//! What a run does:
//! 1. builds `browsermcp` (skip with `--no-build`);
//! 2. serves `tests/fixtures/sites/` on a free port ([`http::Fixtures`]);
//! 3. starts its own browser: `--api-port 47841 --data-dir target/live-data` (isolated from a
//!    browser you use), and waits for `/v1/health`;
//! 4. per task: forgets the `bench` agent and leaves one blank tab (REST), runs the agent
//!    headless with `…/mcp?agent=bench`, then checks the outcome;
//! 5. kills only the browser process it started.
//!
//! Options: `--model <m>`, `--timeout <secs>` (per task, default 300), `--port <api port>`,
//! `--no-build`, `--keep-open` (leave the browser running at the end).

mod agent;
mod http;
mod tasks;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::{Result, workspace_root};
use agent::{AgentKind, AgentOptions, AgentRun};
use http::{Fixtures, rest};
use tasks::{Outcome, Task};

/// API port of the benchmark browser (not the default 47800, so a daily browser is untouched).
const DEFAULT_PORT: u16 = 47841;
/// Agent id the benchmark connects as (`/mcp?agent=bench`).
const AGENT_ID: &str = "bench";

struct Options {
    agent: AgentKind,
    tasks: Vec<&'static Task>,
    model: Option<String>,
    timeout: Duration,
    port: u16,
    build: bool,
    keep_open: bool,
}

fn value_of(args: &[String], flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    args.get(at + 1).cloned()
}

fn parse_options(args: &[String]) -> Result<Options> {
    let agent = AgentKind::parse(&value_of(args, "--agent").unwrap_or("claude-code".into()))?;
    let tasks = tasks::select(&value_of(args, "--tasks").unwrap_or("all".into()))?;
    let timeout = value_of(args, "--timeout")
        .map(|s| {
            s.parse()
                .map_err(|_| format!("--timeout {s}: not a number"))
        })
        .transpose()?
        .unwrap_or(300);
    let port = value_of(args, "--port")
        .map(|s| s.parse().map_err(|_| format!("--port {s}: not a port")))
        .transpose()?
        .unwrap_or(DEFAULT_PORT);
    Ok(Options {
        agent,
        tasks,
        model: value_of(args, "--model"),
        timeout: Duration::from_secs(timeout),
        port,
        build: !args.iter().any(|a| a == "--no-build"),
        keep_open: args.iter().any(|a| a == "--keep-open"),
    })
}

pub fn bench(args: &[String]) -> Result {
    let opts = parse_options(args)?;
    let root = workspace_root();
    if opts.build {
        crate::build(false)?;
    }
    let exe = target_dir(&root)
        .join("debug")
        .join(format!("browsermcp{}", std::env::consts::EXE_SUFFIX));
    if !exe.exists() {
        return Err(format!("{} not found; build it first", exe.display()));
    }

    let fixtures = Fixtures::start(root.join("tests").join("fixtures").join("sites"))?;
    println!("fixtures at {}/bench/", fixtures.base());
    let mut browser = Browser::start(&exe, opts.port, &root.join("target").join("live-data"))?;
    println!("browser pid {} on port {}", browser.child.id(), opts.port);

    let result = run_tasks(&opts, &fixtures, &root, &mut browser);
    if opts.keep_open {
        println!(
            "--keep-open: browser pid {} is still running",
            browser.child.id()
        );
        std::mem::forget(browser);
    } else {
        browser.stop();
    }
    let report = result?;
    let (json_path, md_path) = write_report(&root, &report)?;
    println!("\n{}", markdown(&report));
    println!("wrote {} and {}", json_path.display(), md_path.display());
    Ok(())
}

/// `CARGO_TARGET_DIR` when set (long-path workaround on Windows), else `<root>/target`.
fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

/// The browser process this run started (and the only one it may kill).
struct Browser {
    child: Child,
    exe: PathBuf,
    data_dir: PathBuf,
}

impl Browser {
    fn start(exe: &Path, port: u16, data_dir: &Path) -> Result<Browser> {
        if http::request(port, "GET", "/v1/health", None).is_ok() {
            return Err(format!(
                "port {port} is already in use (another browser?); pass --port"
            ));
        }
        std::fs::create_dir_all(data_dir).map_err(|e| e.to_string())?;
        // Appended: a restarted browser must not erase why the previous one went away.
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(data_dir.join("bench-browser.log"))
            .map_err(|e| e.to_string())?;
        let child = Command::new(exe)
            .arg("--api-port")
            .arg(port.to_string())
            .arg("--data-dir")
            .arg(data_dir)
            .args(["--url", "about:blank"])
            // Keep a covered/minimized window rendering and accepting input events.
            .args([
                "--disable-features=CalculateNativeWinOcclusion",
                "--disable-backgrounding-occluded-windows",
            ])
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        let mut browser = Browser {
            child,
            exe: exe.to_path_buf(),
            data_dir: data_dir.to_path_buf(),
        };
        let deadline = Instant::now() + Duration::from_secs(90);
        while http::request(port, "GET", "/v1/health", None).is_err() {
            if Instant::now() > deadline || browser.child.try_wait().ok().flatten().is_some() {
                browser.stop();
                return Err(format!(
                    "the browser did not come up; see {}",
                    data_dir.join("bench-browser.log").display()
                ));
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        Ok(browser)
    }

    fn stop(&mut self) {
        // Our own child by handle (never by image name): CEF's helper processes exit with it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Before a task: the `bench` agent is forgotten (its locks released) and exactly one blank
/// tab is left open (closing the last tab would quit the browser).
fn reset_browser(port: u16) -> Result {
    let _ = rest(port, "agents.forget", json!({ "id": AGENT_ID }));
    let fresh = rest(port, "tabs.open", json!({ "url": "about:blank" }))?;
    let keep = fresh["id"].clone();
    // Wait until the new tab has loaded before closing the others: closing them while the
    // window still sets the new tab up can leave the window empty, which quits the browser.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let tabs = rest(port, "tabs.list", json!({}))?;
        let ready = tabs["tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|t| t["id"] == keep && t["loading"] == false);
        if ready || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(500));
    let tabs = rest(port, "tabs.list", json!({}))?;
    for tab in tabs["tabs"].as_array().into_iter().flatten() {
        if tab["id"] != keep {
            rest(port, "tabs.close", json!({ "tab": tab["id"] }))?;
        }
    }
    Ok(())
}

fn healthy(port: u16) -> bool {
    http::request(port, "GET", "/v1/health", None).is_ok_and(|(status, _)| status == 200)
}

struct TaskResult {
    task: &'static Task,
    pass: bool,
    reason: String,
    run: AgentRun,
    /// Tabs the `bench` agent owned at the end (REST `tabs.list`, `lockedBy`).
    owned_tabs: usize,
    /// The first attempt was lost because the browser went away.
    retried: bool,
}

struct Report {
    agent: AgentKind,
    date: String,
    results: Vec<TaskResult>,
}

fn run_tasks(
    opts: &Options,
    fixtures: &Fixtures,
    root: &Path,
    browser: &mut Browser,
) -> Result<Report> {
    let cwd = root.join("target").join("bench-work");
    std::fs::create_dir_all(&cwd).map_err(|e| e.to_string())?;
    let agent_opts = AgentOptions {
        kind: opts.agent,
        mcp_url: format!("http://127.0.0.1:{}/mcp?agent={AGENT_ID}", opts.port),
        cwd,
        model: opts.model.clone(),
        timeout: opts.timeout,
    };
    let base = format!("{}/bench", fixtures.base());
    let mut results = Vec::new();
    for task in &opts.tasks {
        let mut attempt = 1;
        let result = loop {
            // The browser can disappear (its window closed, the agent closed the last tab):
            // then start a new one.
            if !healthy(opts.port) {
                println!("[{}] the browser is gone; restarting it", task.id);
                browser.stop();
                *browser =
                    Browser::start(&browser.exe.clone(), opts.port, &browser.data_dir.clone())?;
            }
            let result = run_task(task, &agent_opts, &base, opts.port, fixtures)?;
            // A failure because the browser went away says nothing about the agent: retry once.
            if !result.pass && !healthy(opts.port) && attempt == 1 {
                println!(
                    "[{}] the browser went away during the run; retrying",
                    task.id
                );
                attempt += 1;
                continue;
            }
            break TaskResult {
                retried: attempt > 1,
                ..result
            };
        };
        println!(
            "[{}] {} — {} tool calls, {} tokens{}",
            task.id,
            if result.pass { "PASS" } else { "FAIL" },
            result.run.tool_calls(),
            result.run.total_tokens(),
            if result.pass {
                String::new()
            } else {
                format!(": {}", result.reason)
            }
        );
        results.push(result);
    }
    Ok(Report {
        agent: opts.agent,
        date: today(),
        results,
    })
}

/// One attempt at one task: reset, run the agent, check the outcome.
fn run_task(
    task: &'static Task,
    agent_opts: &AgentOptions,
    base: &str,
    port: u16,
    fixtures: &Fixtures,
) -> Result<TaskResult> {
    reset_browser(port)?;
    fixtures.reset();
    let prompt = format!("{} {}", task.prompt.replace("{base}", base), tasks::RULES);
    println!("[{}] running {} …", task.id, agent_opts.kind.name());
    let run = agent::run(agent_opts, &prompt);
    let log: String = run.events.iter().map(|e| format!("{e}\n")).collect();
    let _ = std::fs::write(agent_opts.cwd.join(format!("{}.jsonl", task.id)), log);

    let reported = fixtures.results(task.id);
    // The browser may be gone: check without tabs then.
    let tabs = rest(port, "tabs.list", json!({})).unwrap_or(Value::Null);
    let owned_tabs = tabs["tabs"]
        .as_array()
        .map(|t| t.iter().filter(|t| t["lockedBy"] == AGENT_ID).count())
        .unwrap_or(0);
    let checked = (task.check)(&Outcome {
        results: &reported,
        tabs: &tabs,
    });
    let (pass, reason) = match (&checked, &run.error) {
        (Ok(()), _) => (true, String::new()),
        (Err(why), Some(err)) => (false, format!("{why} (agent: {err})")),
        (Err(why), None) => (false, why.clone()),
    };
    Ok(TaskResult {
        task,
        pass,
        reason,
        run,
        owned_tabs,
        retried: false,
    })
}

// ---------------------------------------------------------------- report

fn report_json(report: &Report) -> Value {
    let tasks: Vec<Value> = report
        .results
        .iter()
        .map(|r| {
            let mut v = r.run.to_json();
            v["id"] = json!(r.task.id);
            v["kind"] = json!(r.task.kind);
            v["pass"] = json!(r.pass);
            v["reason"] = json!(r.reason);
            v["owned_tabs"] = json!(r.owned_tabs);
            v["retried"] = json!(r.retried);
            v["final_text"] = json!(cut(&r.run.final_text, 300));
            v
        })
        .collect();
    let sum = |f: fn(&AgentRun) -> u64| report.results.iter().map(|r| f(&r.run)).sum::<u64>();
    json!({
        "agent": report.agent.name(),
        "date": report.date,
        "passed": report.results.iter().filter(|r| r.pass).count(),
        "total": report.results.len(),
        "tool_calls": sum(|r| r.tool_calls() as u64),
        "tokens": sum(AgentRun::total_tokens),
        "output_tokens": sum(|r| r.output_tokens),
        "cost_usd": report.results.iter().filter_map(|r| r.run.cost_usd).fold(0.0, |a, b| a + b),
        "tasks": tasks,
    })
}

fn markdown(report: &Report) -> String {
    let mut md = format!(
        "## {} — {}\n\n| task | kind | result | tool calls | tokens (total / output) | cost | time | note |\n\
         |---|---|---|---:|---:|---:|---:|---|\n",
        report.agent.name(),
        report.date
    );
    for r in &report.results {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} / {} | {} | {:.0} s | {} |\n",
            r.task.id,
            r.task.kind,
            if r.pass { "pass" } else { "**FAIL**" },
            r.run.tool_calls(),
            r.run.total_tokens(),
            r.run.output_tokens,
            r.run.cost_usd.map_or("–".into(), |c| format!("${c:.3}")),
            r.run.wall_ms as f64 / 1000.0,
            cut(&r.reason, 120).replace('|', "/"),
        ));
    }
    let json = report_json(report);
    md.push_str(&format!(
        "\n**{}/{} passed**, {} tool calls, {} tokens ({} output), ${:.2}. \
         Model: {}. Screenshots: disabled.\n",
        json["passed"],
        json["total"],
        json["tool_calls"],
        json["tokens"],
        json["output_tokens"],
        json["cost_usd"].as_f64().unwrap_or(0.0),
        report
            .results
            .iter()
            .find_map(|r| r.run.model.clone())
            .unwrap_or_else(|| "default".into()),
    ));
    md
}

/// `tests/bench/results/<date>.json` holds one entry per agent (a later run of the same agent
/// replaces its entry); `<date>.md` is the summary of all of them.
fn write_report(root: &Path, report: &Report) -> Result<(PathBuf, PathBuf)> {
    let dir = root.join("tests").join("bench").join("results");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json_path = dir.join(format!("{}.json", report.date));
    let md_path = dir.join(format!("{}.md", report.date));
    let mut all: Value = std::fs::read_to_string(&json_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    all[report.agent.name()] = report_json(report);
    let text = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(&json_path, text + "\n").map_err(|e| e.to_string())?;

    // Keep the other agents' sections of today's summary.
    let mut md = format!("# BrowserMCP benchmark — {}\n\n", report.date);
    let old = std::fs::read_to_string(&md_path).unwrap_or_default();
    for section in old.split("\n## ").skip(1) {
        if !section.starts_with(&format!("{} ", report.agent.name())) {
            md.push_str(&format!("## {}\n", section.trim_end()));
            md.push('\n');
        }
    }
    md.push_str(&markdown(report));
    std::fs::write(&md_path, md).map_err(|e| e.to_string())?;
    Ok((json_path, md_path))
}

/// At most `max` characters (on a char boundary), with `…` when cut.
pub fn cut(text: &str, max: usize) -> String {
    let text = text.replace('\n', " ");
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

/// Today's date (UTC) as `YYYY-MM-DD`, without a date crate.
fn today() -> String {
    let days = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_398), (2025, 11, 6));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn options() {
        let args: Vec<String> = ["--agent", "codex", "--tasks", "table,login", "--no-build"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let opts = parse_options(&args).unwrap();
        assert_eq!(opts.agent, AgentKind::Codex);
        assert_eq!(opts.tasks.len(), 2);
        assert!(!opts.build);
        assert_eq!(opts.port, DEFAULT_PORT);
        assert_eq!(cut("abcdef", 3), "abc…");
    }

    #[test]
    fn markdown_table() {
        let report = Report {
            agent: AgentKind::ClaudeCode,
            date: "2026-11-06".into(),
            results: vec![TaskResult {
                task: &tasks::TASKS[0],
                pass: true,
                reason: String::new(),
                run: AgentRun {
                    output_tokens: 10,
                    input_tokens: 5,
                    ..AgentRun::default()
                },
                owned_tabs: 1,
                retried: false,
            }],
        };
        let md = markdown(&report);
        assert!(
            md.contains("| search | search results page | pass | 0 | 15 / 10 |"),
            "{md}"
        );
        assert!(md.contains("**1/1 passed**"));
    }
}
