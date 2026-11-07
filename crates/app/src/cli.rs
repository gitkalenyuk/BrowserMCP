//! Command-line options of the browser process.
//!
//! Hand-rolled on purpose: Chromium passes its own switches (`--type=renderer`,
//! `--lang=...`, any `--enable-features`) through the same command line, so unknown
//! flags must be ignored instead of rejected.

use std::path::PathBuf;

pub const DEFAULT_URL: &str = "https://example.com/";
pub const DEFAULT_API_PORT: u16 = 47800;

#[derive(Debug, Clone, PartialEq)]
pub struct Cli {
    pub url: String,
    /// `--url` (or a bare URL) was on the command line; otherwise Settings → Startup decides
    /// what opens.
    pub url_given: bool,
    pub data_dir: Option<PathBuf>,
    pub profile: String,
    pub chrome_style: bool,
    /// `--layout single`: Phase 0 window (page only). Default: composed UI + page.
    pub single_layout: bool,
    pub help: bool,
    /// Port of the local API (MCP at /mcp). 0 = pick a free port.
    pub api_port: u16,
    /// Spike S2: print the page's accessibility tree, then quit.
    pub dump_ax: bool,
    /// `--ui-dir <PATH>`: serve `bmcp://ui/` from this folder instead of the embedded build.
    pub ui_dir: Option<PathBuf>,
    /// `--ui-dev <URL>`: load the chrome from a Vite dev server (hot reload).
    pub ui_dev: Option<String>,
    /// `--cdp-port <N>`: open Chromium's remote debugging port (Playwright/Puppeteer).
    pub cdp_port: Option<u16>,
    /// `--no-restore`: start with the start page instead of the last session.
    pub no_restore: bool,
    /// `--headless`: no window at all; every tab is an off-screen (windowless) page.
    pub headless: bool,
    /// `--window-size <W>x<H>`: page size of headless tabs (default 1280x800).
    pub window_size: Option<(u32, u32)>,
    /// `--device-scale <F>`: device pixel ratio of headless tabs (default 1).
    pub device_scale: Option<f32>,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_owned(),
            url_given: false,
            data_dir: None,
            profile: "default".to_owned(),
            chrome_style: false,
            single_layout: false,
            help: false,
            api_port: DEFAULT_API_PORT,
            dump_ax: false,
            ui_dir: None,
            ui_dev: None,
            cdp_port: None,
            no_restore: false,
            headless: false,
            window_size: None,
            device_scale: None,
        }
    }
}

pub const HELP: &str = "\
browsermcp [OPTIONS] [URL]
browsermcp mcp-stdio [--data-dir <PATH>] [--api-port <PORT>] [--headless ...]
    MCP over stdio; starts the browser in the background if needed (the headless
    options below are passed on to it).

