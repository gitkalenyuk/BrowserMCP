fn main() {
    // Chromium needs an application manifest declaring Windows 10 support (supportedOS),
    // otherwise the GPU process hits a CHECK and crashes (STATUS_BREAKPOINT).
    // Manifest copied from cef-rs (build_util/win/cef-app.exe.manifest), which takes it
    // from CEF's own cefsimple sample.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("windows")
            .join("browsermcp.exe.manifest");
        println!("cargo::rerun-if-changed={}", manifest.display());
        println!("cargo::rustc-link-arg-bin=browsermcp=/MANIFEST:EMBED");
        println!(
            "cargo::rustc-link-arg-bin=browsermcp=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
