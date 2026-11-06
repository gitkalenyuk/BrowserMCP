//! Command-line options of the browser process.
//!
//! Hand-rolled on purpose: Chromium passes its own switches (`--type=renderer`,
//! `--lang=...`, any `--enable-features`) through the same command line, so unknown
//! flags must be ignored instead of rejected.

use std::path::PathBuf;

pub const DEFAULT_URL: &str = "https://example.com/";
pub const DEFAULT_API_PORT: u16 = 47800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub url: String,
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
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_owned(),
            data_dir: None,
            profile: "default".to_owned(),
            chrome_style: false,
            single_layout: false,
            help: false,
            api_port: DEFAULT_API_PORT,
            dump_ax: false,
        }
    }
}

pub const HELP: &str = "\
browsermcp [OPTIONS] [URL]
browsermcp mcp-stdio [--data-dir <PATH>] [--api-port <PORT>]
    MCP over stdio; starts the browser in the background if needed.

Options:
  --url <URL>          Page to open (default: https://example.com/)
  --data-dir <PATH>    Data folder (default: ./data next to the executable)
  --profile <ID>       Profile to use (default: default)
  --chrome-style       Use CEF Chrome runtime style (experimental; default is Alloy)
  --layout <chrome|single>
                       Window layout: UI strip + page + Ctrl+K palette (default),
                       or the page alone (Phase 0)
  --api-port <PORT>    Local API port, MCP at http://127.0.0.1:<PORT>/mcp (default: 47800)
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
                "--api-port" => {
                    if let Some(port) = value(&mut args).and_then(|v| v.parse().ok()) {
                        cli.api_port = port;
                    }
                }
                "-h" | "--help" => cli.help = true,
                // Bare positional argument = URL.
                other if !other.starts_with('-') => cli.url = other.to_owned(),
                // Unknown switch: belongs to Chromium.
                _ => {}
            }
        }
        cli
    }
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
    fn layout_flag() {
        assert!(!Cli::parse(Vec::<String>::new()).single_layout);
        assert!(Cli::parse(["--layout", "single"]).single_layout);
        assert!(Cli::parse(["--layout=single"]).single_layout);
        assert!(!Cli::parse(["--layout=chrome"]).single_layout);
    }

    #[test]
    fn value_flag_without_value_keeps_default() {
        let cli = Cli::parse(["--url", "--chrome-style"]);
        assert_eq!(cli.url, DEFAULT_URL);
        assert!(cli.chrome_style);
    }
}
