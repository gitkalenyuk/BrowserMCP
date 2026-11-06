//! `bmcp-heal-fake-host`: stand-in for `bmcp-script-host` in the heal tests (no V8).
//!
//! ```text
//! bmcp-heal-fake-host run <file> [--ws url] [--input k=v]... [--inputs-file f.json] [--timeout-secs N]
//! bmcp-heal-fake-host check <file>
//! ```
//!
//! `run` executes the file line by line; `{{key}}` is replaced by the input `key`:
//!
//! | Line | Effect |
//! |---|---|
//! | `result <json>` | print `##bmcp:result <json>` |
//! | `fail <CODE> <command> <message>` | uncaught failed browser call at this line, exit 1 |
//! | anything else | ignored (comments, TypeScript-looking filler) |
//!
//! `check` fails (exit 1, `error: …`) when the file contains `SYNTAX_ERROR`.

use std::collections::HashMap;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => check(args.get(1)),
        Some("run") => run(&args[1..]),
        _ => {
            eprintln!("usage: bmcp-heal-fake-host run|check <file>");
            ExitCode::from(2)
        }
    }
}

fn check(file: Option<&String>) -> ExitCode {
    let Some(text) = file.and_then(|f| std::fs::read_to_string(f).ok()) else {
        eprintln!("error: cannot read the file");
        return ExitCode::from(2);
    };
    match text.lines().position(|l| l.contains("SYNTAX_ERROR")) {
        Some(n) => {
            eprintln!(
                "error: Expected ';', got 'SYNTAX_ERROR' at file:///x.ts:{}:1",
                n + 1
            );
            ExitCode::from(1)
        }
        None => ExitCode::SUCCESS,
    }
}

fn run(args: &[String]) -> ExitCode {
    let mut script = None;
    let mut inputs: HashMap<String, String> = HashMap::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--ws" | "--timeout-secs" => {
                it.next();
            }
            "--input" => {
                let pair = it.next().cloned().unwrap_or_default();
                let (k, v) = pair.split_once('=').unwrap_or((&pair, ""));
                inputs.insert(k.into(), v.into());
            }
            "--inputs-file" => {
                let text = it
                    .next()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_default();
                if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&text) {
                    inputs.extend(map);
                }
            }
            path => script = Some(path.to_string()),
        }
    }
    let Some(text) = script.and_then(|p| std::fs::read_to_string(p).ok()) else {
        eprintln!("error: script not found");
        return ExitCode::from(2);
    };
    for (n, raw) in text.lines().enumerate() {
        let mut line = raw.trim().to_string();
        for (k, v) in &inputs {
            line = line.replace(&format!("{{{{{k}}}}}"), v);
        }
        if let Some(json) = line.strip_prefix("result ") {
            println!("##bmcp:result {json}");
        } else if let Some(rest) = line.strip_prefix("fail ") {
            let mut parts = rest.splitn(3, ' ');
            let code = parts.next().unwrap_or("NOT_FOUND");
            let command = parts.next().unwrap_or("page.find");
            let message = parts.next().unwrap_or("failed");
            eprintln!("error: Uncaught Error: browser.{command} failed: {code}: {message}");
            eprintln!("    at file:///C:/data/scripts/script.ts:{}:1", n + 1);
            return ExitCode::from(1);
        }
    }
    ExitCode::SUCCESS
}
