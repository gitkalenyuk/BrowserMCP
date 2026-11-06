//! `cargo xtask <command>` — project automation without shell scripts.
//!
//! Commands:
//!   doctor              check that every build tool is available
//!   build [--release]   build the browser (finds CMake inside Visual Studio if needed)
//!   run [--release] [-- <browser args>]
//!   test [--all]        run Rust tests (--all includes the V8 script host)
//!   gen-sdk [--check]   generate the TypeScript SDK types from the command registry
//!                       (--check: fail if the committed files are stale; see gen_sdk.rs)
//!   bench --agent claude-code|codex --tasks all|<id,id>
//!                       MVP benchmark: 10 tasks on local fixture sites (see bench.rs)
//!
//! The first build downloads CEF (~500 MB) into target/ and copies its runtime files
//! next to browsermcp.exe automatically (done by the `cef-dll-sys` build script).

mod bench;
mod gen_sdk;

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => ("help", &[][..]),
    };
    let result = match cmd {
        "doctor" => doctor(),
        "build" => build(has_flag(rest, "--release")),
        "run" => run(has_flag(rest, "--release"), after_double_dash(rest)),
        // bmcp-script embeds V8 (212 MB download, ~6 min clean build): opt in with --all.
        "test" if has_flag(rest, "--all") => {
            build_env().and_then(|env| cargo(&["test", "--workspace"], &env))
        }
        "test" => build_env()
            .and_then(|env| cargo(&["test", "--workspace", "--exclude", "bmcp-script"], &env)),
        "gen-sdk" => gen_sdk::gen_sdk(has_flag(rest, "--check")),
        "bench" => bench::bench(rest),
        _ => {
            eprintln!(
                "usage: cargo xtask <doctor|build [--release]|run [--release] [-- args]|test|gen-sdk [--check]|bench --agent claude-code|codex --tasks all>"
            );
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

type Result<T = ()> = std::result::Result<T, String>;

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().take_while(|a| *a != "--").any(|a| a == flag)
}

fn after_double_dash(args: &[String]) -> Vec<String> {
    args.iter()
        .skip_while(|a| *a != "--")
        .skip(1)
        .cloned()
        .collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the workspace")
        .to_path_buf()
}

// ---------------------------------------------------------------- tools

fn on_path(program: &str) -> bool {
    let runs = |p: &str| {
        Command::new(p)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    // npm-installed tools (pnpm, ...) are .cmd shims on Windows.
    runs(program) || (cfg!(windows) && runs(&format!("{program}.cmd")))
}

/// CMake from PATH, otherwise the copy bundled with Visual Studio.
fn find_cmake() -> Option<PathBuf> {
    if on_path("cmake") {
        return Some(PathBuf::from("cmake"));
    }
    vswhere(&[
        "-find",
        r"Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe",
    ])
}

fn vswhere(query: &[&str]) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe =
        PathBuf::from(r"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe");
    let out = Command::new(exe)
        .args(["-latest", "-products", "*"])
        .args(query)
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(PathBuf::from)
}

fn has_msvc() -> bool {
    !cfg!(windows)
        || vswhere(&[
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .is_some()
}

fn doctor() -> Result {
    let checks: Vec<(&str, bool, &str)> = vec![
        (
            "cargo",
            on_path("cargo"),
            "install Rust from https://rustup.rs",
        ),
        (
            "MSVC C++ build tools",
            has_msvc(),
            "install VS Build Tools with the C++ workload",
        ),
        (
            "cmake",
            find_cmake().is_some(),
            "install CMake or the VS 'C++ CMake tools' component",
        ),
        (
            "ninja",
            on_path("ninja"),
            "install Ninja (winget install Ninja-build.Ninja)",
        ),
        (
            "node",
            on_path("node"),
            "install Node.js 22+ (needed from Phase 2 for the UI)",
        ),
        (
            "pnpm",
            on_path("pnpm"),
            "corepack enable (needed from Phase 2 for the UI)",
        ),
    ];
    let mut ok = true;
    for (name, found, fix) in &checks {
        if *found {
            println!("  ok       {name}");
        } else {
            ok = false;
            println!("  MISSING  {name}: {fix}");
        }
    }
    if let Some(cmake) = find_cmake() {
        println!("  cmake -> {}", cmake.display());
    }
    if ok {
        Ok(())
    } else {
        Err("some tools are missing".into())
    }
}

// ---------------------------------------------------------------- build / run

/// Environment needed by the cef-dll-sys build script.
fn build_env() -> Result<Vec<(String, String)>> {
    let mut envs = Vec::new();
    if !on_path("cmake") {
        let cmake = find_cmake().ok_or("CMake not found; run `cargo xtask doctor`")?;
        envs.push(("CMAKE".into(), cmake.display().to_string()));
    }
    Ok(envs)
}

fn cargo(args: &[&str], extra_env: &[(String, String)]) -> Result {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args(args)
        .current_dir(workspace_root())
        .envs(extra_env.iter().cloned())
        .status()
        .map_err(|e| format!("cannot start cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo {} failed", args.join(" ")))
    }
}

fn build(release: bool) -> Result {
    let envs = build_env()?;
    let mut args = vec!["build", "--package", "browsermcp"];
    if release {
        args.push("--release");
    }
    cargo(&args, &envs)
}

fn run(release: bool, browser_args: Vec<String>) -> Result {
    build(release)?;
    let profile = if release { "release" } else { "debug" };
    let exe = workspace_root()
        .join("target")
        .join(profile)
        .join(format!("browsermcp{}", env::consts::EXE_SUFFIX));
    let status = Command::new(&exe)
        .args(browser_args)
        .status()
        .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("browsermcp exited with {status}"))
    }
}
