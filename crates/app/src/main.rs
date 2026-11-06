// Release builds on Windows are GUI apps (no console window).
#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod cli;
mod stdio_proxy;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use bmcp_core::{BrowserBackend, DataDir, ProfileId};
use tokio_util::sync::CancellationToken;

fn main() -> ExitCode {
    // `browsermcp mcp-stdio`: a thin proxy that must never initialize CEF.
    if std::env::args().nth(1).as_deref() == Some("mcp-stdio") {
        let cli = cli::Cli::parse(std::env::args().skip(2));
        let result = DataDir::resolve(cli.data_dir.as_deref())
            .map_err(anyhow::Error::from)
            .and_then(|data| stdio_proxy::run(&data, cli.api_port));
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("browsermcp mcp-stdio: {err:#}");
                ExitCode::FAILURE
            }
        };
    }

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
    // Logs go to stderr: stdout is reserved for command output (and later MCP over stdio).
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
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
    let profile = ProfileId::new(cli.profile.clone()).context("--profile")?;

    // Async world: API servers, CDP calls. The main thread belongs to CEF.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("bmcp-tokio")
        .enable_all()
        .build()
        .context("starting tokio")?;
    let handle = runtime.handle().clone();
    let shutdown = CancellationToken::new();

    let on_ready: bmcp_cef::ReadyCallback = {
        let shutdown = shutdown.clone();
        let runtime_dir = data.runtime();
        let (dump_ax, api_port) = (cli.dump_ax, cli.api_port);
        Box::new(move |backend: Arc<dyn BrowserBackend>| {
            if dump_ax {
                handle.spawn(dump_ax_and_quit(backend));
            } else {
                handle.spawn(start_api(backend, api_port, shutdown, runtime_dir));
            }
        })
    };

    browser.run(bmcp_cef::LaunchOptions {
        start_url: cli.url,
        data: data.clone(),
        profile,
        style: if cli.chrome_style {
            bmcp_cef::Style::Chrome
        } else {
            bmcp_cef::Style::Alloy
        },
        on_ready: Some(on_ready),
    })?;

    shutdown.cancel();
    let _ = std::fs::remove_file(data.runtime().join("endpoint.json"));
    runtime.shutdown_timeout(Duration::from_secs(2));
    Ok(())
}

async fn start_api(
    backend: Arc<dyn BrowserBackend>,
    port: u16,
    shutdown: CancellationToken,
    runtime_dir: std::path::PathBuf,
) {
    // One registry for every API surface: built-in commands + API-crate commands.
    let mut registry = bmcp_core::Registry::new();
    bmcp_core::commands::register_builtin(&mut registry);
    bmcp_api::commands::register(&mut registry);
    tracing::info!(commands = registry.len(), "command registry ready");
    let events = bmcp_core::EventBus::new();
    match bmcp_api::serve(backend, Arc::new(registry), events, port, shutdown).await {
        Ok(server) => {
            // Lets tools (and the future `mcp-stdio` proxy) find the running instance.
            let endpoint = serde_json::json!({
                "pid": std::process::id(),
                "mcp": server.mcp_url(),
            });
            if let Err(err) =
                std::fs::write(runtime_dir.join("endpoint.json"), endpoint.to_string())
            {
                tracing::warn!(%err, "cannot write endpoint.json");
            }
        }
        Err(err) => tracing::error!(%err, "API server not started"),
    }
}

/// Spike S2: wait for the first page, print its accessibility tree, quit.
async fn dump_ax_and_quit(backend: Arc<dyn BrowserBackend>) {
    let result = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        let tab = loop {
            let tabs = backend.list_tabs().await?;
            if let Some(tab) = tabs.into_iter().find(|t| !t.loading && !t.url.is_empty()) {
                break tab;
            }
            anyhow::ensure!(
                tokio::time::Instant::now() < deadline,
                "page did not load in 30s"
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        let tree = backend
            .cdp(
                tab.id,
                "Accessibility.getFullAXTree".into(),
                serde_json::json!({}),
            )
            .await?;
        let count = tree["nodes"].as_array().map_or(0, Vec::len);
        println!(
            "tab {} \"{}\" {} — {count} AX nodes",
            tab.id, tab.title, tab.url
        );
        println!("{}", bmcp_api::format_ax_tree(&tree, 200));
        anyhow::Ok(())
    }
    .await;
    if let Err(err) = result {
        eprintln!("dump-ax failed: {err:#}");
    }
    let _ = backend.quit().await;
}
