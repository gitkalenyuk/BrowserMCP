//! `cargo xtask dist` — the portable Windows build (plan: "portable: everything in ./data next
//! to the exe", "distribution: for ourselves").
//!
//! ```text
//! cargo xtask dist [--skip-script-host] [--out <dir>] [--locales en-US,uk|all] [--no-build] [--no-zip]
//! ```
//!
//! 1. `pnpm -C ui build` (so `ui/dist` is embedded into browsermcp.exe by `crates/cef/build.rs`),
//! 2. `cargo build --release` of `browsermcp` and `bmcp-script` (`bmcp-script-host.exe`, V8),
//! 3. assembles `<out>/BrowserMCP-<version>-win64/`:
//!
//! ```text
//! browsermcp.exe  bmcp-script-host.exe
//! libcef.dll chrome_elf.dll *.pak icudtl.dat v8_context_snapshot.bin    CEF runtime
//! d3dcompiler_47.dll dxcompiler.dll vk_swiftshader* vulkan-1.dll        GPU / software GL
//! locales/<kept>.pak                                                    en-US + uk by default
//! data/                    empty: profiles, db, logs… are created here on first start
//! README.txt  THIRD_PARTY.txt  LICENSES/
//! ```
//!
//! 4. zips it into `<out>/BrowserMCP-<version>-win64.zip` and prints the sizes.
//!
//! The CEF runtime files are taken from the CEF distribution the build used (the
//! `cargo::metadata=CEF_DIR=…` line of the `cef-dll-sys` build script), not from
//! `target/release`: `cef-dll-sys` copies *everything* there (libcef.lib, bootstrap.exe,
//! CMakeLists.txt…) and never removes files of an older CEF.

mod licenses;

use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::{Result, build_env, cargo, on_path, workspace_root};

/// Locales kept by default (Chromium falls back to en-US when the system locale has no pak).
const DEFAULT_LOCALES: &[&str] = &["en-US", "uk"];

/// The fallback page `crates/cef/build.rs` embeds when `ui/dist` is missing.
const UI_PLACEHOLDER_MARKER: &[u8] = b"The browser UI was not built.";

#[derive(Debug, PartialEq, Eq)]
struct Options {
    skip_script_host: bool,
    out: Option<PathBuf>,
    /// `None` = every locale.
    locales: Option<Vec<String>>,
    /// Package what is already in `target/release` (no UI or cargo build).
    no_build: bool,
    no_zip: bool,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut opts = Options {
        skip_script_host: false,
        out: None,
        locales: Some(DEFAULT_LOCALES.iter().map(|s| s.to_string()).collect()),
        no_build: false,
        no_zip: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) => (n, Some(v.to_owned())),
            None => (arg.as_str(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or(format!("{name} needs a value"))
        };
        match name {
            "--skip-script-host" => opts.skip_script_host = true,
            "--no-build" => opts.no_build = true,
            "--no-zip" => opts.no_zip = true,
            "--out" => opts.out = Some(PathBuf::from(value()?)),
            "--locales" => {
                let v = value()?;
                opts.locales = if v.eq_ignore_ascii_case("all") {
                    None
                } else {
                    let mut list: Vec<String> = v
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    // Chromium's last-resort fallback: always keep it.
                    if !list.iter().any(|l| l == "en-US") {
                        list.insert(0, "en-US".into());
                    }
                    Some(list)
                };
            }
            other => {
                return Err(format!(
                    "unknown dist option {other:?}; usage: cargo xtask dist [--skip-script-host] \
                     [--out <dir>] [--locales en-US,uk|all] [--no-build] [--no-zip]"
                ));
            }
        }
    }
    Ok(opts)
}

