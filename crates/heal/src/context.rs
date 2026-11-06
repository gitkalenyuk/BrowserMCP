//! What the AI sees when it writes a patch, and the prompt built from it.
//!
//! Context of one heal request: the script source, the failing line ±N (marked), the error
//! (code, command, message), the page snapshot stored when the row broke, the semantic
//! locators the script uses (role / name / text, as recorded by `record.codegen` or written by
//! hand) and the problems of earlier attempts.

use std::fmt::Write as _;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// Longest snapshot text put into a prompt.
const MAX_SNAPSHOT_CHARS: usize = 14_000;
/// Longest script put into a prompt (bigger scripts are cut around the failing line).
const MAX_SCRIPT_CHARS: usize = 40_000;

/// The instructions (system prompt) for patch proposals.
pub const SYSTEM: &str = "\
You repair BrowserMCP automation scripts. A script is TypeScript run by bmcp-script-host with \
these globals: `browser.<ns>.<method>(args)` calls a browser command (e.g. browser.page.find \
{tab?, role?, name?, text?, regex?} returns {total, text} where text has lines like \
`- button \"Sign in\" @3:e7`; browser.page.act {tab?, ref, action, value?}), `inputs.<key>` \
(strings), `browser.output({...})`, `sleep(ms)`.

A mass run paused because one step could no longer find or use its page element: the page \
changed. Fix ONLY that step: update its locator (role / accessible name / text / regex) to \
what the page snapshot shows now, or add a short wait if the element appears late. Keep every \
other line exactly as it is. Never hard-code input values, passwords or other secrets.

Reply with ONE JSON object and nothing else:
{\"diff\": \"<unified diff of the script: @@ hunks with 3 lines of context>\", \
\"explanation\": \"<one or two sentences: what changed on the page and what you changed>\", \
\"confidence\": <number 0..1: how sure you are the patched script now works on this page>, \
\"justification\": \"<only when you change lines more than 15 lines away from the failing \
line: why>\"}
Instead of \"diff\" you may send \"patched_code\" with the whole fixed script.";

/// Everything known about one broken row.
#[derive(Debug, Clone, Default)]
pub struct HealContext {
    pub run: String,
    pub row: u64,
    /// The script version the row ran (e.g. `login` or `login.v2`).
    pub script: String,
    pub code: String,
    /// `{code, command?, message, line?}` from the run's heal request.
    pub error: Value,
    pub line: Option<u32>,
    /// Page snapshot taken when the row broke (may be absent).
    pub snapshot: Option<String>,
}

/// `act("button", "Sign in", …)`, `{ role: "button", name: "Sign in" }`, `text: "…"`,
/// `regex: "…"`, `css: "…"`: lines with semantic locators.
static LOCATOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?x)
        \b(?:role|name|text|regex|css|label)\s*:\s*["'`]
        | \b(?:act|actBy|locate|find)\s*\(\s*["'`{]
        | \bpage\.(?:find|act|act_batch)\b"#,
    )
    .unwrap()
});

/// The lines of `code` that locate elements, as `(line number, trimmed text)`.
pub fn locators(code: &str) -> Vec<(usize, String)> {
    code.lines()
        .enumerate()
        .filter(|(_, l)| {
            let t = l.trim_start();
            !t.starts_with("//") && !t.starts_with('*') && LOCATOR.is_match(l)
        })
        .map(|(n, l)| (n + 1, l.trim().chars().take(200).collect()))
        .take(60)
        .collect()
}

/// Lines `line ± radius` with numbers; the failing line is marked `>>`.
pub fn region(code: &str, line: u32, radius: usize) -> String {
    let line = line as usize;
    let first = line.saturating_sub(radius).max(1);
    let mut out = String::new();
    for (n, text) in code.lines().enumerate() {
        let n = n + 1;
        if n < first || n > line + radius {
            continue;
        }
        let mark = if n == line { ">>" } else { "  " };
        let _ = writeln!(out, "{mark}{n:>4} | {text}");
    }
    out
}

