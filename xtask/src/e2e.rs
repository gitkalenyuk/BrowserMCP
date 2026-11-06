//! `cargo xtask e2e [--filter <name>] [--all] [--no-build]`
//!
//! Builds the browser, then runs the end-to-end suite `tests/e2e/` (vitest + the TypeScript
//! SDK) against `browsermcp --headless`. The suite's global setup starts the browser and a
//! fixture server itself and stops exactly those two processes (tests/e2e/setup/global.ts).
//! `pnpm -r test` only type-checks the suite; it runs through this command (or
//! `pnpm -C tests/e2e run e2e` with an existing build).
//!
//! * `--filter <name>`: only test files whose path contains `<name>` (e.g. `net`, `api`);
//! * `--all`: also build `bmcp-script` (the V8 script host, slow first build) so the record →
//!   script and mass-run tests run instead of being skipped;
//! * `--no-build`: use the binaries that are there.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Result, build_env, cargo, has_flag, workspace_root};

pub fn e2e(args: &[String]) -> Result {
    let root = workspace_root();
    if !has_flag(args, "--no-build") {
        crate::build(false)?;
        if has_flag(args, "--all") {
            cargo(&["build", "--package", "bmcp-script"], &build_env()?)?;
        }
    }
    let exe = target_dir(&root)
        .join("debug")
        .join(format!("browsermcp{}", std::env::consts::EXE_SUFFIX));
    if !exe.exists() {
        return Err(format!("{} not found; build it first", exe.display()));
    }

    let suite = root.join("tests").join("e2e");
    if !suite.join("node_modules").is_dir() {
        pnpm(&root, &["install", "--frozen-lockfile"], &exe)?;
    }
    let mut run = vec!["-C", "tests/e2e", "run", "e2e"];
    let filter = value_of(args, "--filter");
    if let Some(filter) = filter.as_deref() {
        run.push(filter);
    }
    pnpm(&root, &run, &exe)
}

fn value_of(args: &[String], flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    args.get(at + 1).cloned()
}

/// `CARGO_TARGET_DIR` when set, else `<root>/target`.
fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"))
}

fn pnpm(root: &Path, args: &[&str], exe: &Path) -> Result {
    // npm-installed tools are .cmd shims on Windows.
    let program = if cfg!(windows) { "pnpm.cmd" } else { "pnpm" };
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        // The suite finds the browser here (it also understands CARGO_TARGET_DIR).
        .env("BMCP_EXE", exe)
        .status()
        .map_err(|e| format!("cannot start pnpm ({e}); run `corepack enable`"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("pnpm {} failed", args.join(" ")))
    }
}
