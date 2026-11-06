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
    // One bus for everything: the CEF layer emits `tab.*` on it, the API forwards them.
    let events = bmcp_core::EventBus::new();
    // The UI pages need the real API port before the server starts, so pick it now.
    let api_port = resolve_port(cli.api_port);

    let on_ready: bmcp_cef::ReadyCallback = {
        let shutdown = shutdown.clone();
        let runtime_dir = data.runtime();
        let events = events.clone();
        let data = data.clone();
        let dump_ax = cli.dump_ax;
        Box::new(move |backend: Arc<dyn BrowserBackend>| {
            if dump_ax {
                handle.spawn(dump_ax_and_quit(backend));
            } else {
                handle.spawn(start_api(
                    backend,
                    events,
                    api_port,
                    shutdown,
                    runtime_dir,
                    data,
                ));
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
        layout: if cli.single_layout {
            bmcp_cef::WindowLayout::Single
        } else {
            bmcp_cef::WindowLayout::Chrome
        },
        ui: bmcp_cef::UiOptions {
            api_ws: Some(format!("ws://127.0.0.1:{api_port}/v1/ws")),
            dir: cli.ui_dir.clone(),
            dev_url: cli.ui_dev.clone(),
        },
        events,
        on_ready: Some(on_ready),
    })?;

    shutdown.cancel();
    let _ = std::fs::remove_file(data.runtime().join("endpoint.json"));
    runtime.shutdown_timeout(Duration::from_secs(2));
    Ok(())
}

/// `--api-port 0` means "any free port": ask the OS for one now (the UI pages are told the
/// port before the server starts). Another program could grab it in between; then the
/// server logs a bind error.
fn resolve_port(port: u16) -> u16 {
    if port != 0 {
        return port;
    }
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map_or(cli::DEFAULT_API_PORT, |addr| addr.port())
}

async fn start_api(
    backend: Arc<dyn BrowserBackend>,
    events: bmcp_core::EventBus,
    port: u16,
    shutdown: CancellationToken,
    runtime_dir: std::path::PathBuf,
    data: DataDir,
) {
    // One registry for every API surface: built-in commands + API-crate commands.
    let mut registry = bmcp_core::Registry::new();
    bmcp_core::commands::register_builtin(&mut registry);
    bmcp_api::commands::register(&mut registry);
    bmcp_browser::register(&mut registry);
    bmcp_net::register(&mut registry);
    // Bookmarks, history, passwords… in data/db/browsermcp.sqlite.
    let store = match bmcp_data::DataStore::open_in(&data) {
        Ok(store) => {
            registry.extensions().insert(store.clone());
            Some(store)
        }
        Err(err) => {
            tracing::error!(%err, "cannot open browsermcp.sqlite; data commands disabled");
            None
        }
    };
    bmcp_data::register(&mut registry);
    tracing::info!(commands = registry.len(), "command registry ready");
    // Records history from the `tab.updated` / `nav.goto` events on the shared bus.
    if let Some(store) = store {
        bmcp_data::spawn_history_recorder(store, &events, Some(backend.clone()));
    }
    match bmcp_api::serve(backend, Arc::new(registry), events, port, shutdown).await {
        Ok(server) => {
            // Lets tools (and the future `mcp-stdio` proxy) find the running instance.
            let endpoint = serde_json::json!({
                "pid": std::process::id(),
                "mcp": server.mcp_url(),
                "rest": server.rest_url(),
                "ws": server.ws_url(),
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
            // A new tab reports its requested URL at once, so wait for the page's title.
            if let Some(tab) = tabs
                .into_iter()
                .find(|t| !t.loading && !t.url.is_empty() && !t.title.is_empty())
            {
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
