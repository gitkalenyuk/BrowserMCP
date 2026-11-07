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
            .and_then(|data| stdio_proxy::run(&data, cli.api_port, &cli.headless_args()));
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
    // Bind the API socket BEFORE starting CEF: the UI pages and AI agents are told this port,
    // so it must already be ours (otherwise they could talk to whatever else owns it).
    let api_listener = if cli.dump_ax {
        None
    } else {
        Some(bind_api(cli.api_port, &data)?)
    };
    let api_port = api_listener
        .as_ref()
        .and_then(|l| l.local_addr().ok())
        .map_or(cli.api_port, |addr| addr.port());

    // Services the browser needs from its first request on: the adblock gate (asked in the
    // network stack for every request) and the site permission rules. The same instances go
    // into the command registry later.
    let adblock = bmcp_adblock::AdblockService::new(data.root());
    adblock.gate.set_native(true); // adblock.apply then skips its CDP Fetch interception
    let gate: Arc<dyn bmcp_adblock::ResourceGate> = adblock.gate.clone();
    let permissions = bmcp_profiles::PermissionStore::load(data.config().join("permissions.json"));
    // Startup & tabs, search engines, downloads (data/config): the browser and the commands
    // share this instance.
    let settings = bmcp_settings::BrowserSettings::load(&data);
    // What opens first: `--url`, else Settings → Startup & tabs. `--dump-ax` prints the start
    // page and headless runs start clean: neither restores a session, both keep the old
    // default page when no URL is given.
    let automation = cli.dump_ax || cli.headless;
    let start = bmcp_settings::startup::plan(
        &settings.startup(),
        (cli.url_given || automation).then_some(cli.url.as_str()),
        !cli.no_restore && !automation,
    );
    let cdp_port = checked_cdp_port(cli.cdp_port, &data);

    let on_ready: bmcp_cef::ReadyCallback = {
        let shutdown = shutdown.clone();
        let runtime_dir = data.runtime();
        let events = events.clone();
        let data = data.clone();
        let permissions = permissions.clone();
        let settings = settings.clone();
        let more_urls = start.more_urls.clone();
        let profile = profile.to_string();
        Box::new(move |backend: Arc<dyn BrowserBackend>| match api_listener {
            None => {
                handle.spawn(dump_ax_and_quit(backend));
            }
            Some(listener) => {
                handle.spawn(start_api(
                    backend,
                    ApiServices {
                        events,
                        listener,
                        shutdown,
                        runtime_dir,
                        data,
                        adblock,
                        permissions,
                        settings,
                        more_urls,
                        profile,
                    },
                ));
            }
        })
    };

    browser.run(bmcp_cef::LaunchOptions {
        start_url: start.first_url,
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
        settings: Some(settings),
        cdp_port,
        restore_session: start.restore,
        headless: cli.headless.then(|| {
            let mut headless = bmcp_cef::HeadlessOptions::default();
            if let Some((width, height)) = cli.window_size {
                (headless.width, headless.height) = (width, height);
            }
            if let Some(scale) = cli.device_scale {
                headless.device_scale = scale;
            }
            headless
        }),
    })?;

    shutdown.cancel();
    let _ = std::fs::remove_file(data.runtime().join("endpoint.json"));
    runtime.shutdown_timeout(Duration::from_secs(2));
    Ok(())
}

/// Bind 127.0.0.1:`port` (`0` = any free port) for the API. A busy port usually means another
/// BrowserMCP is already running on this data folder or port.
fn bind_api(port: u16, data: &DataDir) -> anyhow::Result<std::net::TcpListener> {
    std::net::TcpListener::bind(("127.0.0.1", port)).with_context(|| {
        format!(
            "cannot use API port {port} on 127.0.0.1 (another BrowserMCP may be running: see {});              pick another with --api-port <n> or --api-port 0",
            data.runtime().join("endpoint.json").display()
        )
    })
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
    listener: std::net::TcpListener,
    shutdown: CancellationToken,
    runtime_dir: std::path::PathBuf,
    data: DataDir,
    adblock: bmcp_adblock::AdblockService,
    permissions: bmcp_profiles::PermissionStore,
    settings: bmcp_settings::BrowserSettings,
    /// Settings → Startup "open specific pages": the pages after the first one.
    more_urls: Vec<String>,
    /// The profile the browser runs with (`--profile`).
    profile: String,
}

