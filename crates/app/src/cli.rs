//! Command-line options of the browser process.
//!
//! Hand-rolled on purpose: Chromium passes its own switches (`--type=renderer`,
//! `--lang=...`, any `--enable-features`) through the same command line, so unknown
//! flags must be ignored instead of rejected.

use std::path::PathBuf;

pub const DEFAULT_URL: &str = "https://example.com/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub url: String,
    pub data_dir: Option<PathBuf>,
    pub profile: String,
    pub alloy: bool,
    pub help: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            url: DEFAULT_URL.to_owned(),
            data_dir: None,
            profile: "default".to_owned(),
            alloy: false,
            help: false,
        }
    }
}

pub const HELP: &str = "\
browsermcp [OPTIONS] [URL]

Options:
  --url <URL>          Page to open (default: https://example.com/)
  --data-dir <PATH>    Data folder (default: ./data next to the executable)
  --profile <ID>       Profile to use (default: default)
  --alloy              Use CEF Alloy runtime style instead of Chrome style
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
                "--alloy" => cli.alloy = true,
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
        let cli = Cli::parse(["https://b.test", "--alloy"]);
        assert_eq!(cli.url, "https://b.test");
        assert!(cli.alloy);
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
    fn value_flag_without_value_keeps_default() {
        let cli = Cli::parse(["--url", "--alloy"]);
        assert_eq!(cli.url, DEFAULT_URL);
        assert!(cli.alloy);
    }
}
