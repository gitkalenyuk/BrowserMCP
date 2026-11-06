//! Turning an AI answer into a checked patch.
//!
//! 1. [`parse_answer`]: the model replies with one JSON object
//!    `{diff | patched_code, explanation, confidence, justification?}` (code fences and chatter
//!    around it are tolerated);
//! 2. [`apply_unified_diff`]: hunks are matched by their content, not by their line numbers
//!    (models often get those wrong), searching nearest to the hinted line;
//! 3. [`validate`]: the patch must change something, stay inside the failing step's region
//!    (±N lines) unless the model justified a wider change, and the patched TypeScript must
//!    transpile (`bmcp-script-host check <file>`).

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;
use similar::{DiffTag, TextDiff};

/// The model's answer, before validation.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Candidate {
    pub diff: Option<String>,
    pub patched_code: Option<String>,
    pub explanation: String,
    /// 0..=1 (clamped); 0.5 when the model gave none.
    pub confidence: f64,
    /// Why lines outside the failing region change (may be empty).
    pub justification: String,
}

/// A patch that passed every check.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    pub patched_code: String,
    /// Canonical unified diff (`--- a/<name>.ts` / `+++ b/<name>.ts`, 3 lines of context).
    pub diff: String,
    /// What was checked, e.g. `["applies", "region 12-42", "transpiles"]`.
    pub checks: Vec<String>,
}

/// The first JSON object in `text` (code fences, prose around it are skipped).
pub fn json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    // Try the longest candidate first, then shorter ones (a trailing `}` in prose).
    let mut end = text.len();
    while let Some(close) = text[..end].rfind('}') {
        if close < start {
            break;
        }
        if let Ok(value @ Value::Object(_)) = serde_json::from_str(&text[start..=close]) {
            return Some(value);
        }
        end = close;
    }
    None
}

/// Parse the model's reply (see the [module docs](self)).
pub fn parse_answer(text: &str) -> Result<Candidate, String> {
    let value = json_object(text).ok_or_else(|| {
        "the answer was not a JSON object with `diff` or `patched_code`".to_string()
    })?;
    let string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
    };
    let candidate = Candidate {
        diff: string("diff"),
        patched_code: string("patched_code"),
        explanation: string("explanation").unwrap_or_default(),
        confidence: value
            .get("confidence")
            .and_then(Value::as_f64)
            .unwrap_or(0.5)
            .clamp(0.0, 1.0),
        justification: string("justification").unwrap_or_default(),
    };
    if candidate.diff.is_none() && candidate.patched_code.is_none() {
        return Err("the answer has neither `diff` nor `patched_code`".into());
    }
    Ok(candidate)
}

/// One hunk: the lines it expects (context + removed) and what replaces them.
struct Hunk {
    /// 1-based start line from the `@@ -a,b` header (a hint only).
    hint: usize,
    old: Vec<String>,
    new: Vec<String>,
}