pub fn dist(args: &[String]) -> Result {
    if !cfg!(windows) {
        return Err("`cargo xtask dist` builds the Windows package; macOS is plan Phase 11".into());
    }
    let opts = parse(args)?;
    let root = workspace_root();
    let meta = cargo_metadata(&root)?;
    let version = package_version(&meta, "browsermcp")?;
    let target_dir = PathBuf::from(
        meta["target_directory"]
            .as_str()
            .ok_or("cargo metadata: no target_directory")?,
    );
    let release = target_dir.join("release");

    if !opts.no_build {
        build_ui(&root)?;
        let envs = build_env()?;
        let mut args = vec!["build", "--release", "--package", "browsermcp"];
        if !opts.skip_script_host {
            args.extend(["--package", "bmcp-script"]);
        }
        cargo(&args, &envs)?;
    }

    let exe = release.join("browsermcp.exe");
    let host = release.join("bmcp-script-host.exe");
    if !exe.is_file() {
        return Err(format!("{} not found (drop --no-build)", exe.display()));
    }
    if !opts.skip_script_host && !host.is_file() {
        return Err(format!(
            "{} not found (drop --no-build, or pass --skip-script-host)",
            host.display()
        ));
    }
    check_ui_embedded(&exe)?;
    let cef_dir = find_cef_dir(&release)?;
    let cef_version = cef_version(&cef_dir).unwrap_or_else(|| "unknown".into());

    let name = format!("BrowserMCP-{version}-win64");
    let out = match &opts.out {
        Some(dir) => std::path::absolute(dir).map_err(|e| format!("--out: {e}"))?,
        None => root.join("dist"),
    };
    let stage = out.join(&name);
    if stage.exists() {
        fs::remove_dir_all(&stage).map_err(|e| format!("cannot clear {}: {e}", stage.display()))?;
    }
    mkdir(&stage)?;
    println!("assembling {}", stage.display());

    copy(&exe, &stage.join("browsermcp.exe"))?;
    if !opts.skip_script_host {
        copy(&host, &stage.join("bmcp-script-host.exe"))?;
    }
    let (kept, dropped) = copy_cef_runtime(&cef_dir, &stage, opts.locales.as_deref())?;
    println!(
        "  CEF {cef_version}: runtime from {}; locales kept {kept}, dropped {dropped}",
        cef_dir.display()
    );
    // The portable data folder: DataDir::resolve() picks `data/` next to the exe.
    mkdir(&stage.join("data"))?;

    let licenses_dir = stage.join("LICENSES");
    mkdir(&licenses_dir)?;
    copy(
        &cef_dir.join("CREDITS.html"),
        &licenses_dir.join("Chromium-CREDITS.html"),
    )?;
    let cef_license = licenses::cef_license(&cef_dir)?;
    write(&licenses_dir.join("CEF-LICENSE.txt"), &cef_license)?;
    copy(
        &root.join("crates/adblock/assets/Readability-LICENSE.md"),
        &licenses_dir.join("Readability-LICENSE.md"),
    )?;
    let roots: Vec<&str> = if opts.skip_script_host {
        vec!["browsermcp"]
    } else {
        vec!["browsermcp", "bmcp-script"]
    };
    let third_party = licenses::third_party(&root, &meta, &roots, &version, &cef_version)?;
    write(&stage.join("THIRD_PARTY.txt"), &third_party)?;
    write(
        &stage.join("README.txt"),
        &readme(&version, &cef_version, opts.skip_script_host),
    )?;

    print_sizes(&stage)?;
    if !opts.no_zip {
        let zip = out.join(format!("{name}.zip"));
        zip_dir(&stage, &name, &zip)?;
        let size = fs::metadata(&zip).map_err(|e| e.to_string())?.len();
        println!("zip: {} ({})", zip.display(), human(size));
    }
    Ok(())
}

// ---------------------------------------------------------------- build steps

fn pnpm() -> Result<&'static str> {
    if on_path("pnpm") {
        // `on_path` also accepts the `.cmd` shim; Command needs the exact file name.
        let plain = Command::new("pnpm").arg("--version").output().is_ok();
        Ok(if plain { "pnpm" } else { "pnpm.cmd" })
    } else {
        Err("pnpm not found (corepack enable); it builds ui/dist".into())
    }
}

fn run_tool(program: &str, args: &[&str], dir: &Path) -> Result {
    println!("> {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} {} failed ({status})", args.join(" ")))
    }
}

/// `pnpm install` (only when the workspace has never been installed) + `pnpm -C ui build`.
fn build_ui(root: &Path) -> Result {
    let pnpm = pnpm()?;
    if !root.join("node_modules").is_dir() || !root.join("ui/node_modules").is_dir() {
        run_tool(pnpm, &["install", "--frozen-lockfile"], root)?;
    }
    run_tool(pnpm, &["-C", "ui", "build"], root)?;
    if !root.join("ui/dist/index.html").is_file() {
        return Err("pnpm -C ui build did not produce ui/dist/index.html".into());
    }
    Ok(())
}