fn cut(text: &str, max: usize) -> String {
    bmcp_ai::model::truncate(text, max)
}

impl HealContext {
    /// The user prompt for one attempt. `history` = problems of earlier attempts (validation
    /// errors, a reviewer's feedback), oldest first.
    pub fn prompt(&self, radius: usize, history: &[String]) -> String {
        let mut p = String::new();
        let e = &self.error;
        let _ = writeln!(p, "## Error (run {}, row {})", self.run, self.row);
        let _ = writeln!(p, "code: {}", e["code"].as_str().unwrap_or("?"));
        if let Some(command) = e["command"].as_str() {
            let _ = writeln!(p, "command: {command}");
        }
        if let Some(line) = self.line {
            let _ = writeln!(p, "line: {line}");
        }
        let _ = writeln!(p, "message: {}", e["message"].as_str().unwrap_or(""));
        for key in ["hint", "candidates"] {
            if let Some(v) = e.get(key).filter(|v| !v.is_null()) {
                let _ = writeln!(p, "{key}: {v}");
            }
        }

        if let Some(line) = self.line {
            let _ = writeln!(
                p,
                "\n## Failing region of {}.ts (>> marks line {line})\n```ts\n{}```",
                self.script,
                region(&self.code, line, radius)
            );
        }

        let locators = locators(&self.code);
        if !locators.is_empty() {
            let _ = writeln!(p, "\n## Element locators in the script");
            for (n, text) in locators {
                let _ = writeln!(p, "{n:>4}: {text}");
            }
        }

        let _ = writeln!(p, "\n## Page snapshot when the row broke");
        match &self.snapshot {
            Some(s) => {
                let _ = writeln!(p, "{}", cut(s, MAX_SNAPSHOT_CHARS));
            }
            None => {
                let _ = writeln!(
                    p,
                    "(none: no snapshot could be taken; rely on the error and the script)"
                );
            }
        }

        let _ = writeln!(
            p,
            "\n## Full script {}.ts (line numbers are not part of the file)\n```ts",
            self.script
        );
        let numbered: String = self
            .code
            .lines()
            .enumerate()
            .map(|(n, l)| format!("{:>4} | {l}\n", n + 1))
            .collect();
        let _ = writeln!(p, "{}```", cut(&numbered, MAX_SCRIPT_CHARS));

        if !history.is_empty() {
            let _ = writeln!(
                p,
                "\n## Earlier attempts were not accepted; fix these problems"
            );
            for (n, h) in history.iter().enumerate() {
                let _ = writeln!(p, "{}. {h}", n + 1);
            }
        }
        let _ = writeln!(p, "\nAnswer with the JSON object only.");
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const CODE: &str = "// login\nconst url = inputs.url;\nawait act(\"textbox\", \"Email\", \"fill\", inputs.email);\nconst f = await browser.page.find({ tab, role: \"button\", name: \"Sign in\" });\nconsole.log(f);\n";

    #[test]
    fn locators_and_region() {
        let found = locators(CODE);
        assert_eq!(
            found.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            vec![3, 4]
        );
        let r = region(CODE, 4, 1);
        assert_eq!(r.lines().count(), 3);
        assert!(r.contains(">>   4 | const f"));
        assert!(r.contains("     3 | await act"));
    }

    #[test]
    fn prompt_has_every_part() {
        let ctx = HealContext {
            run: "run-1".into(),
            row: 2,
            script: "login".into(),
            code: CODE.into(),
            error: json!({"code": "NOT_FOUND", "command": "page.find", "message": "no Sign in"}),
            line: Some(4),
            snapshot: Some("- button \"Log in\" @3:e7".into()),
        };
        let p = ctx.prompt(15, &["the diff did not apply".into()]);
        for part in [
            "code: NOT_FOUND",
            "command: page.find",
            ">>   4 |",
            "Element locators",
            "- button \"Log in\" @3:e7",
            "Full script login.ts",
            "1. the diff did not apply",
        ] {
            assert!(p.contains(part), "missing {part:?} in\n{p}");
        }
    }
}
