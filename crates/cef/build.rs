//! Picks the folder that `src/ui_files.rs` embeds as `bmcp://ui/`:
//! `ui/dist` (the Svelte build, `pnpm -C ui build`) when it exists, otherwise a small
//! "UI not built" placeholder, so a plain `cargo build` / CI still compiles.

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let dist = manifest.join("../../ui/dist");
    let fallback = manifest.join("assets/fallback");

    // Rebuild (and re-embed) when the UI build changes; cargo scans directories recursively.
    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-changed={}", fallback.display());

    let chosen = if dist.join("index.html").is_file() {
        dist
    } else {
        println!(
            "cargo:warning=ui/dist not found: embedding the placeholder UI \
             (run `pnpm -C ui build`, then rebuild)"
        );
        fallback
    };
    let chosen = chosen.canonicalize().unwrap_or(chosen);
    println!("cargo:rustc-env=BMCP_UI_DIST={}", chosen.display());
}