fn parse_hunks(diff: &str) -> Result<Vec<Hunk>, String> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for line in diff.lines() {
        if line.starts_with("@@") {
            // `@@ -12,7 +12,7 @@` → 12
            let hint = line
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.trim_start_matches('-').split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            hunks.push(Hunk {
                hint,
                old: Vec::new(),
                new: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue; // `---` / `+++` headers, prose before the first hunk
        };
        if line.starts_with("\\ No newline") {
            continue;
        }
        match line.chars().next() {
            Some('+') => hunk.new.push(line[1..].to_string()),
            Some('-') => hunk.old.push(line[1..].to_string()),
            Some(' ') => {
                hunk.old.push(line[1..].to_string());
                hunk.new.push(line[1..].to_string());
            }
            // An empty context line whose leading space got lost.
            None => {
                hunk.old.push(String::new());
                hunk.new.push(String::new());
            }
            _ => return Err(format!("unexpected line in the diff: {line:?}")),
        }
    }
    if hunks.is_empty() {
        return Err("the diff has no @@ hunks".into());
    }
    Ok(hunks)
}

/// Where `old` occurs in `lines[from..]`, nearest to `hint` (0-based). Trailing whitespace is
/// ignored when comparing.
fn find_block(lines: &[String], old: &[String], from: usize, hint: usize) -> Option<usize> {
    if old.is_empty() {
        return Some(hint.clamp(from, lines.len()));
    }
    let same = |a: &str, b: &str| a.trim_end() == b.trim_end();
    (from..=lines.len().saturating_sub(old.len()))
        .filter(|&at| old.iter().zip(&lines[at..]).all(|(o, l)| same(o, l)))
        .min_by_key(|&at| at.abs_diff(hint))
}

/// Apply a unified diff to `base`. Hunks are located by content (see the module docs).
pub fn apply_unified_diff(base: &str, diff: &str) -> Result<String, String> {
    let mut lines: Vec<String> = base.lines().map(str::to_string).collect();
    let mut from = 0;
    for (n, hunk) in parse_hunks(diff)?.into_iter().enumerate() {
        let hint = hunk.hint.saturating_sub(1);
        let at = find_block(&lines, &hunk.old, from, hint).ok_or_else(|| {
            format!(
                "hunk {} does not apply: its context/removed lines are not in the script",
                n + 1
            )
        })?;
        let new_len = hunk.new.len();
        lines.splice(at..at + hunk.old.len(), hunk.new);
        from = at + new_len;
    }
    let mut out = lines.join("\n");
    if base.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

/// Canonical unified diff of `old` → `new`.
pub fn unified_diff(old: &str, new: &str, name: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{name}.ts"), &format!("b/{name}.ts"))
        .to_string()
}

/// 1-based line ranges of `old` that the change touches (an insertion counts as the line it
/// goes before).
pub fn changed_lines(old: &str, new: &str) -> Vec<Range<usize>> {
    TextDiff::from_lines(old, new)
        .ops()
        .iter()
        .filter(|op| op.tag() != DiffTag::Equal)
        .map(|op| {
            let r = op.old_range();
            let start = r.start + 1;
            start..(r.end + 1).max(start + 1)
        })
        .collect()
}

/// What [`validate`] needs besides the candidate.
pub struct ValidateInput<'a> {
    pub script_name: &'a str,
    pub base: &'a str,
    /// The failing line (1-based), when known.
    pub line: Option<u32>,
    /// Allowed distance from `line`.
    pub radius: usize,
    /// `bmcp-script-host` (or a test double); `None` = transpile check skipped.
    pub host: Option<&'a Path>,
    /// Folder for the temporary file of the transpile check.
    pub tmp_dir: &'a Path,
}

/// Check a candidate (see the [module docs](self)). `Err` explains what is wrong, in words
/// that can go back to the model.
pub async fn validate(
    input: ValidateInput<'_>,
    candidate: &Candidate,
) -> Result<Validated, String> {
    let mut checks = Vec::new();
    let patched = match (&candidate.patched_code, &candidate.diff) {
        (Some(code), _) => code.clone(),
        (None, Some(diff)) => apply_unified_diff(input.base, diff)?,
        (None, None) => return Err("no patch given".into()),
    };
    checks.push("applies".to_string());
    if patched.trim_end() == input.base.trim_end() {
        return Err("the patch changes nothing".into());
    }

    if let Some(line) = input.line {
        let line = line as usize;
        let allowed = line.saturating_sub(input.radius).max(1)..line + input.radius + 1;
        let outside: Vec<String> = changed_lines(input.base, &patched)
            .into_iter()
            .filter(|r| r.start < allowed.start || r.end > allowed.end)
            .map(|r| {
                if r.end - r.start <= 1 {
                    r.start.to_string()
                } else {
                    format!("{}-{}", r.start, r.end - 1)
                }
            })
            .collect();
        if !outside.is_empty() && candidate.justification.trim().is_empty() {
            return Err(format!(
                "the patch changes line(s) {} outside the failing step's region (lines {}-{}); \
                 keep the fix near line {line}, or say why in `justification`",
                outside.join(", "),
                allowed.start,
                allowed.end - 1
            ));
        }
        checks.push(if outside.is_empty() {
            format!("region {}-{}", allowed.start, allowed.end - 1)
        } else {
            format!(
                "outside region, justified: {}",
                candidate.justification.trim()
            )
        });
    }

    match input.host {
        Some(host) => {
            transpile_check(host, input.tmp_dir, &patched).await?;
            checks.push("transpiles".to_string());
        }
        None => checks.push("transpile check skipped (no script host)".to_string()),
    }
    Ok(Validated {
        diff: unified_diff(input.base, &patched, input.script_name),
        patched_code: patched,
        checks,
    })
}

/// `bmcp-script-host check <tmp file>`; `Err` with the compiler message.
pub async fn transpile_check(host: &Path, tmp_dir: &Path, code: &str) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(tmp_dir).map_err(|e| format!("{}: {e}", tmp_dir.display()))?;
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let file: PathBuf = tmp_dir.join(format!("check-{}-{n}.ts", std::process::id()));
    std::fs::write(&file, code).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut cmd = tokio::process::Command::new(host);
    cmd.arg("check")
        .arg(&file)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = tokio::time::timeout(Duration::from_secs(30), cmd.output()).await;
    let _ = std::fs::remove_file(&file);
    let output = output
        .map_err(|_| "the transpile check timed out".to_string())?
        .map_err(|e| format!("cannot start {}: {e}", host.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("syntax error")
        .trim_start_matches("error: ");
    Err(format!("the patched script does not compile: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "line 1\nline 2\nconst b = find(\"Sign in\");\nline 4\nline 5\n";

    #[test]
    fn answers_are_parsed_leniently() {
        let text = "Here you go:\n```json\n{\"diff\": \"@@ -1 +1 @@\\n-a\\n+b\", \
                    \"explanation\": \"renamed\", \"confidence\": 1.7}\n```\nThanks {!}";
        let c = parse_answer(text).unwrap();
        assert_eq!(c.diff.as_deref(), Some("@@ -1 +1 @@\n-a\n+b"));
        assert_eq!(c.explanation, "renamed");
        assert_eq!(c.confidence, 1.0);
        assert!(parse_answer("no json here").is_err());
        assert!(parse_answer("{\"explanation\": \"x\"}").is_err());
        let full = parse_answer("{\"patched_code\": \"x\"}").unwrap();
        assert_eq!(full.patched_code.as_deref(), Some("x"));
        assert_eq!(full.confidence, 0.5);
    }

    #[test]
    fn diffs_apply_by_content_even_with_wrong_line_numbers() {
        let diff = "--- a/s.ts\n+++ b/s.ts\n@@ -40,3 +40,3 @@\n line 2\n-const b = find(\"Sign in\");\n+const b = find(\"Log in\");\n line 4\n";
        let out = apply_unified_diff(BASE, diff).unwrap();
        assert_eq!(
            out,
            "line 1\nline 2\nconst b = find(\"Log in\");\nline 4\nline 5\n"
        );
        let bad = "@@ -1 +1 @@\n-not in the file\n+x\n";
        assert!(
            apply_unified_diff(BASE, bad)
                .unwrap_err()
                .contains("does not apply")
        );
        assert!(apply_unified_diff(BASE, "just text").is_err());
    }

    #[test]
    fn changed_lines_and_canonical_diff() {
        let new = BASE.replace("Sign in", "Log in");
        assert_eq!(changed_lines(BASE, &new), vec![3..4]);
        let diff = unified_diff(BASE, &new, "login");
        assert!(diff.starts_with("--- a/login.ts\n+++ b/login.ts\n@@"));
        assert!(diff.contains("-const b = find(\"Sign in\");"));
        // Round trip: our own diff applies.
        assert_eq!(apply_unified_diff(BASE, &diff).unwrap(), new);
    }

    #[tokio::test]
    async fn validation_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let input = |line| ValidateInput {
            script_name: "s",
            base: BASE,
            line,
            radius: 1,
            host: None,
            tmp_dir: tmp.path(),
        };
        let near = Candidate {
            patched_code: Some(BASE.replace("Sign in", "Log in")),
            ..Default::default()
        };
        let ok = validate(input(Some(3)), &near).await.unwrap();
        assert_eq!(ok.checks[1], "region 2-4");

        // Line 5 is 2 lines away from line 3: outside a radius of 1.
        let far = Candidate {
            patched_code: Some(BASE.replace("line 5", "line five")),
            ..Default::default()
        };
        let err = validate(input(Some(3)), &far).await.unwrap_err();
        assert!(err.contains("outside the failing step's region"), "{err}");
        let justified = Candidate {
            justification: "the wait belongs after the click".into(),
            ..far.clone()
        };
        assert!(validate(input(Some(3)), &justified).await.is_ok());
        // Unknown failing line: no region check.
        assert!(validate(input(None), &far).await.is_ok());

        let noop = Candidate {
            patched_code: Some(BASE.into()),
            ..Default::default()
        };
        assert!(
            validate(input(Some(3)), &noop)
                .await
                .unwrap_err()
                .contains("nothing")
        );
    }
}