/// The release exe must carry the real UI, not the "UI not built" placeholder.
fn check_ui_embedded(exe: &Path) -> Result {
    let bytes = fs::read(exe).map_err(|e| format!("cannot read {}: {e}", exe.display()))?;
    if bytes
        .windows(UI_PLACEHOLDER_MARKER.len())
        .any(|w| w == UI_PLACEHOLDER_MARKER)
    {
        return Err(format!(
            "{} embeds the placeholder UI: run `pnpm -C ui build` and rebuild (drop --no-build)",
            exe.display()
        ));
    }
    Ok(())
}

fn cargo_metadata(root: &Path) -> Result<Value> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            "x86_64-pc-windows-msvc",
        ])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata output: {e}"))
}

fn package_version(meta: &Value, name: &str) -> Result<String> {
    meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == name && p["source"].is_null())
        .and_then(|p| p["version"].as_str())
        .map(str::to_owned)
        .ok_or(format!("package {name} not in cargo metadata"))
}

/// The CEF distribution folder of the last release build of `cef-dll-sys`.
fn find_cef_dir(release: &Path) -> Result<PathBuf> {
    let build = release.join("build");
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&build).map_err(|e| format!("{}: {e}", build.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("cef-dll-sys-")
        {
            continue;
        }
        let output = entry.path().join("output");
        let (Ok(text), Ok(meta)) = (fs::read_to_string(&output), fs::metadata(&output)) else {
            continue;
        };
        let dir = text.lines().find_map(|l| {
            l.strip_prefix("cargo::metadata=CEF_DIR=")
                .or_else(|| l.strip_prefix("cargo:CEF_DIR="))
        });
        if let Some(dir) = dir {
            let dir = PathBuf::from(dir.trim());
            if dir.join("libcef.dll").is_file() {
                found.push((meta.modified().map_err(|e| e.to_string())?, dir));
            }
        }
    }
    found.sort();
    found.pop().map(|(_, dir)| dir).ok_or(format!(
        "no CEF_DIR in {}/cef-dll-sys-*/output (build first)",
        build.display()
    ))
}

/// `156.0.15+gcf671c0+chromium-156.0.8078.80` from `include/cef_version.h`.
fn cef_version(cef_dir: &Path) -> Option<String> {
    let header = fs::read_to_string(cef_dir.join("include/cef_version.h")).ok()?;
    header.lines().find_map(|l| {
        l.strip_prefix("#define CEF_VERSION ")
            .map(|v| v.trim().trim_matches('"').to_owned())
    })
}

// ---------------------------------------------------------------- assembling

/// Files of the CEF distribution the browser loads at run time.
fn is_cef_runtime_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // .dll: libcef, chrome_elf, d3dcompiler_47, dxcompiler/dxil, vk_swiftshader, vulkan-1.
    // .pak/.dat/.bin: resources, icudtl.dat, v8_context_snapshot.bin (snapshot_blob.bin in
    // older builds). The ICD json lets Vulkan find SwiftShader (software WebGL fallback).
    [".dll", ".pak", ".dat", ".bin"]
        .iter()
        .any(|ext| lower.ends_with(ext))
        || lower == "vk_swiftshader_icd.json"
}

/// `de.pak`, `de_FEMININE.pak`… belong to locale `de`.
fn locale_of(file: &str) -> &str {
    let stem = file.strip_suffix(".pak").unwrap_or(file);
    stem.split_once('_').map_or(stem, |(base, _)| base)
}

