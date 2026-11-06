// Release builds on Windows are GUI apps (no console window).
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod cli;

use std::process::ExitCode;

use anyhow::Context;
use bmcp_core::{DataDir, ProfileId};

fn main() -> ExitCode {
    // Chromium re-launches this same executable for renderer/GPU/utility processes.
    // That check must happen before anything else.
    let browser = match bmcp_cef::start() {
        bmcp_cef::Process::SubprocessExited(code) => return ExitCode::from(code as u8),
        bmcp_cef::Process::Browser(browser) => browser,
    };

    match run(browser) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("browsermcp: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(browser: bmcp_cef::Browser) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = cli::Cli::parse(std::env::args().skip(1));
    if cli.help {
        println!("{}", cli::HELP);
        return Ok(());
    }

    let data = DataDir::resolve(cli.data_dir.as_deref()).context("resolving data folder")?;
    let profile = ProfileId::new(cli.profile).context("--profile")?;

    browser.run(bmcp_cef::LaunchOptions {
        start_url: cli.url,
        data,
        profile,
        style: if cli.alloy {
            bmcp_cef::Style::Alloy
        } else {
            bmcp_cef::Style::Chrome
        },
    })?;
    Ok(())
}