Options:
  --url <URL>          Page to open (default: Settings → Startup & tabs; headless: https://example.com/)
  --data-dir <PATH>    Data folder (default: ./data next to the executable)
  --profile <ID>       Profile to use (default: default)
  --chrome-style       Use CEF Chrome runtime style (experimental; default is Alloy)
  --layout <chrome|single>
                       Window layout: UI strip + page + Ctrl+K palette (default),
                       or the page alone (Phase 0)
  --api-port <PORT>    Local API port, MCP at http://127.0.0.1:<PORT>/mcp (default: 47800)
  --ui-dir <PATH>      Serve the browser UI (bmcp://ui/) from this folder, e.g. ui/dist,
                       instead of the copy built into the executable
  --ui-dev <URL>       Load the UI from a Vite dev server, e.g. http://localhost:5173
                       (hot reload; also set BMCP_DEV_UI=1 so the API accepts it)
  --cdp-port <PORT>    Open the Chrome DevTools Protocol port for Playwright/Puppeteer
                       (connectOverCDP http://127.0.0.1:<PORT>). Off by default; refused
                       while any profile has a stealth identity
  --no-restore         Do not re-open the last session's tabs (data/runtime/session.json),
                       whatever Settings → Startup & tabs says
  --headless           No window at all: the first tab and every later tab are
                       off-screen pages (screenshots, video, CDP input work); the
                       process stays up after the last tab closes, until system.quit
  --window-size <W>x<H>
                       Page size of headless tabs (default 1280x800)
  --device-scale <F>   Device pixel ratio of headless tabs, 0.5-4 (default 1)
  --dump-ax            Print the page's accessibility tree and exit
  -h, --help           Show this help

Any other --switch is passed through to Chromium.";

impl Cli {
    /// Parse arguments (without the program name).
    pub fn parse<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut cli = Cli::default();
        let mut args = args.into_iter().map(Into::into).peekable();

        while let Some(arg) = args.next() {
            let (name, inline_value) = match arg.split_once('=') {
                Some((n, v)) if n.starts_with("--") => (n.to_owned(), Some(v.to_owned())),
                _ => (arg.clone(), None),
            };
            // Value either inline (`--url=x`) or the next arg if it is not a flag (`--url x`).
            let value = |args: &mut std::iter::Peekable<_>| -> Option<String> {
                inline_value
                    .clone()
                    .or_else(|| args.next_if(|next: &String| !next.starts_with("--")))
            };

            match name.as_str() {
                "--url" => {
                    if let Some(v) = value(&mut args) {
                        cli.url = v;
                        cli.url_given = true;
                    }
                }
                "--data-dir" => cli.data_dir = value(&mut args).map(PathBuf::from),
                "--profile" => {
                    if let Some(v) = value(&mut args) {
                        cli.profile = v;
                    }
                }
                "--chrome-style" => cli.chrome_style = true,
                "--layout" => {
                    if let Some(v) = value(&mut args) {
                        cli.single_layout = v == "single";
                    }
                }
                "--dump-ax" => cli.dump_ax = true,
                "--no-restore" => cli.no_restore = true,
                "--ui-dir" => cli.ui_dir = value(&mut args).map(PathBuf::from),
                "--ui-dev" => cli.ui_dev = value(&mut args),
                "--cdp-port" => {
                    cli.cdp_port = value(&mut args)
                        .and_then(|v| v.parse().ok())
                        .filter(|p: &u16| *p != 0);
                }
                "--headless" => cli.headless = true,
                "--window-size" => {
                    cli.window_size = value(&mut args).and_then(|v| parse_size(&v));
                }
                "--device-scale" => {
                    cli.device_scale = value(&mut args)
                        .and_then(|v| v.parse().ok())
                        .filter(|f: &f32| (0.5..=4.0).contains(f));
                }
                "--api-port" => {
                    if let Some(port) = value(&mut args).and_then(|v| v.parse().ok()) {
                        cli.api_port = port;
                    }
                }
                "-h" | "--help" => cli.help = true,
                // Bare positional argument = URL.
                other if !other.starts_with('-') => {
                    cli.url = other.to_owned();
                    cli.url_given = true;
                }
                // Unknown switch: belongs to Chromium.
                _ => {}
            }
        }
        cli
    }

    /// Browser options `mcp-stdio` passes on when it has to start the browser itself.
    pub fn headless_args(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.headless {
            out.push("--headless".to_owned());
        }
        if let Some((w, h)) = self.window_size {
            out.push(format!("--window-size={w}x{h}"));
        }
        if let Some(scale) = self.device_scale {
            out.push(format!("--device-scale={scale}"));
        }
        out
    }
}

/// `1280x800` (also `1280,800`, Chromium's own `--window-size` syntax).
fn parse_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once(['x', 'X', ','])?;
    let (w, h): (u32, u32) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    let ok = |n: u32| (100..=8192).contains(&n);
    (ok(w) && ok(h)).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        assert_eq!(Cli::parse(Vec::<String>::new()), Cli::default());
    }

    #[test]
    fn inline_and_separate_values() {
        let cli = Cli::parse([
            "--url=https://a.test",
            "--data-dir",
            "D:/x",
            "--profile=work",
        ]);
        assert_eq!(cli.url, "https://a.test");
        assert_eq!(cli.data_dir, Some(PathBuf::from("D:/x")));
        assert_eq!(cli.profile, "work");
    }

    #[test]
    fn positional_url_and_flags() {
        let cli = Cli::parse(["https://b.test", "--chrome-style"]);
        assert_eq!(cli.url, "https://b.test");
        assert!(cli.chrome_style);
    }

    #[test]
    fn chromium_switches_are_ignored() {
        let cli = Cli::parse([
            "--enable-features=Foo,Bar",
            "--lang=en-US",
            "--remote-debugging-port=9222",
            "--url",
            "https://c.test",
        ]);
        assert_eq!(cli.url, "https://c.test");
    }

    #[test]
    fn api_port_and_dump_ax() {
        let cli = Cli::parse(["--api-port=0", "--dump-ax"]);
        assert_eq!(cli.api_port, 0);
        assert!(cli.dump_ax);
        assert_eq!(
            Cli::parse(["--api-port", "nope"]).api_port,
            DEFAULT_API_PORT
        );
    }

    #[test]
    fn ui_flags() {
        let cli = Cli::parse(["--ui-dir", "ui/dist", "--ui-dev=http://localhost:5173"]);
        assert_eq!(cli.ui_dir, Some(PathBuf::from("ui/dist")));
        assert_eq!(cli.ui_dev.as_deref(), Some("http://localhost:5173"));
    }

    #[test]
    fn no_restore_flag() {
        assert!(!Cli::parse(Vec::<String>::new()).no_restore);
        assert!(Cli::parse(["--no-restore"]).no_restore);
    }

    #[test]
    fn cdp_port_flag() {
        assert_eq!(Cli::parse(Vec::<String>::new()).cdp_port, None);
        assert_eq!(Cli::parse(["--cdp-port", "9333"]).cdp_port, Some(9333));
        assert_eq!(Cli::parse(["--cdp-port=0"]).cdp_port, None);
        assert_eq!(Cli::parse(["--cdp-port", "x"]).cdp_port, None);
    }

    #[test]
    fn layout_flag() {
        assert!(!Cli::parse(Vec::<String>::new()).single_layout);
        assert!(Cli::parse(["--layout", "single"]).single_layout);
        assert!(Cli::parse(["--layout=single"]).single_layout);
        assert!(!Cli::parse(["--layout=chrome"]).single_layout);
    }

    #[test]
    fn headless_flags() {
        let cli = Cli::parse(Vec::<String>::new());
        assert!(!cli.headless);
        assert!(cli.headless_args().is_empty());
        let cli = Cli::parse([
            "--headless",
            "--window-size",
            "1024x768",
            "--device-scale=2",
            "--url",
            "about:blank",
        ]);
        assert!(cli.headless);
        assert_eq!(cli.window_size, Some((1024, 768)));
        assert_eq!(cli.device_scale, Some(2.0));
        assert_eq!(cli.url, "about:blank");
        assert_eq!(
            cli.headless_args(),
            ["--headless", "--window-size=1024x768", "--device-scale=2"]
        );
        // Chromium's comma syntax works; nonsense is ignored (defaults apply).
        assert_eq!(
            Cli::parse(["--window-size=800,600"]).window_size,
            Some((800, 600))
        );
        assert_eq!(Cli::parse(["--window-size", "big"]).window_size, None);
        assert_eq!(Cli::parse(["--window-size=10x10"]).window_size, None);
        assert_eq!(Cli::parse(["--device-scale", "9"]).device_scale, None);
    }

    #[test]
    fn value_flag_without_value_keeps_default() {
        let cli = Cli::parse(["--url", "--chrome-style"]);
        assert_eq!(cli.url, DEFAULT_URL);
        assert!(cli.chrome_style);
    }
}