fn copy_cef_runtime(
    cef_dir: &Path,
    stage: &Path,
    locales: Option<&[String]>,
) -> Result<(usize, usize)> {
    let mut copied = 0;
    for entry in read_dir_sorted(cef_dir)? {
        let name = entry.file_name().unwrap_or_default().to_string_lossy();
        if entry.is_file() && is_cef_runtime_file(&name) {
            copy(&entry, &stage.join(&*name))?;
            copied += 1;
        }
    }
    if !stage.join("libcef.dll").is_file() || copied < 5 {
        return Err(format!(
            "{} does not look like a CEF distribution",
            cef_dir.display()
        ));
    }

    let src = cef_dir.join("locales");
    let dest = stage.join("locales");
    mkdir(&dest)?;
    let (mut kept, mut dropped) = (0, 0);
    let mut seen = std::collections::BTreeSet::new();
    for entry in read_dir_sorted(&src)? {
        let name = entry.file_name().unwrap_or_default().to_string_lossy();
        let locale = locale_of(&name);
        if locales.is_none_or(|list| list.iter().any(|l| l.eq_ignore_ascii_case(locale))) {
            copy(&entry, &dest.join(&*name))?;
            seen.insert(locale.to_ascii_lowercase());
            kept += 1;
        } else {
            dropped += 1;
        }
    }
    for wanted in locales.unwrap_or_default() {
        if !seen.contains(&wanted.to_ascii_lowercase()) {
            return Err(format!(
                "locale {wanted:?} is not in {} (names are like en-US, uk, pt-BR)",
                src.display()
            ));
        }
    }
    Ok((kept, dropped))
}

fn readme(version: &str, cef_version: &str, skip_script_host: bool) -> String {
    let script_host = if skip_script_host {
        "NOTE: this package was built with --skip-script-host: script.run / mass runs need\n\
         bmcp-script-host.exe next to browsermcp.exe.\n\n"
    } else {
        ""
    };
    format!(
        r#"BrowserMCP {version} (Windows x64, portable)
Chromium via CEF {cef_version}

A browser that people and AI agents both drive. Every browser function is a command,
exposed over MCP, REST (/v1/cmd/<name>) and WebSocket (/v1/ws).

{script_host}RUN
  Unzip anywhere you can write to (not "Program Files") and start browsermcp.exe.
  Everything is stored in the data\ folder next to it: profiles, cookies, history,
  scripts, recordings, logs. Copy or move the whole folder to move the browser; delete it
  to uninstall. Nothing is written to %APPDATA% or the registry.

  browsermcp.exe --help                    all options
  browsermcp.exe https://github.com --profile work
  browsermcp.exe --data-dir D:\other-data  keep the data somewhere else
  browsermcp.exe --api-port 0              pick a free API port (default 47800)

  Only one instance can use a data folder at a time. data\runtime\endpoint.json tells
  tools where the running instance listens.

CONNECT CLAUDE CODE (or any MCP client)
  While the browser runs, MCP is served at http://127.0.0.1:47800/mcp (local only;
  requests from web pages are rejected).

    claude mcp add --transport http browsermcp http://127.0.0.1:47800/mcp

  Clients that only speak stdio (this also starts the browser when it is not running):

    claude mcp add browsermcp -- C:\path\to\BrowserMCP\browsermcp.exe mcp-stdio

  More tool groups: http://127.0.0.1:47800/mcp?toolsets=core,system,net
  REST: GET http://127.0.0.1:47800/v1/health, POST /v1/cmd/tabs.list, /openapi.json

FILES
  browsermcp.exe          the browser (also the Chromium renderer/GPU/utility processes)
  bmcp-script-host.exe    runs TypeScript automation scripts (V8); must stay next to
                          browsermcp.exe
  libcef.dll, *.pak, ...  Chromium Embedded Framework runtime
  locales\                UI languages kept in this build; others fall back to en-US
  data\                   your data (created on first start)
  THIRD_PARTY.txt         third-party software and licenses; full texts in LICENSES\

VIDEO RECORDING
  Recording to video needs ffmpeg: put ffmpeg.exe next to browsermcp.exe, set
  BMCP_FFMPEG=<path>, or install it (winget install Gyan.FFmpeg).

NOTES
  The Chromium sandbox is off in this build (see docs/release.md in the source).
  Windows SmartScreen may warn on first start: the executables are not code-signed.
"#
    )
}

// ---------------------------------------------------------------- zip and sizes

