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

    // Services the browser needs from its first request on: the adblock gate (asked in the
    // network stack for every request) and the site permission rules. The same instances go
    // into the command registry later.
    let adblock = bmcp_adblock::AdblockService::new(data.root());
    adblock.gate.set_native(true); // adblock.apply then skips its CDP Fetch interception
    let gate: Arc<dyn bmcp_adblock::ResourceGate> = adblock.gate.clone();
    let permissions = bmcp_profiles::PermissionStore::load(data.config().join("permissions.json"));
    let cdp_port = checked_cdp_port(cli.cdp_port, &data);

    let on_ready: bmcp_cef::ReadyCallback = {
        let shutdown = shutdown.clone();
        let runtime_dir = data.runtime();
        let events = events.clone();
        let data = data.clone();
        let dump_ax = cli.dump_ax;
        let permissions = permissions.clone();
        let profile = profile.to_string();
        Box::new(move |backend: Arc<dyn BrowserBackend>| {
            if dump_ax {
                handle.spawn(dump_ax_and_quit(backend));
            } else {
                handle.spawn(start_api(
                    backend,
                    ApiServices {
                        events,
                        port: api_port,
                        shutdown,
                        runtime_dir,
                        data,
                        adblock,
                        permissions,
                        profile,
                    },
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
        resource_gate: Some(gate),
        permissions: Some(permissions),
        cdp_port,
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

/// `--cdp-port`, unless a profile has a stealth identity: an open debugging port is easy for
/// sites and local software to detect, so the two are never combined.
fn checked_cdp_port(port: Option<u16>, data: &DataDir) -> Option<u16> {
    let port = port?;
    let identities = bmcp_identity::Services::new(data.config())
        .identities
        .read()
        .map(|f| f.profiles)
        .unwrap_or_default();
    if identities.is_empty() {
        Some(port)
    } else {
        let profiles: Vec<&String> = identities.keys().collect();
        tracing::error!(
            ?profiles,
            "--cdp-port refused: these profiles use a stealth identity (identity_assign); \
             remove the assignment to use the CDP port"
        );
        None
    }
}

/// Everything `start_api` needs besides the backend.
struct ApiServices {
    events: bmcp_core::EventBus,
    port: u16,
    shutdown: CancellationToken,
    runtime_dir: std::path::PathBuf,
    data: DataDir,
    adblock: bmcp_adblock::AdblockService,
    permissions: bmcp_profiles::PermissionStore,
    /// The profile the browser runs with (`--profile`).
    profile: String,
}

async fn start_api(backend: Arc<dyn BrowserBackend>, services: ApiServices) {
    let ApiServices {
        events,
        port,
        shutdown,
        runtime_dir,
        data,
        adblock,
        permissions,
        profile,
    } = services;
    // One registry for every API surface; command crates are registered in `bmcp-registry`.
    let registry = bmcp_registry::build_registry();
    // Command crates that keep files (filter lists, site policies…) find the data folder here.
    registry.extensions().insert(data.clone());
    // Identities and proxy pools in data/config.
    bmcp_identity::install(&registry, data.config());
    // The adblock service whose gate the browser already asks for every request; load the
    // compiled filter lists (data/filters/engine.dat) now instead of on the first command.
    let adblock = registry.extensions().insert(adblock);
    tokio::spawn(async move {
        if let Err(err) = adblock.ensure_engine(false).await {
            tracing::warn!(%err, "adblock engine not loaded");
        }
    });
    // Profiles (profile.*, cookies.*, permissions.*) talk to the CEF backend.
    let profiles = bmcp_profiles::install(&registry, data.root(), &profile, permissions);
    profiles.set_backend(Arc::new(bmcp_cef::CefBackend));
    // Bookmarks, history, passwords… in data/db/browsermcp.sqlite (used by the data commands).
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
    // Built-in AI: agents reach the browser through this same registry (API providers) or the
    // MCP endpoint (Claude Code / Codex).
    let ai = registry.extensions().insert(bmcp_ai::AiService::new(
        data.root(),
        format!("http://127.0.0.1:{port}/mcp"),
    ));
    // Recordings and generated scripts live in the data folder.
    registry
        .extensions()
        .insert(bmcp_record::DataDirExt(data.clone()));
    // Scripts, mass runs, triggers (data/scripts, data/db/runs.sqlite).
    let jobs =
        bmcp_jobs::JobsService::start(bmcp_jobs::JobsConfig::new(&data, port), events.clone())
            .inspect_err(|err| tracing::error!(%err, "jobs service not started"))
            .ok();
    if let Some(jobs) = &jobs {
        registry.extensions().insert(jobs.clone());
    }
    tracing::info!(commands = registry.len(), "command registry ready");
    let registry = Arc::new(registry);
    bmcp_ai::attach(&registry);
    // New tabs of a profile with an identity get it before their first navigation.
    bmcp_cef::set_tab_preparer(
        Arc::new(bmcp_profiles::IdentityPreparer::new(
            registry.clone(),
            backend.clone(),
            events.clone(),
        )),
        tokio::runtime::Handle::current(),
    );
    // Profile settings (proxy forwarders, prefs) and imported cookies.
    tokio::spawn(apply_profiles_and_cookies(
        registry.clone(),
        backend.clone(),
        events.clone(),
    ));
    // Records history from the `tab.updated` / `nav.goto` events on the shared bus, and
    // downloads from the `download.*` events of the CEF download handler.
    if let Some(store) = store {
        bmcp_data::downloads::spawn_download_recorder(store.clone(), &events);
        bmcp_data::spawn_history_recorder(store, &events, Some(backend.clone()));
    }
    match bmcp_api::serve(backend, registry, events, port, shutdown.clone()).await {
        Ok(server) => {
            ai.set_mcp_url(server.mcp_url()); // the real port when `--api-port 0`
            if let Some(jobs) = &jobs {
                jobs.set_ws_url(server.ws_url());
            }
            // Kill agent, terminal and script child processes when the browser quits.
            tokio::spawn(async move {
                shutdown.cancelled().await;
                ai.shutdown();
                if let Some(jobs) = jobs {
                    jobs.shutdown();
                }
            });
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

/// At startup: apply every stored profile (forwarders, preferences) and move cookies waiting
/// from `import.chrome` into the cookie jar; then again after every successful import.
async fn apply_profiles_and_cookies(
    registry: Arc<bmcp_core::Registry>,
    backend: Arc<dyn BrowserBackend>,
    events: bmcp_core::EventBus,
) {
    let ctx = || bmcp_core::Ctx::new(bmcp_core::Caller::Ui, backend.clone(), events.clone());
    let mut ctx_with_ext = ctx();
    ctx_with_ext.extensions = registry.extensions().clone();
    bmcp_profiles::apply_stored(&ctx_with_ext).await;
    let mut finished = events.subscribe();
    let apply_pending = || async {
        match registry
            .call(ctx(), "cookies.apply_pending", serde_json::json!({}))
            .await
        {
            Ok(out)
                if out["applied"].as_u64().unwrap_or(0) > 0
                    || out["failed"].as_u64().unwrap_or(0) > 0 =>
            {
                tracing::info!(%out, "imported cookies applied");
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(%err, "imported cookies not applied"),
        }
    };
    apply_pending().await;
    loop {
        match finished.recv().await {
            Ok(event)
                if event.topic == bmcp_core::events::topics::COMMAND_FINISHED
                    && event.data["name"] == "import.chrome"
                    && event.data["ok"] == true =>
            {
                apply_pending().await;
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
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
