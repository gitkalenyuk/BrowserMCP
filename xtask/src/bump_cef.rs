//! `cargo xtask bump-cef <version> [--dry-run]` — the monthly manual CEF update (plan: "CEF
//! update once a month by hand: cargo xtask bump-cef -> e2e tests").
//!
//! The `cef` crate is pinned exactly in the root Cargo.toml (`cef = { version = "=X" }`); each
//! crate release tracks one CEF build (`156.3.0+156.0.15` = crate 156.3.0, CEF 156.0.15). This
//! rewrites the pin, runs `cargo update -p cef -p cef-dll-sys`, refreshes the comment above the
//! pin with the CEF build from Cargo.lock and prints what to do next. On a failed update the old
//! Cargo.toml is restored.

use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{Result, workspace_root};

const PIN_PREFIX: &str = "cef = { version = \"=";
const COMMENT_MARKER: &str = "tracks a specific CEF build (";

pub fn bump_cef(args: &[String]) -> Result {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let wanted = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .ok_or("usage: cargo xtask bump-cef <cef crate version, e.g. 157.0.0> [--dry-run]")?;
    let version = normalize_version(wanted)?;

    let root = workspace_root();
    let manifest_path = root.join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
    let current = current_pin(&manifest).ok_or(format!(
        "no `{PIN_PREFIX}…\" }}` line in {}",
        manifest_path.display()
    ))?;
    if current == version {
        println!("cef is already pinned to ={version}; running cargo update anyway");
    } else {
        println!("cef: ={current} -> ={version}");
    }
    let updated = replace_pin(&manifest, &version);

    if dry_run {
        println!("dry run, nothing changed. Would:");
        println!("  set `cef = {{ version = \"={version}\" … }}` in Cargo.toml");
        println!("  run `cargo update -p cef -p cef-dll-sys`");
        println!("  update the CEF build in the comment above the pin from Cargo.lock");
        print_next_steps(&root, None);
        return Ok(());
    }

    fs::write(&manifest_path, &updated).map_err(|e| format!("cannot write Cargo.toml: {e}"))?;
    let lock_path = root.join("Cargo.lock");
    let old_lock = fs::read_to_string(&lock_path).ok();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    println!("> cargo update -p cef -p cef-dll-sys");
    let ok = Command::new(cargo)
        .args(["update", "-p", "cef", "-p", "cef-dll-sys"])
        .current_dir(&root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        let _ = fs::write(&manifest_path, &manifest);
        if let Some(lock) = old_lock {
            let _ = fs::write(&lock_path, lock);
        }
        return Err(format!(
            "cargo update failed (is cef {version} published? see https://crates.io/crates/cef/versions); \
             Cargo.toml and Cargo.lock restored"
        ));
    }

    // "157.0.0+157.0.4" -> CEF build "157.0.4".
    let lock = fs::read_to_string(&lock_path).unwrap_or_default();
    let cef_build =
        locked_version(&lock, "cef").and_then(|v| v.split_once('+').map(|(_, b)| b.to_owned()));
    if let Some(build) = &cef_build {
        let with_comment = replace_comment(&updated, build);
        if with_comment != updated {
            fs::write(&manifest_path, with_comment)
                .map_err(|e| format!("cannot write Cargo.toml: {e}"))?;
        }
        println!(
            "locked: cef {}",
            locked_version(&lock, "cef").unwrap_or_default()
        );
    }
    print_next_steps(&root, cef_build.as_deref());
    Ok(())
}

/// `157.0.0` stays, `157.0.0+157.0.4` loses the build metadata (cargo ignores it in a
/// requirement and warns), `=157.0.0` loses the `=`.
fn normalize_version(raw: &str) -> Result<String> {
    let v = raw.trim().trim_start_matches('=');
    let v = v.split_once('+').map_or(v, |(core, _)| core);
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
    {
        Ok(v.to_owned())
    } else {
        Err(format!(
            "{raw:?} is not a cef crate version like 157.0.0 (see https://crates.io/crates/cef/versions)"
        ))
    }
}

fn current_pin(manifest: &str) -> Option<String> {
    manifest.lines().find_map(|l| {
        let rest = l.trim_start().strip_prefix(PIN_PREFIX)?;
        Some(rest.split('"').next()?.to_owned())
    })
}

fn replace_pin(manifest: &str, version: &str) -> String {
    map_lines(manifest, |line| {
        let indent = &line[..line.len() - line.trim_start().len()];
        let rest = line.trim_start().strip_prefix(PIN_PREFIX)?;
        let (_, tail) = rest.split_once('"')?;
        Some(format!("{indent}{PIN_PREFIX}{version}\"{tail}"))
    })
}