fn zip_dir(stage: &Path, top: &str, zip_path: &Path) -> Result {
    use zip::write::SimpleFileOptions;
    let file = fs::File::create(zip_path)
        .map_err(|e| format!("cannot create {}: {e}", zip_path.display()))?;
    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let err = |e: zip::result::ZipError| format!("zip: {e}");

    let mut stack = vec![(stage.to_path_buf(), top.to_owned())];
    while let Some((dir, prefix)) = stack.pop() {
        zip.add_directory(format!("{prefix}/"), options)
            .map_err(err)?;
        // Reverse so the stack pops directories in name order.
        let entries = read_dir_sorted(&dir)?;
        for path in entries.iter().rev() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            let entry_name = format!("{prefix}/{name}");
            if path.is_dir() {
                stack.push((path.clone(), entry_name));
            }
        }
        for path in &entries {
            if path.is_file() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                zip.start_file(format!("{prefix}/{name}"), options)
                    .map_err(err)?;
                let mut src = fs::File::open(path).map_err(|e| e.to_string())?;
                std::io::copy(&mut src, &mut zip)
                    .map_err(|e| format!("zip {}: {e}", path.display()))?;
            }
        }
    }
    zip.finish()
        .map_err(err)?
        .flush()
        .map_err(|e| e.to_string())
}

fn print_sizes(stage: &Path) -> Result {
    let mut rows: Vec<(u64, String)> = Vec::new();
    for path in read_dir_sorted(stage)? {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let size = tree_size(&path)?;
        let label = if path.is_dir() {
            format!("{name}\\")
        } else {
            name.into_owned()
        };
        rows.push((size, label));
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    for (size, label) in &rows {
        println!("  {:>10}  {label}", human(*size));
    }
    println!(
        "  {:>10}  total ({})",
        human(rows.iter().map(|r| r.0).sum()),
        stage.display()
    );
    Ok(())
}

fn tree_size(path: &Path) -> Result<u64> {
    if path.is_file() {
        return fs::metadata(path)
            .map(|m| m.len())
            .map_err(|e| e.to_string());
    }
    let mut total = 0;
    for child in read_dir_sorted(path)? {
        total += tree_size(&child)?;
    }
    Ok(total)
}

fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

// ---------------------------------------------------------------- fs helpers

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("cannot read {}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    paths.sort();
    Ok(paths)
}

fn mkdir(dir: &Path) -> Result {
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))
}

fn copy(from: &Path, to: &Path) -> Result {
    fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} -> {}: {e}", from.display(), to.display()))
}

fn write(path: &Path, text: &str) -> Result {
    // CRLF: the text files are read with Notepad on Windows.
    let text = text.replace("\r\n", "\n").replace('\n', "\r\n");
    fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn default_options() {
        let opts = parse(&[]).unwrap();
        assert!(!opts.skip_script_host && !opts.no_build && !opts.no_zip);
        assert_eq!(opts.out, None);
        assert_eq!(opts.locales, Some(args(&["en-US", "uk"])));
    }

    #[test]
    fn options() {
        let opts = parse(&args(&[
            "--skip-script-host",
            "--out",
            "D:/x",
            "--locales=de,pt-BR",
            "--no-zip",
        ]))
        .unwrap();
        assert!(opts.skip_script_host && opts.no_zip);
        assert_eq!(opts.out, Some(PathBuf::from("D:/x")));
        // en-US is always kept: Chromium falls back to it.
        assert_eq!(opts.locales, Some(args(&["en-US", "de", "pt-BR"])));
        assert_eq!(parse(&args(&["--locales", "all"])).unwrap().locales, None);
        assert!(parse(&args(&["--out"])).is_err());
        assert!(parse(&args(&["--bogus"])).is_err());
    }

    #[test]
    fn runtime_files_and_locales() {
        for keep in [
            "libcef.dll",
            "chrome_elf.dll",
            "resources.pak",
            "chrome_200_percent.pak",
            "icudtl.dat",
            "v8_context_snapshot.bin",
            "vk_swiftshader_icd.json",
            "vulkan-1.dll",
        ] {
            assert!(is_cef_runtime_file(keep), "{keep}");
        }
        for skip in [
            "libcef.lib",
            "bootstrap.exe",
            "bootstrapc.exe",
            "CREDITS.html",
            "archive.json",
            "CMakeLists.txt",
        ] {
            assert!(!is_cef_runtime_file(skip), "{skip}");
        }
        assert_eq!(locale_of("en-US.pak"), "en-US");
        assert_eq!(locale_of("uk_FEMININE.pak"), "uk");
        assert_eq!(locale_of("pt-BR_NEUTER.pak"), "pt-BR");
    }

    #[test]
    fn human_sizes() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2.0 KB");
        assert_eq!(human(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
    }
}