async fn start_api(backend: Arc<dyn BrowserBackend>, services: ApiServices) {
    let ApiServices {
        events,
        listener,
        shutdown,
        runtime_dir,
        data,
        adblock,
        permissions,
        settings,
        more_urls,
        profile,
    } = services;
    let port = listener.local_addr().map_or(0, |addr| addr.port());
    // The browser is up (this runs once its first window exists): Settings → About's uptime.
    let started_ms = bmcp_data::store::now_ms();
    // One registry for every API surface; command crates are registered in `bmcp-registry`.
    let registry = bmcp_registry::build_registry();
    // Command crates that keep files (filter lists, site policies…) find the data folder here.
    registry.extensions().insert(data.clone());
    // startup.config / search.config / downloads.config change the instance the browser reads.
    registry.extensions().insert(settings.clone());
    // system.open_path / reveal_path may also show the chosen download folder and the exe's
    // folder (its license files, Settings → About).
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_default();
    registry
        .extensions()
        .insert(bmcp_system::ExtraRoots(Arc::new({
            let settings = settings.clone();
            let exe_dir = exe_dir.clone();
            move || vec![settings.download_dir(None), exe_dir.clone()]
        })));
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
    // Website languages, Chromium's locale, spellcheck (data/config/languages.json; the CEF
    // layer read the same file before Chromium started).
    bmcp_languages::install(&registry, data.config()).set_backend(Arc::new(bmcp_cef::CefBackend));
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
        // Isolated rows with `proxy_pool` / `fit_identity`: lease + identity per row.
        jobs.set_row_profiles(Arc::new(RowProfilesHook(bmcp_profiles::RowProfiles::new(
            profiles.clone(),
        ))));
    }
    // Self-healing of runs and page.extract (data/extract-cache).
    registry
        .extensions()
        .insert(bmcp_heal::HealService::new(bmcp_heal::HealConfig::new(
            &data,
        )));
    // Plugins, userscripts and webhook triggers (data/plugins, data/userscripts,
    // data/config/webhooks.json). The auth handle lets the API restrict plugin connections.
    let plugins = bmcp_plugins::PluginsService::open(
        bmcp_plugins::PluginsConfig::new(&data, port),
        events.clone(),
    );
    registry.extensions().insert(plugins.clone());
    registry.extensions().insert(plugins.auth_handle());
    // Notifications: UI toasts, Windows toasts, Telegram bot, webhook (data/config/notify.json).
    // Routes bus events (run.finished, run.heal_needed, …) by its rules; serves ui.ask_user.
    let notify = registry
        .extensions()
        .insert(bmcp_notify::NotifyService::start(
            bmcp_notify::NotifyOptions::new(data.config().join("notify.json")),
            events.clone(),
        ));
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
    // Tab freezing/discarding and automatic limits from RAM (data/config/governor.json).
    bmcp_governor::start(registry.extensions(), backend.clone(), events.clone());
    // Watch every tab for JavaScript dialogs (events, auto-policy, DIALOG_OPEN errors).
    bmcp_pagetools::start(&registry, backend.clone(), events.clone());
    // Notification buttons (heal: Skip row / Stop run) run commands through the registry.
    bmcp_notify::attach(&registry, backend.clone());
    // Watches run.heal_needed (proposes patches) and takes the page snapshot of broken rows.
    bmcp_heal::attach(&registry, backend.clone(), events.clone());
    // Webhooks start runs through the registry.
    plugins.attach(&registry);
    // Records history from the `tab.updated` / `nav.goto` events on the shared bus, and
    // downloads from the `download.*` events of the CEF download handler.
    if let Some(store) = store {
        bmcp_data::downloads::spawn_download_recorder(store.clone(), &events);
        bmcp_data::spawn_history_recorder(store, &events, Some(backend.clone()));
    }
    // Link hints in every tab (the in-page `f` / `F` keys).
    bmcp_hints::start(
        registry.extensions().clone(),
        backend.clone(),
        events.clone(),
    );
    // Settings → About and the extra startup pages need these once the API listens.
    let (about_registry, about_backend) = (registry.clone(), backend.clone());
    let about_settings = settings.clone();
    // Serve on the socket bound before CEF started (binding the port again would fail).
    match bmcp_api::serve_on(backend, registry, events, listener, shutdown.clone()).await {
        Ok(server) => {
            ai.set_mcp_url(server.mcp_url()); // the real port when `--api-port 0`
            if let Some(jobs) = &jobs {
                jobs.set_ws_url(server.ws_url());
            }
            // Plugin processes connect to the API, so they start once it listens.
            plugins.start(server.ws_url());
            // Kill agent, terminal, script and plugin child processes when the browser quits.
            tokio::spawn(async move {
                shutdown.cancelled().await;
                ai.shutdown();
                notify.shutdown();
                if let Some(jobs) = jobs {
                    jobs.shutdown();
                }
                plugins.shutdown();
            });
            // Settings → About.
            let cef_version = bmcp_cef::cef_version();
            about_registry.extensions().insert(bmcp_settings::AppInfo {
                chromium_version: bmcp_settings::about::chromium_from_cef(&cef_version),
                cef_version,
                exe_dir,
                api_port: port,
                mcp_url: server.mcp_url(),
                rest_url: server.rest_url(),
                ws_url: server.ws_url(),
                started_ms,
            });
            // Settings → Startup "open specific pages": the rest of the pages, in order.
            let position = about_settings.startup().new_tab_position;
            for url in bmcp_settings::startup::open_order(&more_urls, position) {
                if let Err(err) = about_backend.open_tab(url.clone(), true).await {
                    tracing::warn!(url, %err, "startup page not opened");
                }
            }
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

/// `bmcp_jobs::RowProfileHook` on top of `bmcp_profiles::RowProfiles` (the two crates do
/// not depend on each other).
struct RowProfilesHook(bmcp_profiles::RowProfiles);

impl RowProfilesHook {
    fn request(row: &bmcp_jobs::RowProfile) -> bmcp_profiles::RowRequest {
        bmcp_profiles::RowRequest {
            run: row.run.clone(),
            row: row.row,
            profile: row.profile.clone(),
            pool: row.proxy_pool.clone(),
            fit_identity: row.fit_identity,
            rotate: row.rotate,
        }
    }
}

impl bmcp_jobs::RowProfileHook for RowProfilesHook {
    fn check(
        &self,
        proxy_pool: Option<&str>,
        fit_identity: bool,
    ) -> Result<(), bmcp_core::CmdError> {
        self.0.check(proxy_pool, fit_identity)
    }

    fn prepare(
        &self,
        row: bmcp_jobs::RowProfile,
    ) -> bmcp_core::BoxFuture<'static, Result<(), bmcp_core::CmdError>> {
        let rows = self.0.clone();
        Box::pin(async move { rows.prepare(&Self::request(&row)).await.map(|_| ()) })
    }

    fn release(
        &self,
        row: bmcp_jobs::RowProfile,
        row_done: bool,
    ) -> bmcp_core::BoxFuture<'static, ()> {
        let rows = self.0.clone();
        Box::pin(async move { rows.release(&Self::request(&row), row_done).await })
    }

    fn run_finished(
        &self,
        run: String,
        proxy_pool: Option<String>,
        profile_prefix: String,
    ) -> bmcp_core::BoxFuture<'static, ()> {
        let rows = self.0.clone();
        Box::pin(async move {
            rows.finish_run(&run, proxy_pool.as_deref(), &profile_prefix)
                .await
        })
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
    // Re-apply a profile's prefs when its proxy routing / identity changes (subscribed
    // before the startup pass, so nothing is missed).
    if let Ok(service) = bmcp_profiles::ProfilesService::from_ctx(&ctx_with_ext) {
        bmcp_profiles::watch(service, &events);
    }
    bmcp_profiles::apply_stored(&ctx_with_ext).await;
    // Restored tabs of those profiles may load now (their proxy/identity is in place).
    bmcp_cef::profiles_applied();
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