/// `(156.0.15 / Chromium 156)` -> `(157.0.4 / Chromium 157)`.
fn replace_comment(manifest: &str, cef_build: &str) -> String {
    let major = cef_build.split('.').next().unwrap_or(cef_build);
    map_lines(manifest, |line| {
        let start = line.find(COMMENT_MARKER)? + COMMENT_MARKER.len();
        let end = start + line[start..].find(')')?;
        Some(format!(
            "{}{cef_build} / Chromium {major}{}",
            &line[..start],
            &line[end..]
        ))
    })
}

fn map_lines(text: &str, f: impl Fn(&str) -> Option<String>) -> String {
    let mut out: String = text
        .lines()
        .map(|l| f(l).unwrap_or_else(|| l.to_owned()))
        .collect::<Vec<_>>()
        .join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Version of `name` in Cargo.lock (`[[package]] name = "cef" version = "…"`).
fn locked_version(lock: &str, name: &str) -> Option<String> {
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == format!("name = \"{name}\"") {
            let next = lines.next()?;
            return next
                .trim()
                .strip_prefix("version = \"")
                .map(|v| v.trim_end_matches('"').to_owned());
        }
    }
    None
}

/// `ENGINE_CHROME_MAJOR` in bmcp-identity: the Chromium major the fingerprints assume.
fn identity_chrome_major(root: &Path) -> Option<u32> {
    let src = fs::read_to_string(root.join("crates/identity/src/identity.rs")).ok()?;
    src.lines().find_map(|l| {
        l.trim()
            .strip_prefix("pub const ENGINE_CHROME_MAJOR: u32 = ")?
            .trim_end_matches(';')
            .parse()
            .ok()
    })
}

fn print_next_steps(root: &Path, cef_build: Option<&str>) {
    println!();
    println!("Next steps:");
    println!(
        "  1. read the cef-rs changelog for API changes: https://github.com/tauri-apps/cef-rs/releases"
    );
    println!(
        "  2. cargo xtask build            (downloads the new CEF, ~175 MB, into CEF_PATH or target/)"
    );
    println!(
        "  3. cargo xtask test && cargo clippy --workspace --exclude bmcp-script --all-targets -- -D warnings"
    );
    println!("  4. cargo xtask run              (smoke test: UI strip, a page, Ctrl+K palette)");
    println!("  5. cargo xtask bench --agent claude-code --tasks all   (e2e on the fixture sites)");
    println!("  6. cargo xtask dist             (portable package; check the sizes it prints)");
    println!("  7. commit Cargo.toml + Cargo.lock (CI's CEF cache is keyed by Cargo.lock)");
    let new_major: Option<u32> = cef_build
        .and_then(|b| b.split('.').next())
        .and_then(|m| m.parse().ok());
    match (new_major, identity_chrome_major(root)) {
        (Some(new), Some(old)) if new != old => println!(
            "  !  Chromium major changed {old} -> {new}: update ENGINE_CHROME_MAJOR in \
             crates/identity/src/identity.rs (client hints / fingerprints)"
        ),
        (Some(_), _) => {}
        (None, _) => println!(
            "  Also: if the Chromium major changes, update ENGINE_CHROME_MAJOR in \
             crates/identity/src/identity.rs"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "[workspace.dependencies]\n\
        # Pinned exactly: crate version tracks a specific CEF build (156.0.15 / Chromium 156).\n\
        cef = { version = \"=156.3.0\", default-features = false }\n\
        anyhow = \"1\"\n";

    #[test]
    fn versions() {
        assert_eq!(normalize_version("157.0.0").unwrap(), "157.0.0");
        assert_eq!(normalize_version("=157.1.2+157.0.4").unwrap(), "157.1.2");
        assert!(normalize_version("157").is_err());
        assert!(normalize_version("157.x.0").is_err());
    }

    #[test]
    fn pin_and_comment() {
        assert_eq!(current_pin(MANIFEST).as_deref(), Some("156.3.0"));
        let bumped = replace_pin(MANIFEST, "157.0.0");
        assert!(bumped.contains("cef = { version = \"=157.0.0\", default-features = false }\n"));
        assert!(bumped.contains("anyhow = \"1\"\n"));
        let commented = replace_comment(&bumped, "157.0.4");
        assert!(commented.contains("tracks a specific CEF build (157.0.4 / Chromium 157)."));
        assert_eq!(commented.lines().count(), MANIFEST.lines().count());
    }

    #[test]
    fn lock_versions() {
        let lock = "[[package]]\nname = \"cef\"\nversion = \"157.0.0+157.0.4\"\n\n\
                    [[package]]\nname = \"cef-dll-sys\"\nversion = \"157.0.0+157.0.4\"\n";
        assert_eq!(
            locked_version(lock, "cef").as_deref(),
            Some("157.0.0+157.0.4")
        );
        assert_eq!(locked_version(lock, "missing"), None);
    }
}
